use super::{Context, ContextError};
use super::rbac::lookup_env_vars;
use k8s_openapi::api::core::v1::Secret;
use std::collections::HashMap;
use tokio::fs;
use tracing::debug;

impl Context {
    /// Create a Secret from file contents
    ///
    /// The files map keys become the secret data keys, values are file paths.
    /// Files are read synchronously from the local filesystem.
    ///
    /// # Example
    ///
    /// ```ignore
    /// use std::collections::HashMap;
    ///
    /// let mut files = HashMap::new();
    /// files.insert("ca.crt".to_string(), "/path/to/ca.crt".to_string());
    /// files.insert("tls.crt".to_string(), "/path/to/tls.crt".to_string());
    ///
    /// ctx.secret_from_file("certs", files).await?;
    /// ```
    pub async fn secret_from_file(
        &self,
        name: &str,
        files: HashMap<String, String>,
    ) -> Result<Secret, ContextError> {
        let mut data = std::collections::BTreeMap::new();

        for (key, path) in files {
            let content = fs::read(&path).await.map_err(|e| {
                ContextError::ApplyError(format!("failed to read file {path}: {e}"))
            })?;
            data.insert(key, k8s_openapi::ByteString(content));
        }

        let secret = Secret {
            metadata: kube::api::ObjectMeta {
                name: Some(name.to_string()),
                namespace: Some(self.namespace.clone()),
                ..Default::default()
            },
            data: Some(data),
            ..Default::default()
        };

        debug!(
            namespace = %self.namespace,
            secret = %name,
            "Creating secret from files"
        );

        self.apply(&secret).await
    }

    /// Create a Secret from environment variables
    ///
    /// Each key is looked up in the current process environment.
    /// Returns an error if any key is not set.
    ///
    /// # Example
    ///
    /// ```ignore
    /// // With env vars: API_KEY=abc123, API_SECRET=xyz789
    /// ctx.secret_from_env("api-keys", &["API_KEY", "API_SECRET"]).await?;
    /// ```
    pub async fn secret_from_env(&self, name: &str, keys: &[&str]) -> Result<Secret, ContextError> {
        let env_data = lookup_env_vars(keys).map_err(ContextError::ApplyError)?;

        let mut data = std::collections::BTreeMap::new();
        for (key, value) in env_data {
            data.insert(key, k8s_openapi::ByteString(value.into_bytes()));
        }

        let secret = Secret {
            metadata: kube::api::ObjectMeta {
                name: Some(name.to_string()),
                namespace: Some(self.namespace.clone()),
                ..Default::default()
            },
            data: Some(data),
            ..Default::default()
        };

        debug!(
            namespace = %self.namespace,
            secret = %name,
            keys = ?keys,
            "Creating secret from environment variables"
        );

        self.apply(&secret).await
    }

    /// Create a TLS Secret from certificate and key files
    ///
    /// Creates a Kubernetes TLS secret with the standard `tls.crt` and `tls.key` keys.
    /// The secret type is set to `kubernetes.io/tls`.
    ///
    /// # Example
    ///
    /// ```ignore
    /// ctx.secret_tls("my-tls", "/path/to/cert.pem", "/path/to/key.pem").await?;
    /// ```
    pub async fn secret_tls(
        &self,
        name: &str,
        cert_path: &str,
        key_path: &str,
    ) -> Result<Secret, ContextError> {
        let cert_data = fs::read(cert_path).await.map_err(|e| {
            ContextError::ApplyError(format!("failed to read certificate {cert_path}: {e}"))
        })?;

        let key_data = fs::read(key_path)
            .await
            .map_err(|e| ContextError::ApplyError(format!("failed to read key {key_path}: {e}")))?;

        let mut data = std::collections::BTreeMap::new();
        data.insert("tls.crt".to_string(), k8s_openapi::ByteString(cert_data));
        data.insert("tls.key".to_string(), k8s_openapi::ByteString(key_data));

        let secret = Secret {
            metadata: kube::api::ObjectMeta {
                name: Some(name.to_string()),
                namespace: Some(self.namespace.clone()),
                ..Default::default()
            },
            type_: Some("kubernetes.io/tls".to_string()),
            data: Some(data),
            ..Default::default()
        };

        debug!(
            namespace = %self.namespace,
            secret = %name,
            "Creating TLS secret"
        );

        self.apply(&secret).await
    }
}
