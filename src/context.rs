//! Test context for Kubernetes integration testing
//!
//! Provides a connection to a Kubernetes cluster with an isolated namespace
//! for each test.

use k8s_openapi::api::core::v1::Namespace;
use kube::api::{Api, DeleteParams, PostParams};
use kube::Client;
use tracing::info;

/// Test context providing K8s connection and isolated namespace
pub struct TestContext {
    /// Kubernetes client
    pub client: Client,
    /// Isolated namespace for this test
    pub namespace: String,
}

/// Errors from TestContext operations
#[derive(Debug, thiserror::Error)]
pub enum ContextError {
    #[error("Failed to create Kubernetes client: {0}")]
    ClientError(String),

    #[error("Failed to create namespace: {0}")]
    NamespaceError(String),

    #[error("Failed to cleanup namespace: {0}")]
    CleanupError(String),

    #[error("Failed to apply resource: {0}")]
    ApplyError(String),

    #[error("Failed to get resource: {0}")]
    GetError(String),
}

impl TestContext {
    /// Create a new test context with an isolated namespace
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
    pub async fn cleanup(&self) -> Result<(), ContextError> {
        let namespaces: Api<Namespace> = Api::all(self.client.clone());

        namespaces
            .delete(&self.namespace, &DeleteParams::default())
            .await
            .map_err(|e| ContextError::CleanupError(e.to_string()))?;

        info!(namespace = %self.namespace, "Deleted test namespace");

        Ok(())
    }

    /// Apply a resource to the test namespace
    ///
    /// Creates the resource in the test namespace, overriding any namespace
    /// specified in the resource metadata.
    pub async fn apply<K>(&self, resource: &K) -> Result<K, ContextError>
    where
        K: kube::Resource<Scope = kube::core::NamespaceResourceScope>
            + Clone
            + serde::de::DeserializeOwned
            + serde::Serialize
            + std::fmt::Debug,
        <K as kube::Resource>::DynamicType: Default,
    {
        let api: Api<K> = Api::namespaced(self.client.clone(), &self.namespace);

        // Clone and set namespace to our test namespace
        let mut resource = resource.clone();
        resource.meta_mut().namespace = Some(self.namespace.clone());

        let created = api
            .create(&PostParams::default(), &resource)
            .await
            .map_err(|e| ContextError::ApplyError(e.to_string()))?;

        info!(
            namespace = %self.namespace,
            name = ?created.meta().name,
            "Applied resource"
        );

        Ok(created)
    }

    /// Get a resource from the test namespace
    pub async fn get<K>(&self, name: &str) -> Result<K, ContextError>
    where
        K: kube::Resource<Scope = kube::core::NamespaceResourceScope>
            + Clone
            + serde::de::DeserializeOwned
            + std::fmt::Debug,
        <K as kube::Resource>::DynamicType: Default,
    {
        let api: Api<K> = Api::namespaced(self.client.clone(), &self.namespace);

        let resource = api
            .get(name)
            .await
            .map_err(|e| ContextError::GetError(e.to_string()))?;

        Ok(resource)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// RED: Test that TestContext::new() creates a namespace
    #[tokio::test]
    #[ignore] // Requires real cluster
    async fn test_context_creates_namespace() {
        // Create context
        let ctx = TestContext::new().await.expect("Should create context");

        // Namespace should start with "seppo-test-"
        assert!(
            ctx.namespace.starts_with("seppo-test-"),
            "Namespace should start with 'seppo-test-', got: {}",
            ctx.namespace
        );

        // Verify namespace exists in cluster
        use k8s_openapi::api::core::v1::Namespace;
        use kube::api::Api;

        let namespaces: Api<Namespace> = Api::all(ctx.client.clone());
        let ns = namespaces
            .get(&ctx.namespace)
            .await
            .expect("Namespace should exist in cluster");

        assert_eq!(ns.metadata.name, Some(ctx.namespace.clone()));

        // Cleanup
        ctx.cleanup().await.expect("Should cleanup");
    }

    /// RED: Test that cleanup deletes the namespace
    #[tokio::test]
    #[ignore] // Requires real cluster
    async fn test_context_cleanup_deletes_namespace() {
        // Create context
        let ctx = TestContext::new().await.expect("Should create context");
        let namespace = ctx.namespace.clone();

        // Cleanup
        ctx.cleanup().await.expect("Should cleanup");

        // Verify namespace is deleted (or deleting)
        use k8s_openapi::api::core::v1::Namespace;
        use kube::api::Api;

        let client = Client::try_default().await.expect("Should create client");
        let namespaces: Api<Namespace> = Api::all(client);

        // Namespace should be gone or terminating
        match namespaces.get(&namespace).await {
            Ok(ns) => {
                // If still exists, should be terminating
                let phase = ns.status.and_then(|s| s.phase);
                assert_eq!(phase, Some("Terminating".to_string()));
            }
            Err(kube::Error::Api(e)) if e.code == 404 => {
                // Good - namespace is gone
            }
            Err(e) => panic!("Unexpected error: {}", e),
        }
    }

    /// RED: Test that apply() creates a resource in the test namespace
    #[tokio::test]
    #[ignore] // Requires real cluster
    async fn test_context_apply_creates_resource() {
        use k8s_openapi::api::core::v1::ConfigMap;

        let ctx = TestContext::new().await.expect("Should create context");

        // Create a ConfigMap
        let cm = ConfigMap {
            metadata: kube::api::ObjectMeta {
                name: Some("test-config".to_string()),
                ..Default::default()
            },
            data: Some(
                [("key".to_string(), "value".to_string())]
                    .into_iter()
                    .collect(),
            ),
            ..Default::default()
        };

        // Apply it
        let created = ctx.apply(&cm).await.expect("Should apply ConfigMap");

        // Verify it was created in our namespace
        assert_eq!(
            created.metadata.namespace,
            Some(ctx.namespace.clone()),
            "Resource should be in test namespace"
        );
        assert_eq!(
            created.metadata.name,
            Some("test-config".to_string()),
            "Resource should have correct name"
        );

        // Cleanup
        ctx.cleanup().await.expect("Should cleanup");
    }

    /// RED: Test that get() retrieves a resource from the test namespace
    #[tokio::test]
    #[ignore] // Requires real cluster
    async fn test_context_get_retrieves_resource() {
        use k8s_openapi::api::core::v1::ConfigMap;

        let ctx = TestContext::new().await.expect("Should create context");

        // Create a ConfigMap first
        let cm = ConfigMap {
            metadata: kube::api::ObjectMeta {
                name: Some("test-config".to_string()),
                ..Default::default()
            },
            data: Some(
                [("mykey".to_string(), "myvalue".to_string())]
                    .into_iter()
                    .collect(),
            ),
            ..Default::default()
        };

        ctx.apply(&cm).await.expect("Should apply ConfigMap");

        // Now get it
        let retrieved: ConfigMap = ctx.get("test-config").await.expect("Should get ConfigMap");

        // Verify the data
        let data = retrieved.data.expect("Should have data");
        assert_eq!(data.get("mykey"), Some(&"myvalue".to_string()));

        // Cleanup
        ctx.cleanup().await.expect("Should cleanup");
    }
}
