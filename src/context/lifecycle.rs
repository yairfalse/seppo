use super::{Context, ContextError};
use k8s_openapi::api::core::v1::Namespace;
use kube::api::{Api, DeleteParams, PostParams};
use kube::Client;
use tracing::info;

impl Context {
    /// Create a new context with an isolated namespace
    ///
    /// Creates a unique namespace in the cluster for this context.
    /// The namespace will be labeled with `seppo.io/test=true`.
    ///
    /// # Errors
    ///
    /// Returns `ContextError` if client creation or namespace creation fails.
    pub async fn new() -> Result<Self, ContextError> {
        // 1. Create kube::Client
        let client = Client::try_default()
            .await
            .map_err(|e| ContextError::ClientError(e.to_string()))?;

        // 2. Generate unique namespace name
        let id = uuid::Uuid::new_v4().to_string();
        let namespace = format!("seppo-test-{}", &id[..8]);

        // 3. Create namespace in cluster
        let namespaces: Api<Namespace> = Api::all(client.clone());
        let ns = Namespace {
            metadata: kube::api::ObjectMeta {
                name: Some(namespace.clone()),
                labels: Some(
                    [("seppo.io/test".to_string(), "true".to_string())]
                        .into_iter()
                        .collect(),
                ),
                ..Default::default()
            },
            ..Default::default()
        };

        namespaces
            .create(&PostParams::default(), &ns)
            .await
            .map_err(|e| ContextError::NamespaceError(e.to_string()))?;

        info!(namespace = %namespace, "Created test namespace");

        Ok(Self { client, namespace })
    }

    /// Cleanup the test namespace
    ///
    /// # Errors
    ///
    /// Returns `ContextError::CleanupError` if namespace deletion fails.
    pub async fn cleanup(&self) -> Result<(), ContextError> {
        let namespaces: Api<Namespace> = Api::all(self.client.clone());

        namespaces
            .delete(&self.namespace, &DeleteParams::default())
            .await
            .map_err(|e| ContextError::CleanupError(e.to_string()))?;

        info!(namespace = %self.namespace, "Deleted test namespace");

        Ok(())
    }
}
