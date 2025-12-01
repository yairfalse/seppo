//! Test context for Kubernetes integration testing
//!
//! Provides a connection to a Kubernetes cluster with an isolated namespace
//! for each test.

use kube::Client;

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
}

impl TestContext {
    /// Create a new test context with an isolated namespace
    pub async fn new() -> Result<Self, ContextError> {
        // TODO: Implement
        // 1. Create kube::Client
        // 2. Generate unique namespace name
        // 3. Create namespace in cluster
        // 4. Return TestContext
        todo!("TestContext::new() not implemented")
    }

    /// Cleanup the test namespace
    pub async fn cleanup(&self) -> Result<(), ContextError> {
        // TODO: Implement
        // 1. Delete namespace
        // 2. Wait for deletion
        todo!("TestContext::cleanup() not implemented")
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
}
