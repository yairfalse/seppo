use super::{Context, ContextError};
use k8s_openapi::api::core::v1::Pod;
use kube::api::{Api, AttachParams};
use tokio::io::AsyncReadExt;
use tracing::debug;

impl Context {
    /// Execute a command in a pod
    ///
    /// Runs the specified command in the first container of the pod and
    /// returns the stdout output as a string.
    ///
    /// # Example
    ///
    /// ```ignore
    /// let output = ctx.exec("my-pod", &["cat", "/etc/config"]).await?;
    /// assert!(output.contains("expected content"));
    /// ```
    pub async fn exec(&self, pod_name: &str, command: &[&str]) -> Result<String, ContextError> {
        let pods: Api<Pod> = Api::namespaced(self.client.clone(), &self.namespace);

        let attach_params = AttachParams {
            stdout: true,
            stderr: true,
            ..Default::default()
        };

        // Convert &[&str] to Vec<String> for kube API
        let command_strings: Vec<String> = command.iter().map(|s| (*s).to_string()).collect();

        let mut attached = pods
            .exec(pod_name, command_strings, &attach_params)
            .await
            .map_err(|e| ContextError::ExecError(e.to_string()))?;

        // Collect stdout
        let mut stdout = attached
            .stdout()
            .ok_or_else(|| ContextError::ExecError("No stdout stream available".to_string()))?;

        let mut output = String::new();
        stdout
            .read_to_string(&mut output)
            .await
            .map_err(|e| ContextError::ExecError(e.to_string()))?;

        debug!(
            namespace = %self.namespace,
            pod = %pod_name,
            command = ?command,
            "Executed command in pod"
        );

        Ok(output)
    }

    /// Copy a file from a pod to the local filesystem
    ///
    /// Reads a file from the specified path in the pod and returns its contents.
    /// Uses `cat` under the hood, so works with any pod that has this command.
    ///
    /// # Example
    ///
    /// ```ignore
    /// let content = ctx.copy_from("my-pod", "/etc/config.yaml").await?;
    /// println!("Config: {}", content);
    /// ```
    pub async fn copy_from(
        &self,
        pod_name: &str,
        remote_path: &str,
    ) -> Result<String, ContextError> {
        let output = self
            .exec(pod_name, &["cat", remote_path])
            .await
            .map_err(|e| ContextError::CopyError(format!("failed to read {remote_path}: {e}")))?;

        debug!(
            namespace = %self.namespace,
            pod = %pod_name,
            path = %remote_path,
            "Copied file from pod"
        );

        Ok(output)
    }

    /// Copy content to a file in a pod
    ///
    /// Writes the given content to a file at the specified path in the pod.
    /// Uses shell redirection, so works with any pod that has a shell.
    ///
    /// # Example
    ///
    /// ```ignore
    /// ctx.copy_to("my-pod", "/tmp/config.yaml", "key: value\n").await?;
    /// ```
    pub async fn copy_to(
        &self,
        pod_name: &str,
        remote_path: &str,
        content: &str,
    ) -> Result<(), ContextError> {
        // Validate remote_path to prevent command injection
        if remote_path.contains(';')
            || remote_path.contains('`')
            || remote_path.contains('$')
            || remote_path.contains('|')
            || remote_path.contains('&')
            || remote_path.contains('\n')
            || remote_path.contains('\r')
        {
            return Err(ContextError::CopyError(format!(
                "invalid path '{remote_path}': contains shell metacharacters"
            )));
        }

        // Escape content and path for shell
        let escaped_content = content.replace('\'', "'\"'\"'");
        let escaped_path = remote_path.replace('\'', "'\"'\"'");
        let command = format!("printf '%s' '{escaped_content}' > '{escaped_path}'");

        self.exec(pod_name, &["sh", "-c", &command])
            .await
            .map_err(|e| ContextError::CopyError(format!("failed to write {remote_path}: {e}")))?;

        debug!(
            namespace = %self.namespace,
            pod = %pod_name,
            path = %remote_path,
            "Copied content to pod"
        );

        Ok(())
    }
}
