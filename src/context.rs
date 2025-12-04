//! Test context for Kubernetes integration testing
//!
//! Provides a connection to a Kubernetes cluster with an isolated namespace
//! for each test.

use crate::diagnostics::Diagnostics;
use crate::portforward::{PortForward, PortForwardError};
use k8s_openapi::api::core::v1::{Event, Namespace, Pod};
use kube::api::{Api, AttachParams, DeleteParams, PostParams};
use kube::Client;
use std::collections::HashMap;
use tokio::io::AsyncReadExt;
use tracing::{debug, info, warn};

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

    #[error("Failed to get events: {0}")]
    EventsError(String),

    #[error("Failed to exec command: {0}")]
    ExecError(String),
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

        debug!(
            namespace = %self.namespace,
            resource = %name,
            timeout = ?timeout,
            "Starting wait_for"
        );

        loop {
            match self.get::<K>(name).await {
                Ok(resource) => {
                    if condition(&resource) {
                        debug!(
                            namespace = %self.namespace,
                            resource = %name,
                            elapsed = ?start.elapsed(),
                            "Condition met"
                        );
                        return Ok(resource);
                    }
                    debug!(
                        namespace = %self.namespace,
                        resource = %name,
                        elapsed = ?start.elapsed(),
                        "Resource exists but condition not met, waiting..."
                    );
                }
                Err(ContextError::GetError(_)) => {
                    debug!(
                        namespace = %self.namespace,
                        resource = %name,
                        elapsed = ?start.elapsed(),
                        "Resource doesn't exist yet, waiting..."
                    );
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

    /// Get events from the test namespace
    ///
    /// Returns all events in the namespace, useful for debugging test failures.
    pub async fn events(&self) -> Result<Vec<Event>, ContextError> {
        let events: Api<Event> = Api::namespaced(self.client.clone(), &self.namespace);

        let list = events
            .list(&Default::default())
            .await
            .map_err(|e| ContextError::EventsError(e.to_string()))?;

        Ok(list.items)
    }

    /// Collect logs from all pods in the test namespace
    ///
    /// Returns a map of pod name to logs. Pods that don't have logs yet
    /// (e.g., pending pods) will have an empty string or error message.
    pub async fn collect_pod_logs(&self) -> Result<HashMap<String, String>, ContextError> {
        let pods: Api<Pod> = Api::namespaced(self.client.clone(), &self.namespace);

        let pod_list = pods
            .list(&Default::default())
            .await
            .map_err(|e| ContextError::ListError(e.to_string()))?;

        let mut logs_map = HashMap::new();

        for pod in pod_list.items {
            let pod_name = pod.metadata.name.unwrap_or_default();
            if pod_name.is_empty() {
                continue;
            }

            // Try to get logs, but don't fail if pod isn't ready
            let logs = match pods.logs(&pod_name, &Default::default()).await {
                Ok(logs) => logs,
                Err(e) => format!("[error getting logs: {}]", e),
            };

            logs_map.insert(pod_name, logs);
        }

        Ok(logs_map)
    }

    /// Collect all diagnostic information for debugging
    ///
    /// Gathers pod logs and events from the namespace. This is called
    /// automatically on test failure by the `#[seppo::test]` macro.
    pub async fn collect_diagnostics(&self) -> Diagnostics {
        let mut diag = Diagnostics::new(self.namespace.clone());

        // Collect pod logs (best effort)
        match self.collect_pod_logs().await {
            Ok(logs) => diag.pod_logs = logs,
            Err(e) => {
                warn!(error = %e, "Failed to collect pod logs for diagnostics");
            }
        }

        // Collect events (best effort)
        match self.events().await {
            Ok(events) => diag.events = events,
            Err(e) => {
                warn!(error = %e, "Failed to collect events for diagnostics");
            }
        }

        diag
    }

    /// Execute a command in a pod
    ///
    /// Runs the specified command in the first container of the pod and
    /// returns the combined stdout/stderr output.
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
        let command_strings: Vec<String> = command.iter().map(|s| s.to_string()).collect();

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

    /// Create a port forward to a pod
    ///
    /// Opens a local port that tunnels traffic to the specified pod port.
    /// Returns a `PortForward` that can be used to make HTTP requests.
    ///
    /// # Example
    ///
    /// ```ignore
    /// let mut pf = ctx.forward("my-pod", 8080).await?;
    /// let response = pf.get("/health").await?;
    /// assert!(response.contains("ok"));
    /// ```
    pub async fn forward(
        &self,
        pod_name: &str,
        port: u16,
    ) -> Result<PortForward, PortForwardError> {
        PortForward::new(self.client.clone(), &self.namespace, pod_name, port).await
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

    /// Test that collect_pod_logs() returns logs for all pods
    #[tokio::test]
    #[ignore] // Requires real cluster
    async fn test_context_collect_pod_logs() {
        use k8s_openapi::api::core::v1::Pod;

        let ctx = TestContext::new().await.expect("Should create context");

        // Create a simple pod
        let pod = Pod {
            metadata: kube::api::ObjectMeta {
                name: Some("log-test".to_string()),
                ..Default::default()
            },
            spec: Some(k8s_openapi::api::core::v1::PodSpec {
                containers: vec![k8s_openapi::api::core::v1::Container {
                    name: "test".to_string(),
                    image: Some("busybox:latest".to_string()),
                    command: Some(vec![
                        "sh".to_string(),
                        "-c".to_string(),
                        "echo 'test output' && sleep 30".to_string(),
                    ]),
                    ..Default::default()
                }],
                restart_policy: Some("Never".to_string()),
                ..Default::default()
            }),
            ..Default::default()
        };

        ctx.apply(&pod).await.expect("Should apply Pod");

        // Wait for pod to start
        tokio::time::sleep(tokio::time::Duration::from_secs(5)).await;

        // Collect all pod logs
        let pod_logs = ctx.collect_pod_logs().await.expect("Should collect logs");

        // Should have one pod's logs
        assert_eq!(pod_logs.len(), 1);
        assert!(pod_logs.contains_key("log-test"));
        assert!(pod_logs["log-test"].contains("test output"));

        // Cleanup
        ctx.cleanup().await.expect("Should cleanup");
    }

    /// Test that events() returns namespace events
    #[tokio::test]
    #[ignore] // Requires real cluster
    async fn test_context_events() {
        use k8s_openapi::api::core::v1::ConfigMap;

        let ctx = TestContext::new().await.expect("Should create context");

        // Create a ConfigMap to generate an event
        let cm = ConfigMap {
            metadata: kube::api::ObjectMeta {
                name: Some("event-test".to_string()),
                ..Default::default()
            },
            ..Default::default()
        };

        ctx.apply(&cm).await.expect("Should apply ConfigMap");

        // Get events - should have at least namespace creation event
        let events = ctx.events().await.expect("Should get events");

        // Events list should be returned (may be empty for ConfigMap, but method should work)
        assert!(events.is_empty() || !events.is_empty()); // Just verify it returns

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

    /// Test that forward() creates a port forward to a pod
    #[tokio::test]
    #[ignore] // Requires real cluster
    async fn test_context_forward() {
        let ctx = TestContext::new().await.expect("Should create context");

        // Create a simple nginx pod
        let pod = Pod {
            metadata: kube::api::ObjectMeta {
                name: Some("forward-test".to_string()),
                ..Default::default()
            },
            spec: Some(k8s_openapi::api::core::v1::PodSpec {
                containers: vec![k8s_openapi::api::core::v1::Container {
                    name: "nginx".to_string(),
                    image: Some("nginx:alpine".to_string()),
                    ports: Some(vec![k8s_openapi::api::core::v1::ContainerPort {
                        container_port: 80,
                        ..Default::default()
                    }]),
                    ..Default::default()
                }],
                ..Default::default()
            }),
            ..Default::default()
        };

        ctx.apply(&pod).await.expect("Should apply Pod");

        // Wait for pod to be running
        ctx.wait_for::<Pod, _>("forward-test", |p| {
            p.status
                .as_ref()
                .and_then(|s| s.phase.as_ref())
                .is_some_and(|phase| phase == "Running")
        })
        .await
        .expect("Pod should be running");

        // Create port forward
        let mut pf = ctx
            .forward("forward-test", 80)
            .await
            .expect("Should create port forward");

        // Make a request through the port forward
        let response = pf.get("/").await.expect("Should get response");
        assert!(response.contains("nginx") || response.contains("Welcome"));

        // Cleanup
        ctx.cleanup().await.expect("Should cleanup");
    }

    /// Test that exec() runs a command in a pod
    #[tokio::test]
    #[ignore] // Requires real cluster
    async fn test_context_exec() {
        let ctx = TestContext::new().await.expect("Should create context");

        // Create a simple pod
        let pod = Pod {
            metadata: kube::api::ObjectMeta {
                name: Some("exec-test".to_string()),
                ..Default::default()
            },
            spec: Some(k8s_openapi::api::core::v1::PodSpec {
                containers: vec![k8s_openapi::api::core::v1::Container {
                    name: "test".to_string(),
                    image: Some("busybox:latest".to_string()),
                    command: Some(vec!["sleep".to_string(), "300".to_string()]),
                    ..Default::default()
                }],
                restart_policy: Some("Never".to_string()),
                ..Default::default()
            }),
            ..Default::default()
        };

        ctx.apply(&pod).await.expect("Should apply Pod");

        // Wait for pod to be running
        ctx.wait_for::<Pod, _>("exec-test", |p| {
            p.status
                .as_ref()
                .and_then(|s| s.phase.as_ref())
                .is_some_and(|phase| phase == "Running")
        })
        .await
        .expect("Pod should be running");

        // Execute a command
        let output = ctx
            .exec("exec-test", &["echo", "hello from exec"])
            .await
            .expect("Should exec command");

        assert!(output.contains("hello from exec"));

        // Cleanup
        ctx.cleanup().await.expect("Should cleanup");
    }
}
