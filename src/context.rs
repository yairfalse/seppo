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

    #[error("Failed to delete resource: {0}")]
    DeleteError(String),

    #[error("Failed to list resources: {0}")]
    ListError(String),

    #[error("Failed to get logs: {0}")]
    LogsError(String),

    #[error("Wait timeout: {0}")]
    WaitTimeout(String),
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

    /// Delete a resource from the test namespace
    pub async fn delete<K>(&self, name: &str) -> Result<(), ContextError>
    where
        K: kube::Resource<Scope = kube::core::NamespaceResourceScope>
            + Clone
            + serde::de::DeserializeOwned
            + std::fmt::Debug,
        <K as kube::Resource>::DynamicType: Default,
    {
        let api: Api<K> = Api::namespaced(self.client.clone(), &self.namespace);

        api.delete(name, &DeleteParams::default())
            .await
            .map_err(|e| ContextError::DeleteError(e.to_string()))?;

        info!(
            namespace = %self.namespace,
            name = %name,
            "Deleted resource"
        );

        Ok(())
    }

    /// List resources of a given type in the test namespace
    pub async fn list<K>(&self) -> Result<Vec<K>, ContextError>
    where
        K: kube::Resource<Scope = kube::core::NamespaceResourceScope>
            + Clone
            + serde::de::DeserializeOwned
            + std::fmt::Debug,
        <K as kube::Resource>::DynamicType: Default,
    {
        let api: Api<K> = Api::namespaced(self.client.clone(), &self.namespace);

        let list = api
            .list(&Default::default())
            .await
            .map_err(|e| ContextError::ListError(e.to_string()))?;

        Ok(list.items)
    }

    /// Get logs from a pod in the test namespace
    pub async fn logs(&self, pod_name: &str) -> Result<String, ContextError> {
        use k8s_openapi::api::core::v1::Pod;

        let pods: Api<Pod> = Api::namespaced(self.client.clone(), &self.namespace);

        let logs = pods
            .logs(pod_name, &Default::default())
            .await
            .map_err(|e| ContextError::LogsError(e.to_string()))?;

        Ok(logs)
    }

    /// Wait for a resource to satisfy a condition
    ///
    /// Polls the resource until the condition returns true or timeout is reached.
    /// Default timeout is 60 seconds, polling interval is 1 second.
    ///
    /// # Example
    ///
    /// ```ignore
    /// // Wait for a ConfigMap to have a specific key
    /// ctx.wait_for::<ConfigMap>("my-config", |cm| {
    ///     cm.data.as_ref().map_or(false, |d| d.contains_key("ready"))
    /// }).await?;
    /// ```
    pub async fn wait_for<K, F>(&self, name: &str, condition: F) -> Result<K, ContextError>
    where
        K: kube::Resource<Scope = kube::core::NamespaceResourceScope>
            + Clone
            + serde::de::DeserializeOwned
            + std::fmt::Debug,
        <K as kube::Resource>::DynamicType: Default,
        F: Fn(&K) -> bool,
    {
        self.wait_for_with_timeout(name, condition, std::time::Duration::from_secs(60))
            .await
    }

    /// Wait for a resource with custom timeout
    pub async fn wait_for_with_timeout<K, F>(
        &self,
        name: &str,
        condition: F,
        timeout: std::time::Duration,
    ) -> Result<K, ContextError>
    where
        K: kube::Resource<Scope = kube::core::NamespaceResourceScope>
            + Clone
            + serde::de::DeserializeOwned
            + std::fmt::Debug,
        <K as kube::Resource>::DynamicType: Default,
        F: Fn(&K) -> bool,
    {
        let start = std::time::Instant::now();
        let poll_interval = std::time::Duration::from_secs(1);

        loop {
            match self.get::<K>(name).await {
                Ok(resource) => {
                    if condition(&resource) {
                        return Ok(resource);
                    }
                }
                Err(ContextError::GetError(_)) => {
                    // Resource doesn't exist yet, keep waiting
                }
                Err(e) => return Err(e),
            }

            if start.elapsed() >= timeout {
                return Err(ContextError::WaitTimeout(format!(
                    "Timed out waiting for {} after {:?}",
                    name, timeout
                )));
            }

            tokio::time::sleep(poll_interval).await;
        }
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

    /// Test that delete() removes a resource from the test namespace
    #[tokio::test]
    #[ignore] // Requires real cluster
    async fn test_context_delete_removes_resource() {
        use k8s_openapi::api::core::v1::ConfigMap;

        let ctx = TestContext::new().await.expect("Should create context");

        // Create a ConfigMap
        let cm = ConfigMap {
            metadata: kube::api::ObjectMeta {
                name: Some("to-delete".to_string()),
                ..Default::default()
            },
            ..Default::default()
        };

        ctx.apply(&cm).await.expect("Should apply ConfigMap");

        // Delete it
        ctx.delete::<ConfigMap>("to-delete")
            .await
            .expect("Should delete ConfigMap");

        // Verify it's gone
        let result: Result<ConfigMap, _> = ctx.get("to-delete").await;
        assert!(result.is_err(), "ConfigMap should be deleted");

        // Cleanup
        ctx.cleanup().await.expect("Should cleanup");
    }

    /// Test that list() returns resources from the test namespace
    #[tokio::test]
    #[ignore] // Requires real cluster
    async fn test_context_list_returns_resources() {
        use k8s_openapi::api::core::v1::ConfigMap;

        let ctx = TestContext::new().await.expect("Should create context");

        // Create multiple ConfigMaps
        for i in 0..3 {
            let cm = ConfigMap {
                metadata: kube::api::ObjectMeta {
                    name: Some(format!("config-{}", i)),
                    ..Default::default()
                },
                ..Default::default()
            };
            ctx.apply(&cm).await.expect("Should apply ConfigMap");
        }

        // List them
        let configs: Vec<ConfigMap> = ctx.list().await.expect("Should list ConfigMaps");

        assert_eq!(configs.len(), 3, "Should have 3 ConfigMaps");

        // Cleanup
        ctx.cleanup().await.expect("Should cleanup");
    }

    /// Test that logs() retrieves pod logs
    #[tokio::test]
    #[ignore] // Requires real cluster with a running pod
    async fn test_context_logs_retrieves_pod_logs() {
        use k8s_openapi::api::core::v1::Pod;

        let ctx = TestContext::new().await.expect("Should create context");

        // Create a simple pod that outputs something
        let pod = Pod {
            metadata: kube::api::ObjectMeta {
                name: Some("test-pod".to_string()),
                ..Default::default()
            },
            spec: Some(k8s_openapi::api::core::v1::PodSpec {
                containers: vec![k8s_openapi::api::core::v1::Container {
                    name: "test".to_string(),
                    image: Some("busybox:latest".to_string()),
                    command: Some(vec![
                        "sh".to_string(),
                        "-c".to_string(),
                        "echo 'hello from seppo' && sleep 30".to_string(),
                    ]),
                    ..Default::default()
                }],
                restart_policy: Some("Never".to_string()),
                ..Default::default()
            }),
            ..Default::default()
        };

        ctx.apply(&pod).await.expect("Should apply Pod");

        // Wait a bit for pod to start and output logs
        tokio::time::sleep(tokio::time::Duration::from_secs(5)).await;

        // Get logs
        let logs = ctx.logs("test-pod").await.expect("Should get logs");

        assert!(
            logs.contains("hello from seppo"),
            "Logs should contain our message, got: {}",
            logs
        );

        // Cleanup
        ctx.cleanup().await.expect("Should cleanup");
    }

    /// Test that wait_for() waits for a condition to be met
    #[tokio::test]
    #[ignore] // Requires real cluster
    async fn test_context_wait_for_condition() {
        use k8s_openapi::api::core::v1::ConfigMap;

        let ctx = TestContext::new().await.expect("Should create context");

        // Create a ConfigMap
        let cm = ConfigMap {
            metadata: kube::api::ObjectMeta {
                name: Some("wait-test".to_string()),
                ..Default::default()
            },
            data: Some(
                [("status".to_string(), "ready".to_string())]
                    .into_iter()
                    .collect(),
            ),
            ..Default::default()
        };

        ctx.apply(&cm).await.expect("Should apply ConfigMap");

        // Wait for it to have the "status" key with value "ready"
        let result: ConfigMap = ctx
            .wait_for("wait-test", |cm: &ConfigMap| {
                cm.data
                    .as_ref()
                    .and_then(|d| d.get("status"))
                    .is_some_and(|v| v == "ready")
            })
            .await
            .expect("Should find ConfigMap with condition");

        assert_eq!(
            result.data.unwrap().get("status"),
            Some(&"ready".to_string())
        );

        // Cleanup
        ctx.cleanup().await.expect("Should cleanup");
    }

    /// Test that wait_for() times out when condition is not met
    #[tokio::test]
    #[ignore] // Requires real cluster
    async fn test_context_wait_for_timeout() {
        use k8s_openapi::api::core::v1::ConfigMap;

        let ctx = TestContext::new().await.expect("Should create context");

        // Create a ConfigMap without the expected data
        let cm = ConfigMap {
            metadata: kube::api::ObjectMeta {
                name: Some("timeout-test".to_string()),
                ..Default::default()
            },
            ..Default::default()
        };

        ctx.apply(&cm).await.expect("Should apply ConfigMap");

        // Wait for a condition that will never be true (with short timeout)
        let result: Result<ConfigMap, _> = ctx
            .wait_for_with_timeout(
                "timeout-test",
                |cm: &ConfigMap| {
                    cm.data
                        .as_ref()
                        .is_some_and(|d| d.contains_key("never-exists"))
                },
                std::time::Duration::from_secs(2),
            )
            .await;

        assert!(
            matches!(result, Err(ContextError::WaitTimeout(_))),
            "Should timeout"
        );

        // Cleanup
        ctx.cleanup().await.expect("Should cleanup");
    }
}
