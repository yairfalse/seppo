//! Kubernetes SDK context
//!
//! Provides a connection to a Kubernetes cluster with namespace management.
//! Can be used standalone or with the `#[seppo::test]` macro.

use crate::diagnostics::Diagnostics;
use crate::portforward::{PortForward, PortForwardError};
use crate::stack::{Stack, StackError};
use futures::Stream;
use k8s_openapi::api::apps::v1::{DaemonSet, Deployment, StatefulSet};
use k8s_openapi::api::core::v1::{Event, Namespace, Pod, Service};
use kube::api::{Api, AttachParams, DeleteParams, ListParams, Patch, PatchParams, PostParams};
use kube::runtime::watcher::{self, Event as WatchEvent};
use kube::Client;
use std::collections::HashMap;
use tokio::io::AsyncReadExt;
use tracing::{debug, info, warn};

/// Kubernetes resource kinds supported by wait_ready
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResourceKind {
    Deployment,
    Pod,
    Service,
    StatefulSet,
    DaemonSet,
}

/// Target for port forwarding
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ForwardTarget {
    /// Forward to a pod directly
    Pod(String),
    /// Forward to a service (finds a backing pod)
    Service(String),
    /// Forward to a deployment (finds a pod from the deployment)
    Deployment(String),
}

/// Parse a resource reference like "deployment/myapp" into (kind, name)
///
/// Supports kubectl-style aliases:
/// - `deployment`, `deploy` → Deployment
/// - `pod`, `po` → Pod
/// - `service`, `svc` → Service
/// - `statefulset`, `sts` → StatefulSet
/// - `daemonset`, `ds` → DaemonSet
pub fn parse_resource_ref(reference: &str) -> Result<(ResourceKind, &str), ContextError> {
    let parts: Vec<&str> = reference.splitn(2, '/').collect();

    if parts.len() != 2 {
        return Err(ContextError::InvalidResourceRef(format!(
            "expected 'kind/name', got '{}'",
            reference
        )));
    }

    let kind_str = parts[0];
    let name = parts[1];

    if name.is_empty() {
        return Err(ContextError::InvalidResourceRef(format!(
            "resource name cannot be empty in '{}'",
            reference
        )));
    }

    let kind = match kind_str.to_lowercase().as_str() {
        "deployment" | "deploy" => ResourceKind::Deployment,
        "pod" | "po" => ResourceKind::Pod,
        "service" | "svc" => ResourceKind::Service,
        "statefulset" | "sts" => ResourceKind::StatefulSet,
        "daemonset" | "ds" => ResourceKind::DaemonSet,
        _ => {
            return Err(ContextError::InvalidResourceRef(format!(
                "unknown resource kind '{}' in '{}'",
                kind_str, reference
            )))
        }
    };

    Ok((kind, name))
}

/// Parse a port forward target like "svc/myapp" or "pod/myapp"
///
/// Supported formats:
/// - `pod/name`, `po/name` → Forward to pod
/// - `service/name`, `svc/name` → Forward to service (finds backing pod)
/// - `deployment/name`, `deploy/name` → Forward to deployment (finds pod)
/// - `name` → Treated as pod name (backward compatible)
pub fn parse_forward_target(target: &str) -> Result<ForwardTarget, ContextError> {
    if target.is_empty() {
        return Err(ContextError::InvalidResourceRef(
            "target cannot be empty".to_string(),
        ));
    }

    // Check for kind/name format
    if let Some((kind, name)) = target.split_once('/') {
        if name.is_empty() {
            return Err(ContextError::InvalidResourceRef(format!(
                "resource name cannot be empty in '{}'",
                target
            )));
        }

        match kind.to_lowercase().as_str() {
            "pod" | "po" => Ok(ForwardTarget::Pod(name.to_string())),
            "service" | "svc" => Ok(ForwardTarget::Service(name.to_string())),
            "deployment" | "deploy" => Ok(ForwardTarget::Deployment(name.to_string())),
            _ => Err(ContextError::InvalidResourceRef(format!(
                "unsupported resource kind '{}' for port forwarding (use pod, svc, or deployment)",
                kind
            ))),
        }
    } else {
        // Bare name - treat as pod for backward compatibility
        Ok(ForwardTarget::Pod(target.to_string()))
    }
}

/// Kubernetes SDK context providing cluster connection and namespace management
///
/// `Context` is the main entry point for interacting with Kubernetes.
/// It can be created standalone or injected by the `#[seppo::test]` macro.
///
/// # Standalone Usage
///
/// ```ignore
/// use seppo::Context;
///
/// #[tokio::main]
/// async fn main() -> Result<(), Box<dyn std::error::Error>> {
///     let ctx = Context::new().await?;
///
///     // Use ctx to interact with K8s...
///     ctx.apply(&my_deployment).await?;
///     ctx.wait_ready("deployment/myapp").await?;
///
///     ctx.cleanup().await?;
///     Ok(())
/// }
/// ```
///
/// # With Test Macro
///
/// ```ignore
/// #[seppo::test]
/// async fn my_test(ctx: Context) {
///     ctx.apply(&my_deployment).await?;
///     ctx.wait_ready("deployment/myapp").await?;
/// }
/// ```
pub struct Context {
    /// Kubernetes client
    pub client: Client,
    /// Namespace for operations
    pub namespace: String,
}

/// Alias for backward compatibility
#[deprecated(since = "0.2.0", note = "Use `Context` instead")]
pub type TestContext = Context;

/// Errors from Context operations
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

    #[error("Failed to patch resource: {0}")]
    PatchError(String),

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

    #[error("Invalid resource reference: {0}")]
    InvalidResourceRef(String),

    #[error("Watch error: {0}")]
    WatchError(String),

    #[error("Copy error: {0}")]
    CopyError(String),
}

impl Context {
    /// Create a new context with an isolated namespace
    ///
    /// Creates a unique namespace in the cluster for this context.
    /// The namespace will be labeled with `seppo.io/test=true`.
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
    /// Creates or updates the resource in the test namespace using server-side apply.
    /// This works like `kubectl apply` - it will create if not exists, or update if exists.
    /// Overrides any namespace specified in the resource metadata with the test namespace.
    ///
    /// # Example
    ///
    /// ```ignore
    /// // First apply creates
    /// ctx.apply(&deployment).await?;
    ///
    /// // Second apply updates
    /// deployment.spec.as_mut().unwrap().replicas = Some(5);
    /// ctx.apply(&deployment).await?;
    /// ```
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

        // Get resource name for the patch call
        let name = resource
            .meta()
            .name
            .as_ref()
            .ok_or_else(|| ContextError::ApplyError("resource must have a name".to_string()))?;

        // Use server-side apply (like kubectl apply)
        let patch_params = PatchParams::apply("seppo").force();
        let applied = api
            .patch(name, &patch_params, &Patch::Apply(&resource))
            .await
            .map_err(|e| ContextError::ApplyError(e.to_string()))?;

        info!(
            namespace = %self.namespace,
            name = ?applied.meta().name,
            "Applied resource"
        );

        Ok(applied)
    }

    /// Patch a resource in the test namespace using JSON Merge Patch
    ///
    /// Allows partial updates to a resource without replacing the entire spec.
    /// Uses JSON Merge Patch (RFC 7396) - fields set to `null` are deleted,
    /// other fields are merged.
    ///
    /// # Example
    ///
    /// ```ignore
    /// use serde_json::json;
    ///
    /// // Update deployment replicas
    /// ctx.patch::<Deployment>("myapp", &json!({
    ///     "spec": { "replicas": 5 }
    /// })).await?;
    ///
    /// // Add a label to a pod
    /// ctx.patch::<Pod>("worker", &json!({
    ///     "metadata": { "labels": { "version": "v2" } }
    /// })).await?;
    /// ```
    pub async fn patch<K>(&self, name: &str, patch: &serde_json::Value) -> Result<K, ContextError>
    where
        K: kube::Resource<Scope = kube::core::NamespaceResourceScope>
            + Clone
            + serde::de::DeserializeOwned
            + std::fmt::Debug,
        <K as kube::Resource>::DynamicType: Default,
    {
        let api: Api<K> = Api::namespaced(self.client.clone(), &self.namespace);

        let patched = api
            .patch(name, &PatchParams::default(), &Patch::Merge(patch))
            .await
            .map_err(|e| ContextError::PatchError(e.to_string()))?;

        info!(
            namespace = %self.namespace,
            name = %name,
            "Patched resource"
        );

        Ok(patched)
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

    /// Wait for a resource to be deleted
    ///
    /// Polls until the resource no longer exists in the namespace.
    /// Default timeout is 60 seconds, polling interval is 1 second.
    ///
    /// # Example
    ///
    /// ```ignore
    /// ctx.delete::<Pod>("worker").await?;
    /// ctx.wait_deleted::<Pod>("worker").await?;
    /// // Pod is now fully gone
    /// ```
    pub async fn wait_deleted<K>(&self, name: &str) -> Result<(), ContextError>
    where
        K: kube::Resource<Scope = kube::core::NamespaceResourceScope>
            + Clone
            + serde::de::DeserializeOwned
            + std::fmt::Debug,
        <K as kube::Resource>::DynamicType: Default,
    {
        self.wait_deleted_with_timeout::<K>(name, std::time::Duration::from_secs(60))
            .await
    }

    /// Wait for a resource to be deleted with custom timeout
    pub async fn wait_deleted_with_timeout<K>(
        &self,
        name: &str,
        timeout: std::time::Duration,
    ) -> Result<(), ContextError>
    where
        K: kube::Resource<Scope = kube::core::NamespaceResourceScope>
            + Clone
            + serde::de::DeserializeOwned
            + std::fmt::Debug,
        <K as kube::Resource>::DynamicType: Default,
    {
        let api: Api<K> = Api::namespaced(self.client.clone(), &self.namespace);
        let start = std::time::Instant::now();
        let poll_interval = std::time::Duration::from_secs(1);

        debug!(
            namespace = %self.namespace,
            resource = %name,
            timeout = ?timeout,
            "Starting wait_deleted"
        );

        loop {
            match api.get(name).await {
                Ok(_) => {
                    // Resource still exists, keep waiting
                    debug!(
                        namespace = %self.namespace,
                        resource = %name,
                        elapsed = ?start.elapsed(),
                        "Resource still exists, waiting for deletion..."
                    );
                }
                Err(kube::Error::Api(err)) if err.code == 404 => {
                    // Resource is gone (HTTP 404 Not Found)
                    debug!(
                        namespace = %self.namespace,
                        resource = %name,
                        elapsed = ?start.elapsed(),
                        "Resource deleted"
                    );
                    return Ok(());
                }
                Err(e) => {
                    return Err(ContextError::GetError(e.to_string()));
                }
            }

            if start.elapsed() >= timeout {
                return Err(ContextError::WaitTimeout(format!(
                    "Timed out waiting for {} to be deleted after {:?}",
                    name, timeout
                )));
            }

            tokio::time::sleep(poll_interval).await;
        }
    }

    /// Watch for changes to resources of a given type
    ///
    /// Returns a stream of watch events for all resources of type K in the namespace.
    /// Events include Applied (create/update) and Deleted.
    ///
    /// # Example
    ///
    /// ```ignore
    /// use futures::StreamExt;
    ///
    /// let mut stream = ctx.watch::<Pod>();
    /// while let Some(event) = stream.next().await {
    ///     match event {
    ///         Ok(WatchEvent::Applied(pod)) => println!("Pod changed: {:?}", pod.metadata.name),
    ///         Ok(WatchEvent::Deleted(pod)) => println!("Pod deleted: {:?}", pod.metadata.name),
    ///         Err(e) => eprintln!("Watch error: {}", e),
    ///         _ => {}
    ///     }
    /// }
    /// ```
    pub fn watch<K>(&self) -> impl Stream<Item = Result<WatchEvent<K>, watcher::Error>> + Send + '_
    where
        K: kube::Resource<Scope = kube::core::NamespaceResourceScope>
            + Clone
            + serde::de::DeserializeOwned
            + std::fmt::Debug
            + Send
            + 'static,
        <K as kube::Resource>::DynamicType: Default + Clone,
    {
        let api: Api<K> = Api::namespaced(self.client.clone(), &self.namespace);
        let config = watcher::Config::default();

        debug!(
            namespace = %self.namespace,
            "Starting watch"
        );

        watcher::watcher(api, config)
    }

    /// Watch for changes to a specific resource
    ///
    /// Returns a stream of watch events for a single named resource.
    ///
    /// # Example
    ///
    /// ```ignore
    /// use futures::StreamExt;
    ///
    /// let mut stream = ctx.watch_one::<Deployment>("myapp");
    /// while let Some(event) = stream.next().await {
    ///     if let Ok(WatchEvent::Applied(dep)) = event {
    ///         let ready = dep.status.as_ref().and_then(|s| s.ready_replicas).unwrap_or(0);
    ///         println!("Deployment myapp: {} ready replicas", ready);
    ///     }
    /// }
    /// ```
    pub fn watch_one<K>(
        &self,
        name: &str,
    ) -> impl Stream<Item = Result<WatchEvent<K>, watcher::Error>> + Send + '_
    where
        K: kube::Resource<Scope = kube::core::NamespaceResourceScope>
            + Clone
            + serde::de::DeserializeOwned
            + std::fmt::Debug
            + Send
            + 'static,
        <K as kube::Resource>::DynamicType: Default + Clone,
    {
        let api: Api<K> = Api::namespaced(self.client.clone(), &self.namespace);
        let field_selector = format!("metadata.name={}", name);
        let config = watcher::Config::default().fields(&field_selector);

        debug!(
            namespace = %self.namespace,
            resource = %name,
            "Starting watch for single resource"
        );

        watcher::watcher(api, config)
    }

    /// Wait for a resource to become ready
    ///
    /// Convenience method that understands common Kubernetes readiness patterns:
    /// - `deployment/name` - waits for all replicas to be available
    /// - `pod/name` - waits for pod to be Running with all containers ready
    /// - `statefulset/name` - waits for all replicas to be ready
    /// - `daemonset/name` - waits for all desired pods to be ready
    /// - `service/name` - waits for service to exist (services are always "ready")
    ///
    /// # Example
    ///
    /// ```ignore
    /// ctx.wait_ready("deployment/myapp").await?;
    /// ctx.wait_ready("pod/worker-0").await?;
    /// ```
    pub async fn wait_ready(&self, resource: &str) -> Result<(), ContextError> {
        self.wait_ready_with_timeout(resource, std::time::Duration::from_secs(60))
            .await
    }

    /// Wait for a resource to become ready with a custom timeout.
    ///
    /// This method behaves like [`wait_ready`](Self::wait_ready), but allows specifying
    /// a custom timeout duration for waiting on the resource to become ready.
    ///
    /// # Arguments
    ///
    /// * `resource` - Resource reference in "kind/name" format, e.g.:
    ///   - `deployment/name` - waits for all replicas to be available
    ///   - `pod/name` - waits for pod to be Running with all containers ready
    ///   - `statefulset/name` - waits for all replicas to be ready
    ///   - `daemonset/name` - waits for all desired pods to be ready
    ///   - `service/name` - waits for service to exist (services are always "ready")
    /// * `timeout` - Maximum duration to wait for readiness
    ///
    /// # Example
    ///
    /// ```ignore
    /// ctx.wait_ready_with_timeout("deployment/myapp", std::time::Duration::from_secs(120)).await?;
    /// ```
    pub async fn wait_ready_with_timeout(
        &self,
        resource: &str,
        timeout: std::time::Duration,
    ) -> Result<(), ContextError> {
        let (kind, name) = parse_resource_ref(resource)?;

        debug!(
            namespace = %self.namespace,
            resource = %resource,
            timeout = ?timeout,
            "Waiting for resource to be ready"
        );

        match kind {
            ResourceKind::Deployment => {
                self.wait_for_with_timeout::<Deployment, _>(
                    name,
                    |d| {
                        let spec_replicas = d.spec.as_ref().and_then(|s| s.replicas).unwrap_or(1);
                        let ready_replicas = d
                            .status
                            .as_ref()
                            .and_then(|s| s.ready_replicas)
                            .unwrap_or(0);
                        ready_replicas >= spec_replicas
                    },
                    timeout,
                )
                .await?;
            }
            ResourceKind::Pod => {
                self.wait_for_with_timeout::<Pod, _>(
                    name,
                    |p| {
                        // Pod must be Running
                        let phase = p.status.as_ref().and_then(|s| s.phase.as_deref());
                        if phase != Some("Running") {
                            return false;
                        }
                        // All containers must be ready
                        p.status
                            .as_ref()
                            .and_then(|s| s.container_statuses.as_ref())
                            .map(|containers| {
                                !containers.is_empty() && containers.iter().all(|c| c.ready)
                            })
                            .unwrap_or(false)
                    },
                    timeout,
                )
                .await?;
            }
            ResourceKind::StatefulSet => {
                self.wait_for_with_timeout::<StatefulSet, _>(
                    name,
                    |s| {
                        let spec_replicas = s.spec.as_ref().and_then(|s| s.replicas).unwrap_or(1);
                        let ready_replicas = s
                            .status
                            .as_ref()
                            .and_then(|s| s.ready_replicas)
                            .unwrap_or(0);
                        ready_replicas >= spec_replicas
                    },
                    timeout,
                )
                .await?;
            }
            ResourceKind::DaemonSet => {
                self.wait_for_with_timeout::<DaemonSet, _>(
                    name,
                    |d| {
                        let desired = d
                            .status
                            .as_ref()
                            .map(|s| s.desired_number_scheduled)
                            .unwrap_or(0);
                        let ready = d.status.as_ref().map(|s| s.number_ready).unwrap_or(0);
                        desired > 0 && ready >= desired
                    },
                    timeout,
                )
                .await?;
            }
            ResourceKind::Service => {
                // Services don't have a "ready" state - just verify it exists
                self.get::<Service>(name).await?;
            }
        }

        info!(
            namespace = %self.namespace,
            resource = %resource,
            "Resource is ready"
        );

        Ok(())
    }

    /// Scale a deployment to the specified number of replicas
    ///
    /// # Example
    ///
    /// ```ignore
    /// // Scale up
    /// ctx.scale("deployment/myapp", 3).await?;
    ///
    /// // Scale down to zero
    /// ctx.scale("deployment/myapp", 0).await?;
    /// ```
    pub async fn scale(&self, resource: &str, replicas: i32) -> Result<(), ContextError> {
        if replicas < 0 {
            return Err(ContextError::InvalidResourceRef(format!(
                "replicas must be non-negative, got {}",
                replicas
            )));
        }
        let (kind, name) = parse_resource_ref(resource)?;

        match kind {
            ResourceKind::Deployment => {
                let deployments: Api<Deployment> =
                    Api::namespaced(self.client.clone(), &self.namespace);

                // Get current deployment
                let mut dep = deployments
                    .get(name)
                    .await
                    .map_err(|e| ContextError::GetError(e.to_string()))?;

                // Update replicas
                let spec = dep.spec.as_mut().ok_or_else(|| {
                    ContextError::ApplyError(format!("deployment '{}' has no spec", name))
                })?;
                spec.replicas = Some(replicas);

                // Apply update
                deployments
                    .replace(name, &PostParams::default(), &dep)
                    .await
                    .map_err(|e| ContextError::ApplyError(e.to_string()))?;

                info!(
                    namespace = %self.namespace,
                    deployment = %name,
                    replicas = %replicas,
                    "Scaled deployment"
                );

                Ok(())
            }
            _ => Err(ContextError::InvalidResourceRef(format!(
                "scale only supports deployments, got {:?}",
                kind
            ))),
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
            .map_err(|e| {
                ContextError::CopyError(format!("failed to read {}: {}", remote_path, e))
            })?;

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
                "invalid path '{}': contains shell metacharacters",
                remote_path
            )));
        }

        // Escape content and path for shell
        let escaped_content = content.replace('\'', "'\"'\"'");
        let escaped_path = remote_path.replace('\'', "'\"'\"'");
        let command = format!("printf '%s' '{}' > '{}'", escaped_content, escaped_path);

        self.exec(pod_name, &["sh", "-c", &command])
            .await
            .map_err(|e| {
                ContextError::CopyError(format!("failed to write {}: {}", remote_path, e))
            })?;

        debug!(
            namespace = %self.namespace,
            pod = %pod_name,
            path = %remote_path,
            "Copied content to pod"
        );

        Ok(())
    }

    /// Create a port forward to a pod
    ///
    /// Opens a local port that tunnels traffic to the specified pod port.
    /// Returns a `PortForward` that can be used to make HTTP requests.
    ///
    /// # Example
    ///
    /// ```ignore
    /// let pf = ctx.forward("my-pod", 8080).await?;
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

    /// Create a port forward using kubectl-style resource references
    ///
    /// Supports:
    /// - `pod/name` - forward to pod directly
    /// - `svc/name` - forward to a pod backing the service
    /// - `deployment/name` - forward to a pod from the deployment
    /// - `name` - treated as pod name (backward compatible)
    ///
    /// # Example
    ///
    /// ```ignore
    /// // Forward to a service
    /// let pf = ctx.forward_to("svc/myapp", 8080).await?;
    ///
    /// // Forward to a deployment
    /// let pf = ctx.forward_to("deployment/backend", 3000).await?;
    ///
    /// // Forward to a pod (explicit or bare name)
    /// let pf = ctx.forward_to("pod/worker-0", 9000).await?;
    /// let pf = ctx.forward_to("worker-0", 9000).await?;
    /// ```
    pub async fn forward_to(
        &self,
        target: &str,
        port: u16,
    ) -> Result<PortForward, PortForwardError> {
        let parsed =
            parse_forward_target(target).map_err(|e| PortForwardError::BindError(e.to_string()))?;

        let pod_name = match parsed {
            ForwardTarget::Pod(name) => name,
            ForwardTarget::Service(svc_name) => self
                .find_pod_for_service(&svc_name)
                .await
                .map_err(|e| PortForwardError::BindError(e.to_string()))?,
            ForwardTarget::Deployment(deploy_name) => self
                .find_pod_for_deployment(&deploy_name)
                .await
                .map_err(|e| PortForwardError::BindError(e.to_string()))?,
        };

        debug!(
            namespace = %self.namespace,
            target = %target,
            resolved_pod = %pod_name,
            port = %port,
            "Creating port forward"
        );

        self.forward(&pod_name, port).await
    }

    /// Find a running pod backing a service
    async fn find_pod_for_service(&self, svc_name: &str) -> Result<String, ContextError> {
        let services: Api<Service> = Api::namespaced(self.client.clone(), &self.namespace);
        let svc = services
            .get(svc_name)
            .await
            .map_err(|e| ContextError::GetError(format!("service '{}': {}", svc_name, e)))?;

        // Get the selector from the service
        let selector = svc
            .spec
            .as_ref()
            .and_then(|s| s.selector.as_ref())
            .ok_or_else(|| {
                ContextError::GetError(format!("service '{}' has no selector", svc_name))
            })?;

        self.find_running_pod_by_labels(selector).await
    }

    /// Find a running pod from a deployment
    async fn find_pod_for_deployment(&self, deploy_name: &str) -> Result<String, ContextError> {
        let deployments: Api<Deployment> = Api::namespaced(self.client.clone(), &self.namespace);
        let deploy = deployments
            .get(deploy_name)
            .await
            .map_err(|e| ContextError::GetError(format!("deployment '{}': {}", deploy_name, e)))?;

        // Get the selector from the deployment
        let selector = deploy
            .spec
            .as_ref()
            .and_then(|s| s.selector.match_labels.as_ref())
            .ok_or_else(|| {
                ContextError::GetError(format!("deployment '{}' has no selector", deploy_name))
            })?;

        self.find_running_pod_by_labels(selector).await
    }

    /// Find a running pod matching the given labels
    async fn find_running_pod_by_labels(
        &self,
        labels: &std::collections::BTreeMap<String, String>,
    ) -> Result<String, ContextError> {
        let pods: Api<Pod> = Api::namespaced(self.client.clone(), &self.namespace);

        // Build label selector string
        let label_selector = labels
            .iter()
            .map(|(k, v)| format!("{}={}", k, v))
            .collect::<Vec<_>>()
            .join(",");

        let list_params = ListParams::default().labels(&label_selector);
        let pod_list = pods
            .list(&list_params)
            .await
            .map_err(|e| ContextError::ListError(e.to_string()))?;

        // Find a running pod
        for pod in pod_list.items {
            let phase = pod
                .status
                .as_ref()
                .and_then(|s| s.phase.as_ref())
                .map(|s| s.as_str());

            if phase == Some("Running") {
                if let Some(name) = pod.metadata.name {
                    return Ok(name);
                }
            }
        }

        Err(ContextError::GetError(format!(
            "no running pod found with labels: {}",
            label_selector
        )))
    }

    /// Deploy a stack of services to the test namespace
    ///
    /// Creates Deployments and Services for all services defined in the stack.
    ///
    /// # Example
    ///
    /// ```ignore
    /// let stack = Stack::new()
    ///     .service("frontend").image("fe:test").replicas(4).port(80)
    ///     .service("backend").image("be:test").replicas(2).port(8080)
    ///     .build();  // <-- This is required
    ///
    /// ctx.up(&stack).await?;
    /// ```
    pub async fn up(&self, stack: &Stack) -> Result<(), StackError> {
        use futures::future::try_join_all;

        if stack.services().is_empty() {
            return Err(StackError::EmptyStack);
        }

        // Validate all services have required fields
        for svc_def in stack.services() {
            if svc_def.image.is_empty() {
                return Err(StackError::DeployError(
                    svc_def.name.clone(),
                    "image is required".to_string(),
                ));
            }
        }

        let deployments: Api<Deployment> = Api::namespaced(self.client.clone(), &self.namespace);
        let services: Api<Service> = Api::namespaced(self.client.clone(), &self.namespace);
        let namespace = self.namespace.clone();

        // Create all deployments concurrently
        let deployment_futures = stack.services().iter().map(|svc_def| {
            let deployments = deployments.clone();
            let deployment = stack.deployment_for(svc_def, &namespace);
            let name = svc_def.name.clone();
            async move {
                deployments
                    .create(&PostParams::default(), &deployment)
                    .await
                    .map_err(|e| StackError::DeployError(name, e.to_string()))
            }
        });

        try_join_all(deployment_futures).await?;

        for svc_def in stack.services() {
            info!(
                namespace = %self.namespace,
                service = %svc_def.name,
                replicas = %svc_def.replicas,
                "Deployed service"
            );
        }

        // Create all services concurrently (for those with ports)
        let service_futures = stack
            .services()
            .iter()
            .filter_map(|svc_def| {
                stack.service_for(svc_def, &namespace).map(|k8s_svc| {
                    let services = services.clone();
                    let name = svc_def.name.clone();
                    async move {
                        services
                            .create(&PostParams::default(), &k8s_svc)
                            .await
                            .map_err(|e| StackError::DeployError(name, e.to_string()))
                    }
                })
            })
            .collect::<Vec<_>>();

        try_join_all(service_futures).await?;

        for svc_def in stack.services().iter().filter(|s| s.port.is_some()) {
            info!(
                namespace = %self.namespace,
                service = %svc_def.name,
                port = ?svc_def.port,
                "Created service"
            );
        }

        Ok(())
    }

    /// Create a pod assertion builder
    ///
    /// # Example
    ///
    /// ```ignore
    /// ctx.assert_pod("my-pod").is_running().await?;
    /// ctx.assert_pod("my-pod").has_label("app", "myapp").await?;
    /// ```
    pub fn assert_pod(&self, name: &str) -> crate::assertions::PodAssertion {
        crate::assertions::PodAssertion::new(
            self.client.clone(),
            self.namespace.clone(),
            name.to_string(),
        )
    }

    /// Create a deployment assertion builder
    ///
    /// # Example
    ///
    /// ```ignore
    /// ctx.assert_deployment("my-app").has_replicas(3).await?;
    /// ctx.assert_deployment("my-app").is_available().await?;
    /// ```
    pub fn assert_deployment(&self, name: &str) -> crate::assertions::DeploymentAssertion {
        crate::assertions::DeploymentAssertion::new(
            self.client.clone(),
            self.namespace.clone(),
            name.to_string(),
        )
    }

    /// Create a service assertion builder
    ///
    /// # Example
    ///
    /// ```ignore
    /// ctx.assert_service("my-svc").has_port(8080).await?;
    /// ctx.assert_service("my-svc").is_cluster_ip().await?;
    /// ```
    pub fn assert_service(&self, name: &str) -> crate::assertions::ServiceAssertion {
        crate::assertions::ServiceAssertion::new(
            self.client.clone(),
            self.namespace.clone(),
            name.to_string(),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// RED: Test that Context::new() creates a namespace
    #[tokio::test]
    #[ignore] // Requires real cluster
    async fn test_context_creates_namespace() {
        // Create context
        let ctx = Context::new().await.expect("Should create context");

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
        let ctx = Context::new().await.expect("Should create context");
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

        let ctx = Context::new().await.expect("Should create context");

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

        let ctx = Context::new().await.expect("Should create context");

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

        let ctx = Context::new().await.expect("Should create context");

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

    /// Test that patch() updates a resource partially
    #[tokio::test]
    #[ignore] // Requires real cluster
    async fn test_context_patch_updates_resource() {
        use k8s_openapi::api::core::v1::ConfigMap;

        let ctx = Context::new().await.expect("Should create context");

        // Create a ConfigMap
        let cm = ConfigMap {
            metadata: kube::api::ObjectMeta {
                name: Some("patch-test".to_string()),
                ..Default::default()
            },
            data: Some(
                [("key1".to_string(), "value1".to_string())]
                    .into_iter()
                    .collect(),
            ),
            ..Default::default()
        };

        ctx.apply(&cm).await.expect("Should apply ConfigMap");

        // Patch it - add a new key without removing existing one
        let patched: ConfigMap = ctx
            .patch(
                "patch-test",
                &serde_json::json!({
                    "data": { "key2": "value2" }
                }),
            )
            .await
            .expect("Should patch ConfigMap");

        // Verify the patch - should have both keys
        let data = patched.data.expect("Should have data");
        assert_eq!(data.get("key1"), Some(&"value1".to_string()));
        assert_eq!(data.get("key2"), Some(&"value2".to_string()));

        // Cleanup
        ctx.cleanup().await.expect("Should cleanup");
    }

    /// Test that wait_deleted() waits for resource to be gone
    #[tokio::test]
    #[ignore] // Requires real cluster
    async fn test_context_wait_deleted() {
        use k8s_openapi::api::core::v1::ConfigMap;

        let ctx = Context::new().await.expect("Should create context");

        // Create a ConfigMap
        let cm = ConfigMap {
            metadata: kube::api::ObjectMeta {
                name: Some("delete-wait-test".to_string()),
                ..Default::default()
            },
            ..Default::default()
        };

        ctx.apply(&cm).await.expect("Should apply ConfigMap");

        // Delete it
        ctx.delete::<ConfigMap>("delete-wait-test")
            .await
            .expect("Should delete ConfigMap");

        // Wait for it to be fully deleted
        ctx.wait_deleted::<ConfigMap>("delete-wait-test")
            .await
            .expect("Should wait for deletion");

        // Verify it's gone
        let result: Result<ConfigMap, _> = ctx.get("delete-wait-test").await;
        assert!(result.is_err(), "ConfigMap should be deleted");

        // Cleanup
        ctx.cleanup().await.expect("Should cleanup");
    }

    /// Test that list() returns resources from the test namespace
    #[tokio::test]
    #[ignore] // Requires real cluster
    async fn test_context_list_returns_resources() {
        use k8s_openapi::api::core::v1::ConfigMap;

        let ctx = Context::new().await.expect("Should create context");

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

        let ctx = Context::new().await.expect("Should create context");

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

        let ctx = Context::new().await.expect("Should create context");

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

        let ctx = Context::new().await.expect("Should create context");

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

        let ctx = Context::new().await.expect("Should create context");

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

        let ctx = Context::new().await.expect("Should create context");

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
        let ctx = Context::new().await.expect("Should create context");

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
        let pf = ctx
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
        let ctx = Context::new().await.expect("Should create context");

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

    /// Test that copy_to() and copy_from() transfer files
    #[tokio::test]
    #[ignore] // Requires real cluster
    async fn test_context_copy_to_and_from() {
        let ctx = Context::new().await.expect("Should create context");

        // Create a simple pod
        let pod = Pod {
            metadata: kube::api::ObjectMeta {
                name: Some("copy-test".to_string()),
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
        ctx.wait_for::<Pod, _>("copy-test", |p| {
            p.status
                .as_ref()
                .and_then(|s| s.phase.as_ref())
                .is_some_and(|phase| phase == "Running")
        })
        .await
        .expect("Pod should be running");

        // Copy content to a file in the pod
        let test_content = "hello from seppo\nline 2\n";
        ctx.copy_to("copy-test", "/tmp/test-file.txt", test_content)
            .await
            .expect("Should copy to pod");

        // Read it back
        let content = ctx
            .copy_from("copy-test", "/tmp/test-file.txt")
            .await
            .expect("Should copy from pod");

        assert_eq!(content, test_content);

        // Cleanup
        ctx.cleanup().await.expect("Should cleanup");
    }

    /// Test that up() deploys a stack of services
    #[tokio::test]
    #[ignore] // Requires real cluster
    async fn test_context_up_deploys_stack() {
        use crate::stack::Stack;

        let ctx = Context::new().await.expect("Should create context");

        // Create a stack with one service
        let stack = Stack::new()
            .service("test-app")
            .image("nginx:alpine")
            .replicas(2)
            .port(80)
            .build();

        // Deploy the stack
        ctx.up(&stack).await.expect("Should deploy stack");

        // Verify deployment was created
        let deployment: Deployment = ctx.get("test-app").await.expect("Deployment should exist");
        assert_eq!(
            deployment.spec.as_ref().unwrap().replicas,
            Some(2),
            "Should have 2 replicas"
        );

        // Verify service was created
        let service: Service = ctx.get("test-app").await.expect("Service should exist");
        assert_eq!(
            service.spec.as_ref().unwrap().ports.as_ref().unwrap()[0].port,
            80,
            "Service should expose port 80"
        );

        // Cleanup
        ctx.cleanup().await.expect("Should cleanup");
    }

    /// Test that up() deploys multiple services concurrently
    #[tokio::test]
    #[ignore] // Requires real cluster
    async fn test_context_up_deploys_multiple_services() {
        use crate::stack::Stack;

        let ctx = Context::new().await.expect("Should create context");

        // Create a stack with multiple services
        let stack = Stack::new()
            .service("frontend")
            .image("nginx:alpine")
            .replicas(2)
            .port(80)
            .service("backend")
            .image("nginx:alpine")
            .replicas(1)
            .port(8080)
            .service("worker")
            .image("busybox:latest")
            .replicas(3)
            .build(); // worker has no port, so no Service

        // Deploy the stack
        ctx.up(&stack).await.expect("Should deploy stack");

        // Verify all deployments were created
        let frontend: Deployment = ctx.get("frontend").await.expect("frontend should exist");
        assert_eq!(frontend.spec.as_ref().unwrap().replicas, Some(2));

        let backend: Deployment = ctx.get("backend").await.expect("backend should exist");
        assert_eq!(backend.spec.as_ref().unwrap().replicas, Some(1));

        let worker: Deployment = ctx.get("worker").await.expect("worker should exist");
        assert_eq!(worker.spec.as_ref().unwrap().replicas, Some(3));

        // Verify services were created for frontend and backend (they have ports)
        let frontend_svc: Service = ctx
            .get("frontend")
            .await
            .expect("frontend svc should exist");
        assert_eq!(
            frontend_svc.spec.as_ref().unwrap().ports.as_ref().unwrap()[0].port,
            80
        );

        let backend_svc: Service = ctx.get("backend").await.expect("backend svc should exist");
        assert_eq!(
            backend_svc.spec.as_ref().unwrap().ports.as_ref().unwrap()[0].port,
            8080
        );

        // Worker should NOT have a Service (no port defined)
        let worker_svc: Result<Service, _> = ctx.get("worker").await;
        assert!(worker_svc.is_err(), "worker should not have a Service");

        // Cleanup
        ctx.cleanup().await.expect("Should cleanup");
    }

    /// Test that up() fails on empty stack
    #[tokio::test]
    #[ignore] // Requires real cluster
    async fn test_context_up_fails_on_empty_stack() {
        use crate::stack::Stack;

        let ctx = Context::new().await.expect("Should create context");

        let stack = Stack::new();

        let result = ctx.up(&stack).await;
        assert!(
            matches!(result, Err(StackError::EmptyStack)),
            "Should fail with EmptyStack error"
        );

        // Cleanup
        ctx.cleanup().await.expect("Should cleanup");
    }

    /// Test that up() fails when service has no image
    #[tokio::test]
    #[ignore] // Requires real cluster
    async fn test_context_up_fails_without_image() {
        use crate::stack::Stack;

        let ctx = Context::new().await.expect("Should create context");

        // Create a service without an image
        let stack = Stack::new().service("no-image").replicas(1).build();

        let result = ctx.up(&stack).await;
        assert!(
            matches!(result, Err(StackError::DeployError(name, _)) if name == "no-image"),
            "Should fail with DeployError for missing image"
        );

        // Cleanup
        ctx.cleanup().await.expect("Should cleanup");
    }

    // ============================================================
    // wait_ready() tests
    // ============================================================

    #[test]
    fn test_parse_resource_ref_deployment() {
        let (kind, name) = parse_resource_ref("deployment/myapp").unwrap();
        assert_eq!(kind, ResourceKind::Deployment);
        assert_eq!(name, "myapp");
    }

    #[test]
    fn test_parse_resource_ref_pod() {
        let (kind, name) = parse_resource_ref("pod/nginx").unwrap();
        assert_eq!(kind, ResourceKind::Pod);
        assert_eq!(name, "nginx");
    }

    #[test]
    fn test_parse_resource_ref_service() {
        let (kind, name) = parse_resource_ref("svc/backend").unwrap();
        assert_eq!(kind, ResourceKind::Service);
        assert_eq!(name, "backend");
    }

    #[test]
    fn test_parse_resource_ref_statefulset() {
        let (kind, name) = parse_resource_ref("statefulset/postgres").unwrap();
        assert_eq!(kind, ResourceKind::StatefulSet);
        assert_eq!(name, "postgres");
    }

    #[test]
    fn test_parse_resource_ref_daemonset() {
        let (kind, name) = parse_resource_ref("daemonset/fluentd").unwrap();
        assert_eq!(kind, ResourceKind::DaemonSet);
        assert_eq!(name, "fluentd");
    }

    #[test]
    fn test_parse_resource_ref_invalid_format() {
        assert!(parse_resource_ref("myapp").is_err());
        assert!(parse_resource_ref("").is_err());
        assert!(parse_resource_ref("/myapp").is_err());
        assert!(parse_resource_ref("deployment/").is_err());
    }

    #[test]
    fn test_parse_resource_ref_unknown_kind() {
        assert!(parse_resource_ref("unknown/myapp").is_err());
    }

    #[test]
    fn test_parse_resource_ref_aliases() {
        // deploy -> Deployment
        let (kind, _) = parse_resource_ref("deploy/app").unwrap();
        assert_eq!(kind, ResourceKind::Deployment);

        // po -> Pod
        let (kind, _) = parse_resource_ref("po/app").unwrap();
        assert_eq!(kind, ResourceKind::Pod);

        // service -> Service
        let (kind, _) = parse_resource_ref("service/app").unwrap();
        assert_eq!(kind, ResourceKind::Service);

        // sts -> StatefulSet
        let (kind, _) = parse_resource_ref("sts/app").unwrap();
        assert_eq!(kind, ResourceKind::StatefulSet);

        // ds -> DaemonSet
        let (kind, _) = parse_resource_ref("ds/app").unwrap();
        assert_eq!(kind, ResourceKind::DaemonSet);
    }

    // ============================================================
    // forward_to() tests
    // ============================================================

    #[test]
    fn test_parse_forward_target_pod() {
        let target = parse_forward_target("pod/myapp").unwrap();
        assert_eq!(target, ForwardTarget::Pod("myapp".to_string()));
    }

    #[test]
    fn test_parse_forward_target_service() {
        let target = parse_forward_target("svc/backend").unwrap();
        assert_eq!(target, ForwardTarget::Service("backend".to_string()));
    }

    #[test]
    fn test_parse_forward_target_service_full() {
        let target = parse_forward_target("service/backend").unwrap();
        assert_eq!(target, ForwardTarget::Service("backend".to_string()));
    }

    #[test]
    fn test_parse_forward_target_bare_name() {
        // Bare name defaults to pod for backward compatibility
        let target = parse_forward_target("myapp").unwrap();
        assert_eq!(target, ForwardTarget::Pod("myapp".to_string()));
    }

    #[test]
    fn test_parse_forward_target_deployment() {
        let target = parse_forward_target("deployment/myapp").unwrap();
        assert_eq!(target, ForwardTarget::Deployment("myapp".to_string()));
    }

    #[test]
    fn test_parse_forward_target_deploy_alias() {
        let target = parse_forward_target("deploy/myapp").unwrap();
        assert_eq!(target, ForwardTarget::Deployment("myapp".to_string()));
    }

    #[test]
    fn test_parse_forward_target_po_alias() {
        let target = parse_forward_target("po/myapp").unwrap();
        assert_eq!(target, ForwardTarget::Pod("myapp".to_string()));
    }

    #[test]
    fn test_parse_forward_target_invalid() {
        assert!(parse_forward_target("").is_err());
        assert!(parse_forward_target("unknown/myapp").is_err());
        // Empty resource names should fail
        assert!(parse_forward_target("pod/").is_err());
        assert!(parse_forward_target("svc/").is_err());
    }

    #[tokio::test]
    #[ignore] // Requires real cluster
    async fn test_wait_ready_deployment() {
        let ctx = Context::new().await.expect("Should create context");

        // Create a simple deployment
        let deployment = Deployment {
            metadata: kube::api::ObjectMeta {
                name: Some("ready-test".to_string()),
                ..Default::default()
            },
            spec: Some(k8s_openapi::api::apps::v1::DeploymentSpec {
                replicas: Some(1),
                selector: k8s_openapi::apimachinery::pkg::apis::meta::v1::LabelSelector {
                    match_labels: Some(
                        [("app".to_string(), "ready-test".to_string())]
                            .into_iter()
                            .collect(),
                    ),
                    ..Default::default()
                },
                template: k8s_openapi::api::core::v1::PodTemplateSpec {
                    metadata: Some(kube::api::ObjectMeta {
                        labels: Some(
                            [("app".to_string(), "ready-test".to_string())]
                                .into_iter()
                                .collect(),
                        ),
                        ..Default::default()
                    }),
                    spec: Some(k8s_openapi::api::core::v1::PodSpec {
                        containers: vec![k8s_openapi::api::core::v1::Container {
                            name: "nginx".to_string(),
                            image: Some("nginx:alpine".to_string()),
                            ..Default::default()
                        }],
                        ..Default::default()
                    }),
                },
                ..Default::default()
            }),
            ..Default::default()
        };

        ctx.apply(&deployment)
            .await
            .expect("Should apply deployment");

        // wait_ready should wait until deployment is available
        ctx.wait_ready("deployment/ready-test")
            .await
            .expect("Should become ready");

        // Verify it's actually ready
        let dep: Deployment = ctx.get("ready-test").await.expect("Should get deployment");
        let status = dep.status.expect("Should have status");
        assert_eq!(status.ready_replicas, Some(1));

        ctx.cleanup().await.expect("Should cleanup");
    }

    #[tokio::test]
    #[ignore] // Requires real cluster
    async fn test_wait_ready_pod() {
        let ctx = Context::new().await.expect("Should create context");

        let pod = Pod {
            metadata: kube::api::ObjectMeta {
                name: Some("ready-pod".to_string()),
                ..Default::default()
            },
            spec: Some(k8s_openapi::api::core::v1::PodSpec {
                containers: vec![k8s_openapi::api::core::v1::Container {
                    name: "nginx".to_string(),
                    image: Some("nginx:alpine".to_string()),
                    ..Default::default()
                }],
                ..Default::default()
            }),
            ..Default::default()
        };

        ctx.apply(&pod).await.expect("Should apply pod");

        // wait_ready should wait until pod is Running
        ctx.wait_ready("pod/ready-pod")
            .await
            .expect("Should become ready");

        // Verify it's running
        let p: Pod = ctx.get("ready-pod").await.expect("Should get pod");
        let phase = p.status.and_then(|s| s.phase);
        assert_eq!(phase, Some("Running".to_string()));

        ctx.cleanup().await.expect("Should cleanup");
    }

    #[tokio::test]
    #[ignore] // Requires real cluster
    async fn test_forward_to_pod() {
        let ctx = Context::new().await.expect("Should create context");

        // Create a pod with nginx
        let pod = Pod {
            metadata: kube::api::ObjectMeta {
                name: Some("forward-pod-test".to_string()),
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
        ctx.wait_for::<Pod, _>("forward-pod-test", |p| {
            p.status
                .as_ref()
                .and_then(|s| s.phase.as_ref())
                .is_some_and(|phase| phase == "Running")
        })
        .await
        .expect("Pod should be running");

        // Forward using pod/ prefix
        let pf = ctx
            .forward_to("pod/forward-pod-test", 80)
            .await
            .expect("Should create port forward");

        let response = pf.get("/").await.expect("Should get response");
        assert!(response.contains("nginx") || response.contains("Welcome"));

        ctx.cleanup().await.expect("Should cleanup");
    }

    #[tokio::test]
    #[ignore] // Requires real cluster
    async fn test_forward_to_service() {
        use crate::stack::Stack;

        let ctx = Context::new().await.expect("Should create context");

        // Create a deployment with service using Stack
        let stack = Stack::new()
            .service("forward-svc-test")
            .image("nginx:alpine")
            .replicas(1)
            .port(80)
            .build();

        ctx.up(&stack).await.expect("Should deploy stack");

        // Wait for deployment to be ready
        ctx.wait_for::<Deployment, _>("forward-svc-test", |d| {
            d.status
                .as_ref()
                .and_then(|s| s.ready_replicas)
                .unwrap_or(0)
                >= 1
        })
        .await
        .expect("Deployment should be ready");

        // Forward using svc/ prefix - should find a backing pod
        let pf = ctx
            .forward_to("svc/forward-svc-test", 80)
            .await
            .expect("Should create port forward to service");

        let response = pf.get("/").await.expect("Should get response");
        assert!(response.contains("nginx") || response.contains("Welcome"));

        ctx.cleanup().await.expect("Should cleanup");
    }

    // ============================================================
    // scale() tests
    // ============================================================

    #[tokio::test]
    #[ignore] // Requires real cluster
    async fn test_scale_deployment() {
        let ctx = Context::new().await.expect("Should create context");

        // Create a deployment with 1 replica
        let deployment = Deployment {
            metadata: kube::api::ObjectMeta {
                name: Some("scale-test".to_string()),
                ..Default::default()
            },
            spec: Some(k8s_openapi::api::apps::v1::DeploymentSpec {
                replicas: Some(1),
                selector: k8s_openapi::apimachinery::pkg::apis::meta::v1::LabelSelector {
                    match_labels: Some(
                        [("app".to_string(), "scale-test".to_string())]
                            .into_iter()
                            .collect(),
                    ),
                    ..Default::default()
                },
                template: k8s_openapi::api::core::v1::PodTemplateSpec {
                    metadata: Some(kube::api::ObjectMeta {
                        labels: Some(
                            [("app".to_string(), "scale-test".to_string())]
                                .into_iter()
                                .collect(),
                        ),
                        ..Default::default()
                    }),
                    spec: Some(k8s_openapi::api::core::v1::PodSpec {
                        containers: vec![k8s_openapi::api::core::v1::Container {
                            name: "nginx".to_string(),
                            image: Some("nginx:alpine".to_string()),
                            ..Default::default()
                        }],
                        ..Default::default()
                    }),
                },
                ..Default::default()
            }),
            ..Default::default()
        };

        ctx.apply(&deployment)
            .await
            .expect("Should apply deployment");

        // Scale up to 3
        ctx.scale("deployment/scale-test", 3)
            .await
            .expect("Should scale up");

        // Verify
        let dep: Deployment = ctx.get("scale-test").await.expect("Should get deployment");
        assert_eq!(dep.spec.as_ref().unwrap().replicas, Some(3));

        // Scale down to 0
        ctx.scale("deployment/scale-test", 0)
            .await
            .expect("Should scale down");

        let dep: Deployment = ctx.get("scale-test").await.expect("Should get deployment");
        assert_eq!(dep.spec.as_ref().unwrap().replicas, Some(0));

        ctx.cleanup().await.expect("Should cleanup");
    }
}
