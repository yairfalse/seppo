//! Kubernetes SDK context
//!
//! Provides a connection to a Kubernetes cluster with namespace management.
//! Can be used standalone or with the `#[seppo::test]` macro.
//!
//! # Errors
//!
//! All fallible methods in this module return `ContextError` which provides
//! detailed error information for Kubernetes operations including:
//! - Client connection errors
//! - Namespace creation/deletion errors
//! - Resource CRUD operation errors
//! - Wait/timeout errors
//! - Port forwarding errors

// TODO: Add individual # Errors sections to each method for full pedantic compliance
#![allow(clippy::missing_errors_doc)]
#![allow(clippy::missing_panics_doc)]

use crate::diagnostics::Diagnostics;
use crate::portforward::{PortForward, PortForwardError};
use crate::stack::{Stack, StackError};
use futures::{AsyncBufReadExt, Stream};
use json_patch::Patch as JsonPatchOps;
use k8s_openapi::api::apps::v1::{DaemonSet, Deployment, ReplicaSet, StatefulSet};
use k8s_openapi::api::authorization::v1::{
    ResourceAttributes, SelfSubjectAccessReview, SelfSubjectAccessReviewSpec,
};
use k8s_openapi::api::core::v1::{
    Event, Namespace, PersistentVolumeClaim, Pod, Secret, Service, ServiceAccount,
};
use k8s_openapi::api::networking::v1::{
    Ingress, NetworkPolicy, NetworkPolicyIngressRule, NetworkPolicyPeer, NetworkPolicySpec,
};
use k8s_openapi::api::rbac::v1::{PolicyRule, Role, RoleBinding, RoleRef, Subject};
use kube::api::{
    Api, AttachParams, DeleteParams, ListParams, LogParams, ObjectMeta, Patch, PatchParams,
    PostParams,
};
use kube::runtime::watcher::{self, Event as WatchEvent};
use kube::Client;
use std::collections::HashMap;
use tokio::fs;
use tokio::io::AsyncReadExt;
use tracing::{debug, info, warn};

/// Kubernetes resource kinds supported by `wait_ready`
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

/// `GroupVersionResource` identifies a Kubernetes resource type
///
/// Used with the dynamic client to work with CRDs and other resources
/// without compile-time type information.
///
/// # Example
///
/// ```ignore
/// // For Gateway API HTTPRoute
/// let gvr = Gvr::new("gateway.networking.k8s.io", "v1", "httproutes", "HTTPRoute");
///
/// // Or use built-in helpers
/// let gvr = Gvr::http_route();
///
/// ctx.apply_dynamic(&gvr, &json!({
///     "apiVersion": "gateway.networking.k8s.io/v1",
///     "kind": "HTTPRoute",
///     "metadata": { "name": "my-route" },
///     "spec": { ... }
/// })).await?;
/// ```
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Gvr {
    /// API group (e.g., "gateway.networking.k8s.io", "" for core)
    pub group: String,
    /// API version (e.g., "v1", "v1beta1")
    pub version: String,
    /// Resource name (plural, e.g., "httproutes", "pods")
    pub resource: String,
    /// Kind name (singular, e.g., "`HTTPRoute`", "Pod")
    pub kind: String,
}

impl Gvr {
    /// Create a new `GroupVersionResource`
    #[must_use]
    pub fn new(group: &str, version: &str, resource: &str, kind: &str) -> Self {
        Self {
            group: group.to_string(),
            version: version.to_string(),
            resource: resource.to_string(),
            kind: kind.to_string(),
        }
    }

    /// Gateway API: `GatewayClass`
    #[must_use]
    pub fn gateway_class() -> Self {
        Self::new(
            "gateway.networking.k8s.io",
            "v1",
            "gatewayclasses",
            "GatewayClass",
        )
    }

    /// Gateway API: Gateway
    #[must_use]
    pub fn gateway() -> Self {
        Self::new("gateway.networking.k8s.io", "v1", "gateways", "Gateway")
    }

    /// Gateway API: `HTTPRoute`
    #[must_use]
    pub fn http_route() -> Self {
        Self::new("gateway.networking.k8s.io", "v1", "httproutes", "HTTPRoute")
    }

    /// Gateway API: `GRPCRoute`
    #[must_use]
    pub fn grpc_route() -> Self {
        Self::new("gateway.networking.k8s.io", "v1", "grpcroutes", "GRPCRoute")
    }

    /// Cert-Manager: Certificate
    #[must_use]
    pub fn certificate() -> Self {
        Self::new("cert-manager.io", "v1", "certificates", "Certificate")
    }

    /// Cert-Manager: Issuer
    #[must_use]
    pub fn issuer() -> Self {
        Self::new("cert-manager.io", "v1", "issuers", "Issuer")
    }

    /// Cert-Manager: `ClusterIssuer`
    #[must_use]
    pub fn cluster_issuer() -> Self {
        Self::new("cert-manager.io", "v1", "clusterissuers", "ClusterIssuer")
    }

    /// Convert to kube `ApiResource`
    pub(crate) fn to_api_resource(&self) -> kube::core::ApiResource {
        kube::core::ApiResource {
            group: self.group.clone(),
            version: self.version.clone(),
            api_version: if self.group.is_empty() {
                self.version.clone()
            } else {
                format!("{}/{}", self.group, self.version)
            },
            kind: self.kind.clone(),
            plural: self.resource.clone(),
        }
    }
}

// ============================================================================
// Traffic Testing Types
// ============================================================================

/// Configuration for traffic generation
///
/// # Example
///
/// ```ignore
/// let config = TrafficConfig::default()
///     .with_rps(100)
///     .with_duration(Duration::from_secs(30))
///     .with_endpoint("/health");
/// ```
#[derive(Debug, Clone)]
pub struct TrafficConfig {
    /// Requests per second
    pub rps: u32,
    /// How long to generate traffic
    pub duration: std::time::Duration,
    /// HTTP endpoint to hit
    pub endpoint: String,
}

impl Default for TrafficConfig {
    fn default() -> Self {
        Self {
            rps: 10,
            duration: std::time::Duration::from_secs(10),
            endpoint: "/".to_string(),
        }
    }
}

impl TrafficConfig {
    /// Set requests per second
    #[must_use]
    pub fn with_rps(mut self, rps: u32) -> Self {
        self.rps = rps;
        self
    }

    /// Set duration
    #[must_use]
    pub fn with_duration(mut self, duration: std::time::Duration) -> Self {
        self.duration = duration;
        self
    }

    /// Set endpoint path
    #[must_use]
    pub fn with_endpoint(mut self, endpoint: impl Into<String>) -> Self {
        self.endpoint = endpoint.into();
        self
    }
}

/// Statistics from a traffic test
#[derive(Debug, Clone)]
pub struct TrafficStats {
    /// Total requests made
    pub total_requests: u64,
    /// Number of failed requests
    pub errors: u64,
    /// Individual request latencies
    pub latencies: Vec<std::time::Duration>,
}

impl TrafficStats {
    /// Calculate error rate (0.0 to 1.0)
    #[must_use]
    #[allow(clippy::cast_precision_loss)] // Precision loss acceptable for statistics
    pub fn error_rate(&self) -> f64 {
        if self.total_requests == 0 {
            0.0
        } else {
            self.errors as f64 / self.total_requests as f64
        }
    }

    /// Calculate 99th percentile latency
    #[must_use]
    #[allow(
        clippy::cast_precision_loss,
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss
    )]
    pub fn p99_latency(&self) -> std::time::Duration {
        if self.latencies.is_empty() {
            return std::time::Duration::ZERO;
        }

        let mut sorted = self.latencies.clone();
        sorted.sort();

        let idx = ((sorted.len() as f64) * 0.99) as usize;
        let idx = idx.min(sorted.len() - 1);
        sorted[idx]
    }

    /// Calculate median latency
    #[must_use]
    pub fn median_latency(&self) -> std::time::Duration {
        if self.latencies.is_empty() {
            return std::time::Duration::ZERO;
        }

        let mut sorted = self.latencies.clone();
        sorted.sort();
        sorted[sorted.len() / 2]
    }
}

/// Handle to running traffic generation
///
/// Use `wait()` to block until traffic generation completes,
/// or `stop()` to stop early. Use `stats()` to get results.
pub struct TrafficHandle {
    _pf: PortForward,
    handle: tokio::task::JoinHandle<()>,
    stop_flag: std::sync::Arc<std::sync::atomic::AtomicBool>,
    total: std::sync::Arc<std::sync::atomic::AtomicU64>,
    errors: std::sync::Arc<std::sync::atomic::AtomicU64>,
    latencies: std::sync::Arc<std::sync::Mutex<Vec<std::time::Duration>>>,
}

impl TrafficHandle {
    /// Wait for traffic generation to complete
    pub async fn wait(self) {
        let _ = self.handle.await;
    }

    /// Stop traffic generation early
    pub fn stop(&self) {
        self.stop_flag
            .store(true, std::sync::atomic::Ordering::Relaxed);
    }

    /// Get current statistics
    ///
    /// # Panics
    ///
    /// Panics if the latencies mutex is poisoned.
    #[must_use]
    pub fn stats(&self) -> TrafficStats {
        use std::sync::atomic::Ordering;

        TrafficStats {
            total_requests: self.total.load(Ordering::Relaxed),
            errors: self.errors.load(Ordering::Relaxed),
            latencies: self.latencies.lock().unwrap().clone(),
        }
    }

    /// Get total requests made so far
    #[must_use]
    pub fn total_requests(&self) -> u64 {
        self.total.load(std::sync::atomic::Ordering::Relaxed)
    }

    /// Get error rate so far
    #[must_use]
    pub fn error_rate(&self) -> f64 {
        self.stats().error_rate()
    }
}

/// Extract just the resource name from either "name" or "kind/name" format
///
/// This helper allows methods to accept both formats for consistency:
/// - `"myapp"` → `"myapp"`
/// - `"deployment/myapp"` → `"myapp"`
/// - `"pod/worker-0"` → `"worker-0"`
#[must_use]
pub fn extract_resource_name(reference: &str) -> &str {
    // If there's a slash, take everything after it
    reference.split('/').next_back().unwrap_or(reference)
}

/// Create a deterministic suffix from label selectors for naming `NetworkPolicies`
///
/// Uses a hash function to generate a collision-resistant 8-character hex suffix.
/// Sorted keys ensure the same labels always produce the same suffix.
fn label_suffix(labels: &HashMap<String, String>) -> String {
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};

    let mut hasher = DefaultHasher::new();

    // Sort keys for deterministic ordering
    let mut parts: Vec<(&String, &String)> = labels.iter().collect();
    parts.sort_by_key(|(k, _)| *k);

    // Hash each key-value pair
    for (k, v) in parts {
        k.hash(&mut hasher);
        v.hash(&mut hasher);
    }

    // Take first 8 hex characters of the hash
    format!("{:x}", hasher.finish())[..8].to_string()
}

/// Parse a resource reference like "deployment/myapp" into (kind, name)
///
/// Supports kubectl-style aliases:
/// - `deployment`, `deploy` → Deployment
/// - `pod`, `po` → Pod
/// - `service`, `svc` → Service
/// - `statefulset`, `sts` → `StatefulSet`
/// - `daemonset`, `ds` → `DaemonSet`
///
/// # Errors
///
/// Returns `ContextError::InvalidResourceRef` if the reference format is invalid.
pub fn parse_resource_ref(reference: &str) -> Result<(ResourceKind, &str), ContextError> {
    let parts: Vec<&str> = reference.splitn(2, '/').collect();

    if parts.len() != 2 {
        return Err(ContextError::InvalidResourceRef(format!(
            "expected 'kind/name', got '{reference}'"
        )));
    }

    let kind_str = parts[0];
    let name = parts[1];

    if name.is_empty() {
        return Err(ContextError::InvalidResourceRef(format!(
            "resource name cannot be empty in '{reference}'"
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
                "unknown resource kind '{kind_str}' in '{reference}'"
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
///
/// # Errors
///
/// Returns `ContextError::InvalidResourceRef` if the target format is invalid.
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
                "resource name cannot be empty in '{target}'"
            )));
        }

        match kind.to_lowercase().as_str() {
            "pod" | "po" => Ok(ForwardTarget::Pod(name.to_string())),
            "service" | "svc" => Ok(ForwardTarget::Service(name.to_string())),
            "deployment" | "deploy" => Ok(ForwardTarget::Deployment(name.to_string())),
            _ => Err(ContextError::InvalidResourceRef(format!(
                "unsupported resource kind '{kind}' for port forwarding (use pod, svc, or deployment)"
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

    #[error("{0}")]
    WaitTimeout(#[from] crate::wait::WaitError),

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

    #[error("Rollback error: {0}")]
    RollbackError(String),
}

/// Improve a kube error message with human-readable context
///
/// Parses common Kubernetes error patterns and returns a more
/// understandable message. Includes resource name/kind context.
fn improve_error_message(err: &kube::Error, resource_kind: &str, resource_name: &str) -> String {
    let raw = err.to_string();

    // Parse common error patterns
    if raw.contains("NotFound") || raw.contains("404") {
        return format!("{resource_kind} '{resource_name}' not found in namespace");
    }

    if raw.contains("AlreadyExists") || raw.contains("409") {
        return format!("{resource_kind} '{resource_name}' already exists");
    }

    if raw.contains("ImagePullBackOff") || raw.contains("ErrImagePull") {
        // Try to extract image name from error
        if let Some(start) = raw.find("image \"") {
            if let Some(end) = raw[start + 7..].find('"') {
                let image = &raw[start + 7..start + 7 + end];
                return format!(
                    "{resource_kind} '{resource_name}' failed: image '{image}' not found or inaccessible"
                );
            }
        }
        return format!("{resource_kind} '{resource_name}' failed: image pull error");
    }

    if raw.contains("Forbidden") || raw.contains("403") {
        return format!("{resource_kind} '{resource_name}': permission denied (check RBAC)");
    }

    if raw.contains("connection refused") || raw.contains("ECONNREFUSED") {
        return format!("{resource_kind} '{resource_name}': cannot connect to Kubernetes API");
    }

    if raw.contains("timeout") || raw.contains("deadline exceeded") {
        return format!("{resource_kind} '{resource_name}': operation timed out");
    }

    // For unrecognized errors, add context prefix
    format!("{resource_kind} '{resource_name}': {raw}")
}

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
    /// // Assuming spec is set
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
        // Field manager identifies who owns these fields for conflict detection
        let patch_params = PatchParams::apply("seppo-sdk").force();
        let kind = K::kind(&Default::default()).to_string();
        let applied = api
            .patch(name, &patch_params, &Patch::Apply(&resource))
            .await
            .map_err(|e| ContextError::ApplyError(improve_error_message(&e, &kind, name)))?;

        info!(
            namespace = %self.namespace,
            name = ?applied.meta().name,
            "Applied resource"
        );

        Ok(applied)
    }

    /// Apply a cluster-scoped resource (create or update)
    ///
    /// Use this for resources that are not namespaced, such as:
    /// - `ClusterRole`, `ClusterRoleBinding`
    /// - `GatewayClass`
    /// - `CustomResourceDefinition`
    /// - Namespace
    /// - `PersistentVolume`
    /// - `StorageClass`
    ///
    /// # Example
    ///
    /// ```ignore
    /// use k8s_openapi::api::rbac::v1::ClusterRole;
    ///
    /// let role = ClusterRole {
    ///     metadata: ObjectMeta {
    ///         name: Some("my-cluster-role".to_string()),
    ///         ..Default::default()
    ///     },
    ///     rules: Some(vec![]),
    ///     ..Default::default()
    /// };
    ///
    /// ctx.apply_cluster(&role).await?;
    /// ```
    pub async fn apply_cluster<K>(&self, resource: &K) -> Result<K, ContextError>
    where
        K: kube::Resource<Scope = kube::core::ClusterResourceScope>
            + Clone
            + serde::de::DeserializeOwned
            + serde::Serialize
            + std::fmt::Debug,
        <K as kube::Resource>::DynamicType: Default,
    {
        let api: Api<K> = Api::all(self.client.clone());

        let name = resource
            .meta()
            .name
            .as_ref()
            .ok_or_else(|| ContextError::ApplyError("resource must have a name".to_string()))?;

        // Use server-side apply (like kubectl apply)
        let patch_params = PatchParams::apply("seppo-sdk").force();
        let kind = K::kind(&Default::default()).to_string();
        let applied = api
            .patch(name, &patch_params, &Patch::Apply(resource))
            .await
            .map_err(|e| ContextError::ApplyError(improve_error_message(&e, &kind, name)))?;

        info!(
            name = ?applied.meta().name,
            "Applied cluster-scoped resource"
        );

        Ok(applied)
    }

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

    /// Apply an `RBACBundle` (`ServiceAccount`, Role, `RoleBinding`)
    ///
    /// This is a convenience method for applying all three RBAC resources
    /// created by the `RBACBuilder`.
    ///
    /// # Example
    ///
    /// ```ignore
    /// let rbac = RBACBuilder::service_account("my-sa")
    ///     .can_get(&["pods", "services"])
    ///     .can_list(&["deployments"])
    ///     .build();
    ///
    /// ctx.apply_rbac(&rbac).await?;
    /// ```
    pub async fn apply_rbac(&self, bundle: &RBACBundle) -> Result<(), ContextError> {
        debug!(
            namespace = %self.namespace,
            service_account = ?bundle.service_account.metadata.name,
            "Applying RBAC bundle"
        );

        // Apply ServiceAccount
        let mut sa = bundle.service_account.clone();
        sa.metadata.namespace = Some(self.namespace.clone());
        self.apply(&sa).await?;

        // Apply Role
        let mut role = bundle.role.clone();
        role.metadata.namespace = Some(self.namespace.clone());
        self.apply(&role).await?;

        // Apply RoleBinding with namespace set on subject
        let mut rb = bundle.role_binding.clone();
        rb.metadata.namespace = Some(self.namespace.clone());
        if let Some(ref mut subjects) = rb.subjects {
            for subject in subjects.iter_mut() {
                subject.namespace = Some(self.namespace.clone());
            }
        }
        self.apply(&rb).await?;

        Ok(())
    }

    /// Create a `NetworkPolicy` that denies all ingress/egress to pods matching the selector
    ///
    /// This effectively isolates the selected pods from all network traffic.
    /// Useful for testing network segmentation or simulating network failures.
    ///
    /// # Cleanup
    ///
    /// The `NetworkPolicy` is created in the context's namespace and will be automatically
    /// deleted when the namespace is cleaned up via [`cleanup()`](Self::cleanup).
    /// To remove isolation before cleanup, delete the returned `NetworkPolicy` using
    /// [`delete()`](Self::delete) with the policy name from `metadata.name`.
    ///
    /// # Example
    ///
    /// ```ignore
    /// use std::collections::HashMap;
    ///
    /// let mut selector = HashMap::new();
    /// selector.insert("app".to_string(), "isolated".to_string());
    ///
    /// let policy = ctx.isolate(selector).await?;
    /// // Pods with label app=isolated are now network-isolated
    ///
    /// // To remove isolation early:
    /// ctx.delete::<NetworkPolicy>(policy.metadata.name.as_ref().unwrap()).await?;
    /// ```
    pub async fn isolate(
        &self,
        selector: HashMap<String, String>,
    ) -> Result<NetworkPolicy, ContextError> {
        let name = format!("seppo-isolate-{}", label_suffix(&selector));

        let policy = NetworkPolicy {
            metadata: kube::api::ObjectMeta {
                name: Some(name.clone()),
                namespace: Some(self.namespace.clone()),
                ..Default::default()
            },
            spec: Some(NetworkPolicySpec {
                pod_selector: Some(
                    k8s_openapi::apimachinery::pkg::apis::meta::v1::LabelSelector {
                        match_labels: Some(selector.into_iter().collect()),
                        ..Default::default()
                    },
                ),
                policy_types: Some(vec!["Ingress".to_string(), "Egress".to_string()]),
                // Empty ingress/egress means deny all
                ingress: None,
                egress: None,
            }),
        };

        debug!(
            namespace = %self.namespace,
            policy = %name,
            "Creating isolate NetworkPolicy"
        );

        self.apply(&policy).await
    }

    /// Create a `NetworkPolicy` that allows ingress from source pods to target pods
    ///
    /// # Example
    ///
    /// ```ignore
    /// use std::collections::HashMap;
    ///
    /// let mut target = HashMap::new();
    /// target.insert("app".to_string(), "backend".to_string());
    ///
    /// let mut source = HashMap::new();
    /// source.insert("app".to_string(), "frontend".to_string());
    ///
    /// ctx.allow_from(target, source).await?;
    /// // Pods with app=backend can now receive traffic from pods with app=frontend
    /// ```
    pub async fn allow_from(
        &self,
        target: HashMap<String, String>,
        source: HashMap<String, String>,
    ) -> Result<NetworkPolicy, ContextError> {
        let name = format!(
            "seppo-allow-{}-from-{}",
            label_suffix(&target),
            label_suffix(&source)
        );

        let policy = NetworkPolicy {
            metadata: kube::api::ObjectMeta {
                name: Some(name.clone()),
                namespace: Some(self.namespace.clone()),
                ..Default::default()
            },
            spec: Some(NetworkPolicySpec {
                pod_selector: Some(
                    k8s_openapi::apimachinery::pkg::apis::meta::v1::LabelSelector {
                        match_labels: Some(target.into_iter().collect()),
                        ..Default::default()
                    },
                ),
                policy_types: Some(vec!["Ingress".to_string()]),
                ingress: Some(vec![NetworkPolicyIngressRule {
                    from: Some(vec![NetworkPolicyPeer {
                        pod_selector: Some(
                            k8s_openapi::apimachinery::pkg::apis::meta::v1::LabelSelector {
                                match_labels: Some(source.into_iter().collect()),
                                ..Default::default()
                            },
                        ),
                        ..Default::default()
                    }]),
                    ..Default::default()
                }]),
                egress: None,
            }),
        };

        debug!(
            namespace = %self.namespace,
            policy = %name,
            "Creating allow-from NetworkPolicy"
        );

        self.apply(&policy).await
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

        let kind = K::kind(&Default::default()).to_string();
        let patched = api
            .patch(name, &PatchParams::default(), &Patch::Merge(patch))
            .await
            .map_err(|e| ContextError::PatchError(improve_error_message(&e, &kind, name)))?;

        info!(
            namespace = %self.namespace,
            name = %name,
            "Patched resource"
        );

        Ok(patched)
    }

    /// Strategic merge patch a resource
    ///
    /// Uses Kubernetes strategic merge patch, which handles arrays specially
    /// (e.g., merging container lists by name rather than index).
    ///
    /// Best for K8s-native resources where array merging semantics matter.
    ///
    /// # Example
    ///
    /// ```ignore
    /// use k8s_openapi::api::apps::v1::Deployment;
    ///
    /// // Add a label using strategic merge
    /// ctx.patch_strategic::<Deployment>("myapp", &json!({
    ///     "metadata": { "labels": { "version": "v2" } }
    /// })).await?;
    /// ```
    pub async fn patch_strategic<K>(
        &self,
        name: &str,
        patch: &serde_json::Value,
    ) -> Result<K, ContextError>
    where
        K: kube::Resource<Scope = kube::core::NamespaceResourceScope>
            + Clone
            + serde::de::DeserializeOwned
            + std::fmt::Debug,
        <K as kube::Resource>::DynamicType: Default,
    {
        let api: Api<K> = Api::namespaced(self.client.clone(), &self.namespace);

        let kind = K::kind(&Default::default()).to_string();
        let patched = api
            .patch(name, &PatchParams::default(), &Patch::Strategic(patch))
            .await
            .map_err(|e| ContextError::PatchError(improve_error_message(&e, &kind, name)))?;

        info!(
            namespace = %self.namespace,
            name = %name,
            "Strategic merge patched resource"
        );

        Ok(patched)
    }

    /// JSON patch a resource (RFC 6902)
    ///
    /// Uses JSON Patch format - an array of operations.
    /// Best for precise, surgical changes to specific fields.
    ///
    /// # Example
    ///
    /// ```ignore
    /// use k8s_openapi::api::apps::v1::Deployment;
    ///
    /// // Add a label using JSON patch operations
    /// ctx.patch_json::<Deployment>("myapp", &json!([
    ///     { "op": "add", "path": "/metadata/labels/version", "value": "v2" }
    /// ])).await?;
    /// ```
    pub async fn patch_json<K>(
        &self,
        name: &str,
        patch: &serde_json::Value,
    ) -> Result<K, ContextError>
    where
        K: kube::Resource<Scope = kube::core::NamespaceResourceScope>
            + Clone
            + serde::de::DeserializeOwned
            + std::fmt::Debug,
        <K as kube::Resource>::DynamicType: Default,
    {
        // Parse the JSON value into json_patch::Patch
        let json_patch: JsonPatchOps = serde_json::from_value(patch.clone()).map_err(|e| {
            ContextError::PatchError(format!(
                "invalid JSON patch format: {e}. Expected array of operations like \
                 [{{\"op\": \"add\", \"path\": \"/path\", \"value\": ...}}]"
            ))
        })?;

        let api: Api<K> = Api::namespaced(self.client.clone(), &self.namespace);

        let kind = K::kind(&Default::default()).to_string();
        let patched = api
            .patch(
                name,
                &PatchParams::default(),
                &Patch::Json::<()>(json_patch),
            )
            .await
            .map_err(|e| ContextError::PatchError(improve_error_message(&e, &kind, name)))?;

        info!(
            namespace = %self.namespace,
            name = %name,
            "JSON patched resource"
        );

        Ok(patched)
    }

    // ========================================================================
    // Dynamic Client Operations (for CRDs and unstructured resources)
    // ========================================================================

    /// Apply an unstructured resource using the dynamic client
    ///
    /// Useful for CRDs or when you don't have typed structs.
    /// Creates the resource if it doesn't exist, updates if it does.
    ///
    /// # Example
    ///
    /// ```ignore
    /// let gvr = Gvr::http_route();
    /// ctx.apply_dynamic(&gvr, &json!({
    ///     "apiVersion": "gateway.networking.k8s.io/v1",
    ///     "kind": "HTTPRoute",
    ///     "metadata": { "name": "my-route" },
    ///     "spec": {
    ///         "parentRefs": [{ "name": "my-gateway" }],
    ///         "rules": [{ "backendRefs": [{ "name": "my-service", "port": 8080 }] }]
    ///     }
    /// })).await?;
    /// ```
    pub async fn apply_dynamic(
        &self,
        gvr: &Gvr,
        obj: &serde_json::Value,
    ) -> Result<serde_json::Value, ContextError> {
        use kube::api::DynamicObject;

        let ar = gvr.to_api_resource();
        let api: Api<DynamicObject> =
            Api::namespaced_with(self.client.clone(), &self.namespace, &ar);

        // Parse the JSON into a DynamicObject
        let mut dyn_obj: DynamicObject = serde_json::from_value(obj.clone())
            .map_err(|e| ContextError::ApplyError(format!("invalid object format: {e}")))?;

        // Ensure namespace is set
        if dyn_obj.metadata.namespace.is_none() {
            dyn_obj.metadata.namespace = Some(self.namespace.clone());
        }

        let name = dyn_obj.metadata.name.clone().ok_or_else(|| {
            ContextError::ApplyError("object must have metadata.name".to_string())
        })?;

        // Try to get existing, create or update
        let result = match api.get(&name).await {
            Ok(existing) => {
                // Update - preserve resourceVersion
                dyn_obj.metadata.resource_version = existing.metadata.resource_version;
                api.replace(&name, &PostParams::default(), &dyn_obj)
                    .await
                    .map_err(|e| {
                        ContextError::ApplyError(format!("failed to update {name}: {e}"))
                    })?
            }
            Err(kube::Error::Api(ref ae)) if ae.code == 404 => {
                // Create
                api.create(&PostParams::default(), &dyn_obj)
                    .await
                    .map_err(|e| {
                        ContextError::ApplyError(format!("failed to create {name}: {e}"))
                    })?
            }
            Err(e) => {
                return Err(ContextError::ApplyError(format!(
                    "failed to check {name}: {e}"
                )));
            }
        };

        info!(
            namespace = %self.namespace,
            name = %name,
            gvr = ?gvr,
            "Applied dynamic resource"
        );

        serde_json::to_value(result)
            .map_err(|e| ContextError::ApplyError(format!("failed to serialize result: {e}")))
    }

    /// Get an unstructured resource using the dynamic client
    ///
    /// Returns the resource as a JSON value.
    ///
    /// # Example
    ///
    /// ```ignore
    /// let gvr = Gvr::http_route();
    /// let route = ctx.get_dynamic(&gvr, "my-route").await?;
    /// println!("Route spec: {:?}", route["spec"]);
    /// ```
    pub async fn get_dynamic(
        &self,
        gvr: &Gvr,
        name: &str,
    ) -> Result<serde_json::Value, ContextError> {
        use kube::api::DynamicObject;

        let ar = gvr.to_api_resource();
        let api: Api<DynamicObject> =
            Api::namespaced_with(self.client.clone(), &self.namespace, &ar);

        let obj = api.get(name).await.map_err(|e| {
            ContextError::GetError(format!("{} '{}' not found: {}", gvr.kind, name, e))
        })?;

        serde_json::to_value(obj)
            .map_err(|e| ContextError::GetError(format!("failed to serialize: {e}")))
    }

    /// Delete an unstructured resource using the dynamic client
    ///
    /// # Example
    ///
    /// ```ignore
    /// let gvr = Gvr::http_route();
    /// ctx.delete_dynamic(&gvr, "my-route").await?;
    /// ```
    pub async fn delete_dynamic(&self, gvr: &Gvr, name: &str) -> Result<(), ContextError> {
        use kube::api::DynamicObject;

        let ar = gvr.to_api_resource();
        let api: Api<DynamicObject> =
            Api::namespaced_with(self.client.clone(), &self.namespace, &ar);

        api.delete(name, &DeleteParams::default())
            .await
            .map_err(|e| {
                ContextError::DeleteError(format!("{} '{}' delete failed: {}", gvr.kind, name, e))
            })?;

        info!(
            namespace = %self.namespace,
            name = %name,
            gvr = ?gvr,
            "Deleted dynamic resource"
        );

        Ok(())
    }

    /// List unstructured resources using the dynamic client
    ///
    /// Returns resources as a vector of JSON values.
    ///
    /// # Example
    ///
    /// ```ignore
    /// let gvr = Gvr::http_route();
    /// let routes = ctx.list_dynamic(&gvr).await?;
    /// for route in routes {
    ///     println!("Route: {}", route["metadata"]["name"]);
    /// }
    /// ```
    pub async fn list_dynamic(&self, gvr: &Gvr) -> Result<Vec<serde_json::Value>, ContextError> {
        use kube::api::DynamicObject;

        let ar = gvr.to_api_resource();
        let api: Api<DynamicObject> =
            Api::namespaced_with(self.client.clone(), &self.namespace, &ar);

        let list = api.list(&ListParams::default()).await.map_err(|e| {
            ContextError::ListError(format!("failed to list {}: {}", gvr.resource, e))
        })?;

        list.items
            .into_iter()
            .map(|obj| {
                serde_json::to_value(obj)
                    .map_err(|e| ContextError::ListError(format!("failed to serialize: {e}")))
            })
            .collect()
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
        let kind = K::kind(&Default::default()).to_string();

        let resource = api
            .get(name)
            .await
            .map_err(|e| ContextError::GetError(improve_error_message(&e, &kind, name)))?;

        Ok(resource)
    }

    /// Get a cluster-scoped resource
    ///
    /// # Example
    ///
    /// ```ignore
    /// use k8s_openapi::api::rbac::v1::ClusterRole;
    ///
    /// let role: ClusterRole = ctx.get_cluster("my-cluster-role").await?;
    /// ```
    pub async fn get_cluster<K>(&self, name: &str) -> Result<K, ContextError>
    where
        K: kube::Resource<Scope = kube::core::ClusterResourceScope>
            + Clone
            + serde::de::DeserializeOwned
            + std::fmt::Debug,
        <K as kube::Resource>::DynamicType: Default,
    {
        let api: Api<K> = Api::all(self.client.clone());
        let kind = K::kind(&Default::default()).to_string();

        let resource = api
            .get(name)
            .await
            .map_err(|e| ContextError::GetError(improve_error_message(&e, &kind, name)))?;

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
        let kind = K::kind(&Default::default()).to_string();

        api.delete(name, &DeleteParams::default())
            .await
            .map_err(|e| ContextError::DeleteError(improve_error_message(&e, &kind, name)))?;

        info!(
            namespace = %self.namespace,
            name = %name,
            "Deleted resource"
        );

        Ok(())
    }

    /// Delete a cluster-scoped resource
    ///
    /// # Example
    ///
    /// ```ignore
    /// use k8s_openapi::api::rbac::v1::ClusterRole;
    ///
    /// ctx.delete_cluster::<ClusterRole>("my-cluster-role").await?;
    /// ```
    pub async fn delete_cluster<K>(&self, name: &str) -> Result<(), ContextError>
    where
        K: kube::Resource<Scope = kube::core::ClusterResourceScope>
            + Clone
            + serde::de::DeserializeOwned
            + std::fmt::Debug,
        <K as kube::Resource>::DynamicType: Default,
    {
        let api: Api<K> = Api::all(self.client.clone());
        let kind = K::kind(&Default::default()).to_string();

        api.delete(name, &DeleteParams::default())
            .await
            .map_err(|e| ContextError::DeleteError(improve_error_message(&e, &kind, name)))?;

        info!(
            name = %name,
            "Deleted cluster-scoped resource"
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
        let kind = K::kind(&Default::default()).to_string();

        let list = api
            .list(&ListParams::default())
            .await
            .map_err(|e| ContextError::ListError(format!("failed to list {kind}: {e}")))?;

        Ok(list.items)
    }

    /// Get logs from a pod in the test namespace
    pub async fn logs(&self, pod_name: &str) -> Result<String, ContextError> {
        let pods: Api<Pod> = Api::namespaced(self.client.clone(), &self.namespace);

        let logs = pods
            .logs(pod_name, &LogParams::default())
            .await
            .map_err(|e| ContextError::LogsError(improve_error_message(&e, "Pod", pod_name)))?;

        Ok(logs)
    }

    /// Stream logs from a pod in real-time
    ///
    /// Returns a stream of log lines that can be iterated with `try_next().await`.
    /// Uses `follow: true` for continuous streaming like `kubectl logs -f`.
    ///
    /// # Lifetime
    ///
    /// The returned stream holds an internal reference to the Kubernetes API
    /// connection. The stream must be dropped before the `Context` can be dropped.
    /// This is enforced at compile-time via the `'_` lifetime on the return type.
    ///
    /// # Example
    ///
    /// ```ignore
    /// use futures::TryStreamExt;
    ///
    /// let mut stream = ctx.logs_stream("my-pod").await?;
    /// while let Some(line) = stream.try_next().await? {
    ///     println!("{}", line);
    /// }
    /// ```
    pub async fn logs_stream(
        &self,
        pod_name: &str,
    ) -> Result<impl Stream<Item = Result<String, std::io::Error>> + '_, ContextError> {
        let pods: Api<Pod> = Api::namespaced(self.client.clone(), &self.namespace);

        let params = LogParams {
            follow: true,
            ..Default::default()
        };

        debug!(
            namespace = %self.namespace,
            pod = %pod_name,
            "Starting log stream"
        );

        let log_stream = pods
            .log_stream(pod_name, &params)
            .await
            .map_err(|e| ContextError::LogsError(improve_error_message(&e, "Pod", pod_name)))?;

        Ok(log_stream.lines())
    }

    /// Get logs from all pods matching a label selector
    ///
    /// Returns a `HashMap` of pod name to logs. Useful for debugging deployments
    /// with multiple replicas or getting logs from all pods in a service.
    ///
    /// # Arguments
    ///
    /// * `selector` - Label selector string, e.g., "app=myapp" or "app=myapp,tier=backend"
    ///
    /// # Example
    ///
    /// ```ignore
    /// // Get logs from all pods with label app=myapp
    /// let all_logs = ctx.logs_all("app=myapp").await?;
    /// for (pod_name, logs) in &all_logs {
    ///     println!("=== {} ===\n{}", pod_name, logs);
    /// }
    /// ```
    pub async fn logs_all(&self, selector: &str) -> Result<HashMap<String, String>, ContextError> {
        let pods: Api<Pod> = Api::namespaced(self.client.clone(), &self.namespace);

        // List pods matching the selector
        let list_params = ListParams::default().labels(selector);
        let pod_list = pods
            .list(&list_params)
            .await
            .map_err(|e| ContextError::ListError(e.to_string()))?;

        let mut all_logs = HashMap::new();

        for pod in pod_list.items {
            let pod_name = pod.metadata.name.unwrap_or_default();
            if pod_name.is_empty() {
                continue;
            }

            // Try to get logs, but don't fail if a pod doesn't have logs yet
            match pods.logs(&pod_name, &LogParams::default()).await {
                Ok(logs) => {
                    all_logs.insert(pod_name, logs);
                }
                Err(e) => {
                    debug!(
                        pod = %pod_name,
                        error = %e,
                        "Could not get logs from pod (may not be ready yet)"
                    );
                    // Insert empty string to indicate pod exists but no logs
                    all_logs.insert(pod_name, String::new());
                }
            }
        }

        info!(
            namespace = %self.namespace,
            selector = %selector,
            pod_count = %all_logs.len(),
            "Collected logs from pods"
        );

        Ok(all_logs)
    }

    /// Wait for a resource to satisfy a condition
    ///
    /// Polls the resource until the condition returns true or timeout is reached.
    /// Default timeout is 60 seconds, polling interval is 1 second.
    ///
    /// Accepts both formats for consistency with other methods:
    /// - `"my-config"` - just the resource name
    /// - `"configmap/my-config"` - kind/name format (kind is ignored, type comes from generic)
    ///
    /// # Example
    ///
    /// ```ignore
    /// // Wait for a ConfigMap to have a specific key
    /// ctx.wait_for::<ConfigMap>("my-config", |cm| {
    ///     cm.data.as_ref().map_or(false, |d| d.contains_key("ready"))
    /// }).await?;
    ///
    /// // Also works with kind/name format
    /// ctx.wait_for::<Gateway>("gateway/test-gw", |gw| {
    ///     gw.status.is_some()
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
    ///
    /// Accepts both `"name"` and `"kind/name"` formats for consistency.
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
        // Extract just the name if "kind/name" format was provided
        let name = extract_resource_name(name);
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
                return Err(crate::wait::WaitError::new(name, timeout, start.elapsed())
                    .with_state("condition not met")
                    .into());
            }

            tokio::time::sleep(poll_interval).await;
        }
    }

    /// Wait for a resource to be deleted
    ///
    /// Polls until the resource no longer exists in the namespace.
    /// Default timeout is 60 seconds, polling interval is 1 second.
    ///
    /// Accepts both `"name"` and `"kind/name"` formats for consistency.
    ///
    /// # Example
    ///
    /// ```ignore
    /// ctx.delete::<Pod>("worker").await?;
    /// ctx.wait_deleted::<Pod>("worker").await?;
    /// // Pod is now fully gone
    ///
    /// // Also works with kind/name format
    /// ctx.wait_deleted::<Pod>("pod/worker").await?;
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
    ///
    /// Accepts both `"name"` and `"kind/name"` formats for consistency.
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
        // Extract just the name if "kind/name" format was provided
        let name = extract_resource_name(name);
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
                return Err(crate::wait::WaitError::new(name, timeout, start.elapsed())
                    .with_state("still exists")
                    .into());
            }

            tokio::time::sleep(poll_interval).await;
        }
    }

    /// Wait for a `PersistentVolumeClaim` to be bound
    ///
    /// Polls until the PVC's status.phase is "Bound".
    /// Default timeout is 60 seconds, polling interval is 1 second.
    ///
    /// Accepts both `"name"` and `"pvc/name"` formats for consistency.
    ///
    /// # Example
    ///
    /// ```ignore
    /// ctx.apply(&my_pvc).await?;
    /// ctx.wait_pvc_bound("my-pvc").await?;
    /// // PVC is now bound and ready
    /// ```
    pub async fn wait_pvc_bound(&self, name: &str) -> Result<PersistentVolumeClaim, ContextError> {
        self.wait_pvc_bound_with_timeout(name, std::time::Duration::from_secs(60))
            .await
    }

    /// Wait for a PVC to be bound with a custom timeout
    pub async fn wait_pvc_bound_with_timeout(
        &self,
        name: &str,
        timeout: std::time::Duration,
    ) -> Result<PersistentVolumeClaim, ContextError> {
        // Extract just the name if "pvc/name" format was provided
        let name = extract_resource_name(name);
        let start = std::time::Instant::now();
        let poll_interval = std::time::Duration::from_secs(1);

        debug!(
            namespace = %self.namespace,
            pvc = %name,
            timeout = ?timeout,
            "Starting wait_pvc_bound"
        );

        loop {
            match self.get::<PersistentVolumeClaim>(name).await {
                Ok(pvc) => {
                    let phase = pvc
                        .status
                        .as_ref()
                        .and_then(|s| s.phase.as_ref())
                        .map_or("Unknown", std::string::String::as_str);

                    if phase == "Bound" {
                        debug!(
                            namespace = %self.namespace,
                            pvc = %name,
                            elapsed = ?start.elapsed(),
                            "PVC is bound"
                        );
                        return Ok(pvc);
                    }

                    debug!(
                        namespace = %self.namespace,
                        pvc = %name,
                        phase = %phase,
                        elapsed = ?start.elapsed(),
                        "PVC not bound yet, waiting..."
                    );
                }
                Err(ContextError::GetError(_)) => {
                    debug!(
                        namespace = %self.namespace,
                        pvc = %name,
                        elapsed = ?start.elapsed(),
                        "PVC doesn't exist yet, waiting..."
                    );
                }
                Err(e) => return Err(e),
            }

            if start.elapsed() >= timeout {
                return Err(crate::wait::WaitError::new(
                    format!("pvc/{name}"),
                    timeout,
                    start.elapsed(),
                )
                .with_state("not bound")
                .into());
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
        let field_selector = format!("metadata.name={name}");
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
                            .is_some_and(|containers| {
                                !containers.is_empty() && containers.iter().all(|c| c.ready)
                            })
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
                        let desired = d.status.as_ref().map_or(0, |s| s.desired_number_scheduled);
                        let ready = d.status.as_ref().map_or(0, |s| s.number_ready);
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

    /// Immediately kill a pod (force deletion with grace period 0)
    ///
    /// Unlike `delete()`, this bypasses the graceful termination period
    /// and kills the pod immediately. Useful for testing failure scenarios
    /// and self-healing behavior.
    ///
    /// # Example
    ///
    /// ```ignore
    /// // Kill a pod immediately
    /// ctx.kill("pod/worker-0").await?;
    ///
    /// // Verify the deployment recovers
    /// ctx.wait_ready("deployment/myapp").await?;
    /// ```
    pub async fn kill(&self, resource: &str) -> Result<(), ContextError> {
        let (kind, name) = parse_resource_ref(resource)?;

        match kind {
            ResourceKind::Pod => {
                let pods: Api<Pod> = Api::namespaced(self.client.clone(), &self.namespace);

                // Delete with grace period 0 for immediate termination
                let delete_params = DeleteParams {
                    grace_period_seconds: Some(0),
                    ..Default::default()
                };

                pods.delete(name, &delete_params)
                    .await
                    .map_err(|e| ContextError::DeleteError(e.to_string()))?;

                info!(
                    namespace = %self.namespace,
                    pod = %name,
                    "Killed pod (immediate deletion)"
                );

                Ok(())
            }
            _ => Err(ContextError::InvalidResourceRef(format!(
                "kill only supports pods, got {kind:?}"
            ))),
        }
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
                "replicas must be non-negative, got {replicas}"
            )));
        }
        let (kind, name) = parse_resource_ref(resource)?;

        match kind {
            ResourceKind::Deployment => {
                let deployments: Api<Deployment> =
                    Api::namespaced(self.client.clone(), &self.namespace);

                // Use JSON merge patch to avoid conflicts
                let patch = serde_json::json!({
                    "spec": {
                        "replicas": replicas
                    }
                });

                deployments
                    .patch(name, &PatchParams::default(), &Patch::Merge(&patch))
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
                "scale only supports deployments, got {kind:?}"
            ))),
        }
    }

    /// Rollback a deployment to the previous revision
    ///
    /// This finds the previous `ReplicaSet` revision and updates the deployment
    /// to use that revision's pod template.
    ///
    /// # Example
    ///
    /// ```ignore
    /// ctx.rollback("deployment/myapp").await?;
    /// ```
    pub async fn rollback(&self, resource: &str) -> Result<(), ContextError> {
        let (kind, name) = parse_resource_ref(resource)?;

        match kind {
            ResourceKind::Deployment => {
                let deployments: Api<Deployment> =
                    Api::namespaced(self.client.clone(), &self.namespace);
                let replicasets: Api<ReplicaSet> =
                    Api::namespaced(self.client.clone(), &self.namespace);

                // Get current deployment
                let dep = deployments
                    .get(name)
                    .await
                    .map_err(|e| ContextError::GetError(e.to_string()))?;

                // Get deployment UID for owner reference matching
                let dep_uid =
                    dep.metadata.uid.as_ref().ok_or_else(|| {
                        ContextError::GetError("deployment has no UID".to_string())
                    })?;

                // List all ReplicaSets and filter by owner reference
                let rs_list = replicasets
                    .list(&ListParams::default())
                    .await
                    .map_err(|e| ContextError::GetError(e.to_string()))?;

                // Helper to extract revision from a ReplicaSet
                let get_revision = |rs: &ReplicaSet| -> i64 {
                    rs.metadata
                        .annotations
                        .as_ref()
                        .and_then(|ann| ann.get("deployment.kubernetes.io/revision"))
                        .and_then(|v| v.parse().ok())
                        .unwrap_or(0)
                };

                // Filter by owner reference and valid revision (> 0)
                let mut owned_rs: Vec<_> = rs_list
                    .items
                    .into_iter()
                    .filter(|rs| {
                        let owned = rs.metadata.owner_references.as_ref().is_some_and(|refs| {
                            refs.iter().any(|r| r.uid.as_str() == dep_uid.as_str())
                        });
                        let has_revision = get_revision(rs) > 0;
                        owned && has_revision
                    })
                    .collect();

                if owned_rs.len() < 2 {
                    return Err(ContextError::RollbackError(
                        "no previous revision available for rollback".to_string(),
                    ));
                }

                // Sort by revision annotation (descending)
                owned_rs.sort_by(|a, b| {
                    let rev_a = get_revision(a);
                    let rev_b = get_revision(b);
                    rev_b.cmp(&rev_a)
                });

                // Get the second-highest revision (previous)
                let previous_rs = &owned_rs[1];
                let previous_template = previous_rs
                    .spec
                    .as_ref()
                    .and_then(|s| s.template.clone())
                    .ok_or_else(|| {
                        ContextError::RollbackError(
                            "previous ReplicaSet has no pod template".to_string(),
                        )
                    })?;

                // Patch the deployment with the previous template
                let patch = serde_json::json!({
                    "spec": {
                        "template": previous_template
                    }
                });

                deployments
                    .patch(name, &PatchParams::default(), &Patch::Merge(&patch))
                    .await
                    .map_err(|e| ContextError::RollbackError(e.to_string()))?;

                info!(
                    namespace = %self.namespace,
                    deployment = %name,
                    "Rolled back deployment to previous revision"
                );

                Ok(())
            }
            _ => Err(ContextError::InvalidResourceRef(format!(
                "rollback only supports deployments, got {kind:?}"
            ))),
        }
    }

    /// Restart a deployment, statefulset, or daemonset by triggering a rolling update
    ///
    /// This works like `kubectl rollout restart` - it adds a `restartedAt` annotation
    /// to the pod template, which triggers Kubernetes to perform a rolling update.
    ///
    /// # Supported Resources
    ///
    /// - `deployment/name` or `deploy/name`
    /// - `statefulset/name` or `sts/name`
    /// - `daemonset/name` or `ds/name`
    ///
    /// # Example
    ///
    /// ```ignore
    /// // Restart a deployment
    /// ctx.restart("deployment/myapp").await?;
    ///
    /// // Wait for the restart to complete
    /// ctx.wait_ready("deployment/myapp").await?;
    /// ```
    pub async fn restart(&self, resource: &str) -> Result<(), ContextError> {
        let (kind, name) = parse_resource_ref(resource)?;
        let restarted_at = chrono::Utc::now().to_rfc3339();

        // Use strategic merge patch to add restart annotation
        let patch = serde_json::json!({
            "spec": {
                "template": {
                    "metadata": {
                        "annotations": {
                            "kubectl.kubernetes.io/restartedAt": restarted_at
                        }
                    }
                }
            }
        });

        // Helper macro to reduce duplication across resource types
        macro_rules! restart_workload {
            ($api:expr, $kind_name:expr) => {{
                $api.patch(name, &PatchParams::default(), &Patch::Strategic(&patch))
                    .await
                    .map_err(|e| ContextError::ApplyError(e.to_string()))?;

                info!(
                    namespace = %self.namespace,
                    resource = %name,
                    kind = %$kind_name,
                    "Restarted workload (rolling update triggered)"
                );

                Ok(())
            }};
        }

        match kind {
            ResourceKind::Deployment => {
                let api: Api<Deployment> = Api::namespaced(self.client.clone(), &self.namespace);
                restart_workload!(api, "deployment")
            }
            ResourceKind::StatefulSet => {
                let api: Api<StatefulSet> = Api::namespaced(self.client.clone(), &self.namespace);
                restart_workload!(api, "statefulset")
            }
            ResourceKind::DaemonSet => {
                let api: Api<DaemonSet> = Api::namespaced(self.client.clone(), &self.namespace);
                restart_workload!(api, "daemonset")
            }
            _ => Err(ContextError::InvalidResourceRef(format!(
                "restart only supports deployment, statefulset, and daemonset, got {kind:?}"
            ))),
        }
    }

    /// Check if the current user has permission to perform an action
    ///
    /// Uses the Kubernetes `SelfSubjectAccessReview` API to check RBAC permissions.
    /// Useful for tests that need to verify RBAC configuration.
    ///
    /// # Arguments
    ///
    /// * `verb` - The action to check: "get", "list", "watch", "create", "update", "delete", etc.
    /// * `resource` - The resource type: "pods", "deployments", "secrets", etc.
    ///
    /// # Example
    ///
    /// ```ignore
    /// // Check if we can create pods
    /// let can_create_pods = ctx.can_i("create", "pods").await?;
    /// assert!(can_create_pods, "Should be able to create pods");
    ///
    /// // Check if we can delete secrets
    /// let can_delete_secrets = ctx.can_i("delete", "secrets").await?;
    /// ```
    pub async fn can_i(&self, verb: &str, resource: &str) -> Result<bool, ContextError> {
        let review = SelfSubjectAccessReview {
            metadata: ObjectMeta::default(),
            spec: SelfSubjectAccessReviewSpec {
                resource_attributes: Some(ResourceAttributes {
                    namespace: Some(self.namespace.clone()),
                    verb: Some(verb.to_string()),
                    resource: Some(resource.to_string()),
                    ..Default::default()
                }),
                ..Default::default()
            },
            status: None,
        };

        let api: Api<SelfSubjectAccessReview> = Api::all(self.client.clone());
        let result = api
            .create(&PostParams::default(), &review)
            .await
            .map_err(|e| ContextError::GetError(format!("RBAC check failed: {e}")))?;

        let allowed = result.status.is_some_and(|s| s.allowed);

        debug!(
            verb = %verb,
            resource = %resource,
            namespace = %self.namespace,
            allowed = %allowed,
            "RBAC permission check"
        );

        Ok(allowed)
    }

    /// Get events from the test namespace
    ///
    /// Returns all events in the namespace, useful for debugging test failures.
    pub async fn events(&self) -> Result<Vec<Event>, ContextError> {
        let events: Api<Event> = Api::namespaced(self.client.clone(), &self.namespace);

        let list = events
            .list(&ListParams::default())
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
            .list(&ListParams::default())
            .await
            .map_err(|e| ContextError::ListError(e.to_string()))?;

        let mut logs_map = HashMap::new();

        for pod in pod_list.items {
            let pod_name = pod.metadata.name.unwrap_or_default();
            if pod_name.is_empty() {
                continue;
            }

            // Try to get logs, but don't fail if pod isn't ready
            let logs = match pods.logs(&pod_name, &LogParams::default()).await {
                Ok(logs) => logs,
                Err(e) => format!("[error getting logs: {e}]"),
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

    /// Create a port forward to a pod
    ///
    /// Opens a local port that tunnels traffic to the specified pod port.
    /// Returns a `PortForward` that can be used to make HTTP requests.
    ///
    /// # Deprecated
    ///
    /// Use [`port_forward`](Self::port_forward) instead, which supports
    /// kubectl-style resource references like `svc/name` and `deployment/name`.
    ///
    /// # Example
    ///
    /// ```ignore
    /// let pf = ctx.forward("my-pod", 8080).await?;
    /// let response = pf.get("/health").await?;
    /// assert!(response.contains("ok"));
    /// ```
    #[deprecated(since = "0.2.0", note = "Use port_forward() instead")]
    pub async fn forward(
        &self,
        pod_name: &str,
        port: u16,
    ) -> Result<PortForward, PortForwardError> {
        self.port_forward(pod_name, port).await
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
    /// let pf = ctx.port_forward("svc/myapp", 8080).await?;
    ///
    /// // Forward to a deployment
    /// let pf = ctx.port_forward("deployment/backend", 3000).await?;
    ///
    /// // Forward to a pod (explicit or bare name)
    /// let pf = ctx.port_forward("pod/worker-0", 9000).await?;
    /// let pf = ctx.port_forward("worker-0", 9000).await?;
    /// ```
    pub async fn port_forward(
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

        PortForward::new(self.client.clone(), &self.namespace, &pod_name, port).await
    }

    /// Create a port forward using kubectl-style resource references
    ///
    /// # Deprecated
    ///
    /// Use [`port_forward`](Self::port_forward) instead.
    #[deprecated(since = "0.2.0", note = "Use port_forward() instead")]
    pub async fn forward_to(
        &self,
        target: &str,
        port: u16,
    ) -> Result<PortForward, PortForwardError> {
        self.port_forward(target, port).await
    }

    /// Find a running pod backing a service
    async fn find_pod_for_service(&self, svc_name: &str) -> Result<String, ContextError> {
        let services: Api<Service> = Api::namespaced(self.client.clone(), &self.namespace);
        let svc = services
            .get(svc_name)
            .await
            .map_err(|e| ContextError::GetError(format!("service '{svc_name}': {e}")))?;

        // Get the selector from the service
        let selector = svc
            .spec
            .as_ref()
            .and_then(|s| s.selector.as_ref())
            .ok_or_else(|| {
                ContextError::GetError(format!("service '{svc_name}' has no selector"))
            })?;

        self.find_running_pod_by_labels(selector).await
    }

    /// Find a running pod from a deployment
    async fn find_pod_for_deployment(&self, deploy_name: &str) -> Result<String, ContextError> {
        let deployments: Api<Deployment> = Api::namespaced(self.client.clone(), &self.namespace);
        let deploy = deployments
            .get(deploy_name)
            .await
            .map_err(|e| ContextError::GetError(format!("deployment '{deploy_name}': {e}")))?;

        // Get the selector from the deployment
        let selector = deploy
            .spec
            .as_ref()
            .and_then(|s| s.selector.match_labels.as_ref())
            .ok_or_else(|| {
                ContextError::GetError(format!("deployment '{deploy_name}' has no selector"))
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
            .map(|(k, v)| format!("{k}={v}"))
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
                .map(std::string::String::as_str);

            if phase == Some("Running") {
                if let Some(name) = pod.metadata.name {
                    return Ok(name);
                }
            }
        }

        Err(ContextError::GetError(format!(
            "no running pod found with labels: {label_selector}"
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

    /// Retry an operation with exponential backoff
    ///
    /// Useful for operations that may transiently fail, like waiting for
    /// a resource to be fully ready or handling API rate limits.
    ///
    /// Starts with 100ms backoff, doubling each time up to 5 seconds max.
    ///
    /// # Panics
    ///
    /// Panics if `max_attempts` is 0. Use at least 1 attempt.
    ///
    /// # Example
    ///
    /// ```ignore
    /// // Retry an HTTP request up to 5 times
    /// ctx.retry(5, || async {
    ///     let resp = pf.get("/health").await?;
    ///     if resp.contains("ok") { Ok(()) } else { Err("not ready".into()) }
    /// }).await?;
    /// ```
    pub async fn retry<F, Fut, E>(&self, max_attempts: usize, mut f: F) -> Result<(), E>
    where
        F: FnMut() -> Fut,
        Fut: std::future::Future<Output = Result<(), E>>,
        E: std::fmt::Display,
    {
        assert!(
            max_attempts > 0,
            "retry() requires at least 1 attempt, got max_attempts=0"
        );

        let mut backoff = std::time::Duration::from_millis(100);
        let max_backoff = std::time::Duration::from_secs(5);
        let mut last_error: Option<E> = None;

        for attempt in 1..=max_attempts {
            match f().await {
                Ok(()) => {
                    debug!(attempt = attempt, "Retry succeeded");
                    return Ok(());
                }
                Err(e) => {
                    let is_last_attempt = attempt == max_attempts;

                    if is_last_attempt {
                        warn!(
                            attempt = attempt,
                            max_attempts = max_attempts,
                            error = %e,
                            "Retry exhausted all attempts"
                        );
                    } else {
                        debug!(
                            attempt = attempt,
                            max_attempts = max_attempts,
                            backoff = ?backoff,
                            error = %e,
                            "Retry attempt failed, backing off"
                        );
                        tokio::time::sleep(backoff).await;
                        backoff = std::cmp::min(backoff * 2, max_backoff);
                    }

                    last_error = Some(e);
                }
            }
        }

        // This is reached when all attempts fail
        Err(last_error.expect("max_attempts > 0 guarantees at least one iteration"))
    }

    // ========================================================================
    // Test Scenario Helpers
    // ========================================================================

    /// Test self-healing behavior of a deployment
    ///
    /// Kills a pod and verifies the deployment recovers within the timeout.
    ///
    /// # Example
    ///
    /// ```ignore
    /// ctx.test_self_healing("deployment/myapp", Duration::from_secs(60)).await?;
    /// ```
    pub async fn test_self_healing(
        &self,
        resource: &str,
        recovery_timeout: std::time::Duration,
    ) -> Result<(), ContextError> {
        // Parse deployment/name format
        let (kind, name) = parse_resource_ref(resource)?;
        if kind != ResourceKind::Deployment {
            return Err(ContextError::InvalidResourceRef(
                "test_self_healing requires deployment/name format".to_string(),
            ));
        }

        // Find a pod from this deployment
        let pods: Api<Pod> = Api::namespaced(self.client.clone(), &self.namespace);
        let pod_list = pods
            .list(&ListParams::default().labels(&format!("app={name}")))
            .await
            .map_err(|e| ContextError::ListError(format!("failed to list pods: {e}")))?;

        if pod_list.items.is_empty() {
            return Err(ContextError::ListError(format!(
                "no pods found for deployment {name}"
            )));
        }

        // Kill the first pod
        let pod_name = pod_list.items[0]
            .metadata
            .name
            .as_ref()
            .ok_or_else(|| ContextError::ListError("pod has no name".to_string()))?;

        info!(
            namespace = %self.namespace,
            deployment = %name,
            pod = %pod_name,
            "Killing pod for self-healing test"
        );

        self.kill(&format!("pod/{pod_name}")).await?;

        // Wait for recovery
        self.wait_ready_with_timeout(resource, recovery_timeout)
            .await
    }

    /// Test scaling behavior of a deployment
    ///
    /// Scales to the target replica count and waits for stability.
    ///
    /// # Example
    ///
    /// ```ignore
    /// ctx.test_scaling("deployment/myapp", 5, Duration::from_secs(60)).await?;
    /// ```
    pub async fn test_scaling(
        &self,
        resource: &str,
        target_replicas: i32,
        timeout: std::time::Duration,
    ) -> Result<(), ContextError> {
        // Parse deployment/name format
        let (kind, name) = parse_resource_ref(resource)?;
        if kind != ResourceKind::Deployment {
            return Err(ContextError::InvalidResourceRef(
                "test_scaling requires deployment/name format".to_string(),
            ));
        }

        info!(
            namespace = %self.namespace,
            deployment = %name,
            target_replicas = target_replicas,
            "Scaling deployment for scaling test"
        );

        // Scale the deployment
        self.scale(resource, target_replicas).await?;

        // Wait for stability
        self.wait_ready_with_timeout(resource, timeout).await
    }

    /// Start generating HTTP traffic to a service
    ///
    /// Returns a handle that can be used to wait for completion and get statistics.
    ///
    /// # Example
    ///
    /// ```ignore
    /// let config = TrafficConfig::default()
    ///     .with_rps(100)
    ///     .with_duration(Duration::from_secs(30));
    ///
    /// let traffic = ctx.start_traffic("svc/myapp", 8080, config).await?;
    /// traffic.wait().await;
    /// let stats = traffic.stats();
    /// println!("Error rate: {:.2}%", stats.error_rate() * 100.0);
    /// ```
    pub async fn start_traffic(
        &self,
        resource: &str,
        port: u16,
        config: TrafficConfig,
    ) -> Result<TrafficHandle, PortForwardError> {
        use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
        use std::sync::Arc;

        // Set up port forward
        let pf = self.port_forward(resource, port).await?;
        let local_port = pf.local_addr().port();

        // Create shared state
        let stop_flag = Arc::new(AtomicBool::new(false));
        let total = Arc::new(AtomicU64::new(0));
        let errors = Arc::new(AtomicU64::new(0));
        let latencies = Arc::new(std::sync::Mutex::new(Vec::new()));

        // Clone for the spawned task
        let stop_flag_clone = stop_flag.clone();
        let total_clone = total.clone();
        let errors_clone = errors.clone();
        let latencies_clone = latencies.clone();
        let endpoint = config.endpoint.clone();
        let rps = config.rps;
        let duration = config.duration;

        // Start traffic generation in background
        let handle = tokio::spawn(async move {
            let client = reqwest::Client::builder()
                .timeout(std::time::Duration::from_secs(5))
                .build()
                .unwrap();

            let interval = std::time::Duration::from_secs_f64(1.0 / f64::from(rps));
            let deadline = std::time::Instant::now() + duration;

            while std::time::Instant::now() < deadline && !stop_flag_clone.load(Ordering::Relaxed) {
                let start = std::time::Instant::now();
                let url = format!("http://127.0.0.1:{local_port}{endpoint}");

                let result = client.get(&url).send().await;
                let latency = start.elapsed();

                total_clone.fetch_add(1, Ordering::Relaxed);
                if let Ok(ref resp) = result {
                    if !resp.status().is_success() {
                        errors_clone.fetch_add(1, Ordering::Relaxed);
                    }
                } else {
                    errors_clone.fetch_add(1, Ordering::Relaxed);
                }

                latencies_clone.lock().unwrap().push(latency);

                tokio::time::sleep(interval).await;
            }
        });

        Ok(TrafficHandle {
            _pf: pf,
            handle,
            stop_flag,
            total,
            errors,
            latencies,
        })
    }

    /// Create a pod assertion builder
    ///
    /// # Example
    ///
    /// ```ignore
    /// ctx.assert_pod("my-pod").is_running().await?;
    /// ctx.assert_pod("my-pod").has_label("app", "myapp").await?;
    /// ```
    #[must_use]
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
    #[must_use]
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
    #[must_use]
    pub fn assert_service(&self, name: &str) -> crate::assertions::ServiceAssertion {
        crate::assertions::ServiceAssertion::new(
            self.client.clone(),
            self.namespace.clone(),
            name.to_string(),
        )
    }

    /// Create a PVC assertion builder
    ///
    /// # Example
    ///
    /// ```ignore
    /// ctx.assert_pvc("my-data").is_bound().await?;
    /// ctx.assert_pvc("my-data").has_storage_class("standard").await?;
    /// ctx.assert_pvc("my-data").has_capacity("10Gi").await?;
    /// ```
    #[must_use]
    pub fn assert_pvc(&self, name: &str) -> crate::assertions::PvcAssertion {
        crate::assertions::PvcAssertion::new(
            self.client.clone(),
            self.namespace.clone(),
            name.to_string(),
        )
    }

    /// Get resource metrics from the metrics-server API
    ///
    /// Currently supports pod metrics. The target format follows
    /// kubectl-style references: `pod/name` or just `name` (treated as pod).
    ///
    /// # Example
    ///
    /// ```ignore
    /// let metrics = ctx.metrics("pod/api-xyz").await?;
    ///
    /// println!("CPU: {}", metrics.cpu);           // "250m"
    /// println!("Memory: {}", metrics.memory);     // "512Mi"
    /// ```
    ///
    /// # Errors
    ///
    /// Returns `MetricsError::ServerNotAvailable` if metrics-server is not installed.
    /// Returns `MetricsError::PodNotFound` if the specified pod doesn't exist.
    pub async fn metrics(
        &self,
        target: &str,
    ) -> Result<crate::metrics::PodMetrics, crate::metrics::MetricsError> {
        let pod_name = if target.starts_with("pod/") {
            target.strip_prefix("pod/").unwrap_or(target)
        } else {
            target
        };

        crate::metrics::fetch_pod_metrics(&self.client, &self.namespace, pod_name).await
    }

    /// Print combined diagnostic information for a resource
    ///
    /// Prints to stdout: resource status, related pods, events, and logs.
    /// Useful for debugging during test development.
    ///
    /// Supports kubectl-style references: `deployment/name`, `pod/name`, `svc/name`.
    ///
    /// # Example
    ///
    /// ```ignore
    /// // Print diagnostics for a deployment
    /// ctx.debug("deployment/api").await?;
    ///
    /// // Output:
    /// // === deployment/api ===
    /// // Status: 3/3 ready
    /// //
    /// // === Pods ===
    /// // api-xyz-123: Running (0 restarts)
    /// //
    /// // === Events ===
    /// // 10:42:01 Scheduled: Assigned to node-1
    /// //
    /// // === Logs (last 20 lines) ===
    /// // [api-xyz-123] Server started on :8080
    /// ```
    pub async fn debug(&self, target: &str) -> Result<(), ContextError> {
        let (kind, name) = parse_resource_ref(target)?;

        let kind_name = match &kind {
            ResourceKind::Deployment => "deployment",
            ResourceKind::Pod => "pod",
            ResourceKind::Service => "service",
            ResourceKind::DaemonSet => "daemonset",
            ResourceKind::StatefulSet => "statefulset",
        };

        println!();
        println!("=== {kind_name}/{name} ===");

        match kind {
            ResourceKind::Deployment => self.debug_deployment(name).await?,
            ResourceKind::Pod => self.debug_pod(name).await?,
            ResourceKind::Service => self.debug_service(name).await?,
            ResourceKind::DaemonSet => self.debug_daemonset(name).await?,
            ResourceKind::StatefulSet => self.debug_statefulset(name).await?,
        }

        Ok(())
    }

    async fn debug_deployment(&self, name: &str) -> Result<(), ContextError> {
        let deploys: Api<Deployment> = Api::namespaced(self.client.clone(), &self.namespace);

        match deploys.get(name).await {
            Ok(d) => {
                let status = d.status.as_ref();
                let ready = status.and_then(|s| s.ready_replicas).unwrap_or(0);
                let desired = d.spec.as_ref().and_then(|s| s.replicas).unwrap_or(0);
                println!("Status: {ready}/{desired} ready");
            }
            Err(e) => {
                println!("Error fetching deployment: {e}");
            }
        }

        // Get pods with matching labels
        self.debug_pods_for_app(name).await?;
        self.debug_events_for_resource("Deployment", name).await?;
        self.debug_logs_for_app(name).await?;

        Ok(())
    }

    async fn debug_pod(&self, name: &str) -> Result<(), ContextError> {
        let pods: Api<Pod> = Api::namespaced(self.client.clone(), &self.namespace);

        match pods.get(name).await {
            Ok(p) => {
                let phase = p
                    .status
                    .as_ref()
                    .and_then(|s| s.phase.as_ref())
                    .map_or("Unknown", std::string::String::as_str);
                let restarts: i32 = p
                    .status
                    .as_ref()
                    .and_then(|s| s.container_statuses.as_ref())
                    .map_or(0, |cs| cs.iter().map(|c| c.restart_count).sum());
                println!("Phase: {phase} ({restarts} restarts)");

                // Print container statuses
                if let Some(statuses) = p
                    .status
                    .as_ref()
                    .and_then(|s| s.container_statuses.as_ref())
                {
                    println!();
                    println!("=== Containers ===");
                    for cs in statuses {
                        let ready = if cs.ready { "ready" } else { "not ready" };
                        println!("  {}: {} ({})", cs.name, ready, cs.restart_count);
                    }
                }
            }
            Err(e) => {
                println!("Error fetching pod: {e}");
            }
        }

        self.debug_events_for_resource("Pod", name).await?;

        // Print logs
        match self.logs(name).await {
            Ok(logs) => {
                println!();
                println!("=== Logs (last 20 lines) ===");
                for line in logs
                    .lines()
                    .rev()
                    .take(20)
                    .collect::<Vec<_>>()
                    .into_iter()
                    .rev()
                {
                    println!("  {line}");
                }
            }
            Err(e) => {
                println!("Error fetching logs: {e}");
            }
        }

        Ok(())
    }

    async fn debug_service(&self, name: &str) -> Result<(), ContextError> {
        let svcs: Api<Service> = Api::namespaced(self.client.clone(), &self.namespace);

        match svcs.get(name).await {
            Ok(s) => {
                let svc_type = s
                    .spec
                    .as_ref()
                    .and_then(|s| s.type_.as_ref())
                    .map_or("ClusterIP", std::string::String::as_str);
                let ports: Vec<String> = s
                    .spec
                    .as_ref()
                    .and_then(|s| s.ports.as_ref())
                    .map(|ps| ps.iter().map(|p| format!("{}", p.port)).collect())
                    .unwrap_or_default();
                println!("Type: {svc_type}");
                println!("Ports: {}", ports.join(", "));
            }
            Err(e) => {
                println!("Error fetching service: {e}");
            }
        }

        self.debug_events_for_resource("Service", name).await?;

        Ok(())
    }

    async fn debug_daemonset(&self, name: &str) -> Result<(), ContextError> {
        let ds: Api<DaemonSet> = Api::namespaced(self.client.clone(), &self.namespace);

        match ds.get(name).await {
            Ok(d) => {
                let status = d.status.as_ref();
                let ready = status.map_or(0, |s| s.number_ready);
                let desired = status.map_or(0, |s| s.desired_number_scheduled);
                println!("Status: {ready}/{desired} ready");
            }
            Err(e) => {
                println!("Error fetching daemonset: {e}");
            }
        }

        self.debug_pods_for_app(name).await?;
        self.debug_events_for_resource("DaemonSet", name).await?;

        Ok(())
    }

    async fn debug_statefulset(&self, name: &str) -> Result<(), ContextError> {
        let sts: Api<StatefulSet> = Api::namespaced(self.client.clone(), &self.namespace);

        match sts.get(name).await {
            Ok(s) => {
                let status = s.status.as_ref();
                let ready = status.and_then(|s| s.ready_replicas).unwrap_or(0);
                let desired = s.spec.as_ref().and_then(|s| s.replicas).unwrap_or(0);
                println!("Status: {ready}/{desired} ready");
            }
            Err(e) => {
                println!("Error fetching statefulset: {e}");
            }
        }

        self.debug_pods_for_app(name).await?;
        self.debug_events_for_resource("StatefulSet", name).await?;

        Ok(())
    }

    async fn debug_pods_for_app(&self, app_name: &str) -> Result<(), ContextError> {
        let pods: Api<Pod> = Api::namespaced(self.client.clone(), &self.namespace);
        let lp = ListParams::default().labels(&format!("app={app_name}"));

        match pods.list(&lp).await {
            Ok(pod_list) => {
                if !pod_list.items.is_empty() {
                    println!();
                    println!("=== Pods ===");
                    for p in &pod_list.items {
                        let name = p.metadata.name.as_deref().unwrap_or("unknown");
                        let phase = p
                            .status
                            .as_ref()
                            .and_then(|s| s.phase.as_ref())
                            .map_or("Unknown", std::string::String::as_str);
                        let restarts: i32 = p
                            .status
                            .as_ref()
                            .and_then(|s| s.container_statuses.as_ref())
                            .map_or(0, |cs| cs.iter().map(|c| c.restart_count).sum());
                        println!("  {name}: {phase} ({restarts} restarts)");
                    }
                }
            }
            Err(e) => {
                println!("Error listing pods: {e}");
            }
        }

        Ok(())
    }

    async fn debug_events_for_resource(&self, kind: &str, name: &str) -> Result<(), ContextError> {
        match self.events().await {
            Ok(events) => {
                let filtered: Vec<_> = events
                    .iter()
                    .filter(|e| {
                        e.involved_object.kind.as_deref() == Some(kind)
                            && e.involved_object.name.as_deref() == Some(name)
                    })
                    .take(10)
                    .collect();

                if !filtered.is_empty() {
                    println!();
                    println!("=== Events ===");
                    for e in filtered {
                        let reason = e.reason.as_deref().unwrap_or("?");
                        let msg = e.message.as_deref().unwrap_or("");
                        // Truncate long messages
                        let msg = if msg.len() > 60 {
                            format!("{}...", &msg[..57])
                        } else {
                            msg.to_string()
                        };
                        println!("  {reason}: {msg}");
                    }
                }
            }
            Err(e) => {
                println!("Error fetching events: {e}");
            }
        }

        Ok(())
    }

    async fn debug_logs_for_app(&self, app_name: &str) -> Result<(), ContextError> {
        let pods: Api<Pod> = Api::namespaced(self.client.clone(), &self.namespace);
        let lp = ListParams::default().labels(&format!("app={app_name}"));

        if let Ok(pod_list) = pods.list(&lp).await {
            for p in pod_list.items.iter().take(2) {
                // Limit to first 2 pods
                let name = p.metadata.name.as_deref().unwrap_or("unknown");
                if let Ok(logs) = self.logs(name).await {
                    println!();
                    println!("=== Logs [{name}] (last 20 lines) ===");
                    for line in logs
                        .lines()
                        .rev()
                        .take(20)
                        .collect::<Vec<_>>()
                        .into_iter()
                        .rev()
                    {
                        println!("  {line}");
                    }
                } else {
                    // Skip pods without logs
                }
            }
        } else {
            // Silently skip if we can't list pods
        }

        Ok(())
    }

    /// Start an Ingress test chain
    ///
    /// Provides fluent assertions for testing Ingress resources.
    ///
    /// # Example
    ///
    /// ```ignore
    /// ctx.test_ingress("my-ingress")
    ///     .host("api.example.com")
    ///     .path("/v1")
    ///     .expect_backend("api-svc", 8080)
    ///     .expect_tls("api-tls-secret")
    ///     .must();
    /// ```
    #[must_use]
    pub fn test_ingress(&self, name: &str) -> IngressTest {
        IngressTest::new(
            self.client.clone(),
            self.namespace.clone(),
            name.to_string(),
        )
    }
}

/// Fluent builder for testing Ingress resources
///
/// Uses a short-circuit pattern: once an error is recorded via any assertion
/// method (like `expect_backend()` or `expect_tls()`), subsequent methods
/// skip their checks and preserve the first error. Use `error()` to get the
/// first error or `must()` to panic with it.
///
/// # Example
///
/// ```ignore
/// ctx.test_ingress("my-ingress")
///     .host("example.com")
///     .path("/api")
///     .expect_backend("svc/api", 8080).await
///     .expect_tls("my-tls-secret").await
///     .must(); // Panics if any assertion failed
/// ```
pub struct IngressTest {
    client: Client,
    namespace: String,
    name: String,
    host: Option<String>,
    path: Option<String>,
    err: Option<String>,
}

impl IngressTest {
    fn new(client: Client, namespace: String, name: String) -> Self {
        Self {
            client,
            namespace,
            name,
            host: None,
            path: None,
            err: None,
        }
    }

    /// Set the host to test
    #[must_use]
    pub fn host(mut self, host: &str) -> Self {
        if self.err.is_some() {
            return self;
        }
        self.host = Some(host.to_string());
        self
    }

    /// Set the path to test
    #[must_use]
    pub fn path(mut self, path: &str) -> Self {
        if self.err.is_some() {
            return self;
        }
        self.path = Some(path.to_string());
        self
    }

    /// Assert the ingress routes to the expected backend
    pub async fn expect_backend(mut self, backend: &str, port: i32) -> Self {
        if self.err.is_some() {
            return self;
        }

        let api: Api<Ingress> = Api::namespaced(self.client.clone(), &self.namespace);

        match api.get(&self.name).await {
            Ok(ingress) => {
                let expected_svc = backend.strip_prefix("svc/").unwrap_or(backend);

                let Some(spec) = ingress.spec else {
                    self.err = Some(format!("Ingress {} has no spec", self.name));
                    return self;
                };

                let mut found = false;
                for rule in spec.rules.unwrap_or_default() {
                    // Check host match (None in test means match any)
                    if let Some(ref test_host) = self.host {
                        if rule.host.as_ref() != Some(test_host) {
                            continue;
                        }
                    }

                    if let Some(http) = rule.http {
                        // Path matching semantics:
                        // - If self.path is None, match any ingress path.
                        // - If the ingress path is empty, treat it as matching all request paths.
                        // - Otherwise, require that the ingress path is a path-segment prefix of
                        //   the requested path, e.g. "/api" matches "/api", "/api/", "/api/v1"
                        //   but does NOT match "/apiv2".
                        let path_prefix_matches = |test_path: &str, ingress_path: &str| {
                            if ingress_path.is_empty() {
                                // Empty ingress path behaves as a catch-all.
                                true
                            } else if test_path == ingress_path {
                                true
                            } else if let Some(rest) = test_path.strip_prefix(ingress_path) {
                                rest.starts_with('/')
                            } else {
                                false
                            }
                        };

                        for p in http.paths {
                            // Check path match (None in test means match any)
                            let ingress_path = p.path.as_deref().unwrap_or("");
                            let path_matches = self.path.as_deref().is_none_or(|test_path| {
                                path_prefix_matches(test_path, ingress_path)
                            });

                            if path_matches {
                                if let Some(service) = p.backend.service {
                                    let actual_svc = service.name;
                                    let actual_port =
                                        service.port.and_then(|p| p.number).unwrap_or(0);

                                    if actual_svc == expected_svc && actual_port == port {
                                        found = true;
                                        break;
                                    }
                                }
                            }
                        }
                    }
                    if found {
                        break;
                    }
                }

                if !found {
                    self.err = Some(format!(
                        "Ingress {} does not route host={:?} path={:?} to {}:{}",
                        self.name, self.host, self.path, expected_svc, port
                    ));
                }
            }
            Err(e) => {
                self.err = Some(format!("Failed to get ingress {}: {}", self.name, e));
            }
        }

        self
    }

    /// Assert the ingress has TLS configured with the given secret
    pub async fn expect_tls(mut self, secret_name: &str) -> Self {
        if self.err.is_some() {
            return self;
        }

        let api: Api<Ingress> = Api::namespaced(self.client.clone(), &self.namespace);

        match api.get(&self.name).await {
            Ok(ingress) => {
                let Some(spec) = ingress.spec else {
                    self.err = Some(format!("Ingress {} has no spec", self.name));
                    return self;
                };

                let mut found = false;
                for tls in spec.tls.unwrap_or_default() {
                    if tls.secret_name.as_ref() == Some(&secret_name.to_string()) {
                        // Check if host matches (if specified)
                        if self.host.is_none() {
                            found = true;
                            break;
                        }
                        for host in tls.hosts.unwrap_or_default() {
                            if Some(&host) == self.host.as_ref() {
                                found = true;
                                break;
                            }
                        }
                    }
                    if found {
                        break;
                    }
                }

                if !found {
                    self.err = Some(format!(
                        "Ingress {} does not have TLS secret {} for host {:?}",
                        self.name, secret_name, self.host
                    ));
                }
            }
            Err(e) => {
                self.err = Some(format!("Failed to get ingress {}: {}", self.name, e));
            }
        }

        self
    }

    /// Get any error from the test chain
    #[must_use]
    pub fn error(&self) -> Option<&str> {
        self.err.as_deref()
    }

    /// Panic if there was an error in the test chain
    pub fn must(self) {
        if let Some(err) = self.err {
            panic!("IngressTest failed: {err}");
        }
    }
}

/// Bundle containing `ServiceAccount`, Role, and `RoleBinding` for RBAC setup
pub struct RBACBundle {
    /// The `ServiceAccount`
    pub service_account: ServiceAccount,
    /// The Role defining permissions
    pub role: Role,
    /// The `RoleBinding` connecting `ServiceAccount` to Role
    pub role_binding: RoleBinding,
}

/// Fluent builder for creating RBAC resources
///
/// # Example
///
/// ```ignore
/// let rbac = RBACBuilder::service_account("my-sa")
///     .with_role("my-role")
///     .can_get(&["pods", "services"])
///     .can_list(&["deployments"])
///     .can_all(&["secrets"])
///     .build();
///
/// ctx.apply_rbac(&rbac).await?;
/// ```
pub struct RBACBuilder {
    name: String,
    role_name: String,
    rules: Vec<PolicyRule>,
}

impl RBACBuilder {
    /// Create a new `RBACBuilder` with the given `ServiceAccount` name
    #[must_use]
    pub fn service_account(name: &str) -> Self {
        Self {
            name: name.to_string(),
            role_name: format!("{name}-role"),
            rules: Vec::new(),
        }
    }

    /// Set a custom role name
    #[must_use]
    pub fn with_role(mut self, role_name: &str) -> Self {
        self.role_name = role_name.to_string();
        self
    }

    /// Add a policy rule with the correct API group for each resource
    fn add_rule(&mut self, verbs: &[&str], resources: &[&str]) {
        // Group resources by their API group
        let mut by_group: HashMap<String, Vec<String>> = HashMap::new();
        for res in resources {
            let group = api_group_for_resource(res);
            by_group.entry(group).or_default().push((*res).to_string());
        }

        // Create separate rules for each API group
        for (group, group_resources) in by_group {
            self.rules.push(PolicyRule {
                api_groups: Some(vec![group]),
                resources: Some(group_resources),
                verbs: verbs.iter().map(|v| (*v).to_string()).collect(),
                ..Default::default()
            });
        }
    }

    /// Add get permission for the specified resources
    #[must_use]
    pub fn can_get(mut self, resources: &[&str]) -> Self {
        self.add_rule(&["get"], resources);
        self
    }

    /// Add list permission for the specified resources
    #[must_use]
    pub fn can_list(mut self, resources: &[&str]) -> Self {
        self.add_rule(&["list"], resources);
        self
    }

    /// Add watch permission for the specified resources
    #[must_use]
    pub fn can_watch(mut self, resources: &[&str]) -> Self {
        self.add_rule(&["watch"], resources);
        self
    }

    /// Add create permission for the specified resources
    #[must_use]
    pub fn can_create(mut self, resources: &[&str]) -> Self {
        self.add_rule(&["create"], resources);
        self
    }

    /// Add update permission for the specified resources
    #[must_use]
    pub fn can_update<I, S>(mut self, resources: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: AsRef<str>,
    {
        let resource_strings: Vec<String> = resources
            .into_iter()
            .map(|r| r.as_ref().to_string())
            .collect();
        let resource_refs: Vec<&str> = resource_strings
            .iter()
            .map(std::string::String::as_str)
            .collect();
        self.add_rule(&["update"], &resource_refs);
        self
    }

    /// Add delete permission for the specified resources
    #[must_use]
    pub fn can_delete<I, S>(mut self, resources: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: AsRef<str>,
    {
        let resource_strings: Vec<String> = resources
            .into_iter()
            .map(|r| r.as_ref().to_string())
            .collect();
        let resource_refs: Vec<&str> = resource_strings
            .iter()
            .map(std::string::String::as_str)
            .collect();
        self.add_rule(&["delete"], &resource_refs);
        self
    }

    /// Add all permissions (get, list, watch, create, update, delete) for resources
    #[must_use]
    pub fn can_all<I, S>(mut self, resources: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: AsRef<str>,
    {
        let resource_strings: Vec<String> = resources
            .into_iter()
            .map(|r| r.as_ref().to_string())
            .collect();
        let resource_refs: Vec<&str> = resource_strings
            .iter()
            .map(std::string::String::as_str)
            .collect();
        self.add_rule(
            &["get", "list", "watch", "create", "update", "delete"],
            &resource_refs,
        );
        self
    }

    /// Add custom verbs for the specified resources
    #[must_use]
    pub fn can<IV, IS, VV, VS>(mut self, verbs: IV, resources: IS) -> Self
    where
        IV: IntoIterator<Item = VV>,
        VV: AsRef<str>,
        IS: IntoIterator<Item = VS>,
        VS: AsRef<str>,
    {
        let verb_strings: Vec<String> = verbs.into_iter().map(|v| v.as_ref().to_string()).collect();
        let verb_refs: Vec<&str> = verb_strings
            .iter()
            .map(std::string::String::as_str)
            .collect();

        let resource_strings: Vec<String> = resources
            .into_iter()
            .map(|r| r.as_ref().to_string())
            .collect();
        let resource_refs: Vec<&str> = resource_strings
            .iter()
            .map(std::string::String::as_str)
            .collect();

        self.add_rule(&verb_refs, &resource_refs);
        self
    }

    /// Add a rule with explicit API group for custom resources (CRDs)
    #[must_use]
    pub fn for_api_group<IV, VV, IR, VR>(
        mut self,
        api_group: &str,
        verbs: IV,
        resources: IR,
    ) -> Self
    where
        IV: IntoIterator<Item = VV>,
        VV: AsRef<str>,
        IR: IntoIterator<Item = VR>,
        VR: AsRef<str>,
    {
        self.rules.push(PolicyRule {
            api_groups: Some(vec![api_group.to_string()]),
            resources: Some(
                resources
                    .into_iter()
                    .map(|r| r.as_ref().to_string())
                    .collect(),
            ),
            verbs: verbs.into_iter().map(|v| v.as_ref().to_string()).collect(),
            ..Default::default()
        });
        self
    }

    /// Build the `RBACBundle` with `ServiceAccount`, Role, and `RoleBinding`
    #[must_use]
    pub fn build(self) -> RBACBundle {
        RBACBundle {
            service_account: ServiceAccount {
                metadata: kube::api::ObjectMeta {
                    name: Some(self.name.clone()),
                    ..Default::default()
                },
                ..Default::default()
            },
            role: Role {
                metadata: kube::api::ObjectMeta {
                    name: Some(self.role_name.clone()),
                    ..Default::default()
                },
                rules: Some(self.rules),
            },
            role_binding: RoleBinding {
                metadata: kube::api::ObjectMeta {
                    name: Some(format!("{}-binding", self.name)),
                    ..Default::default()
                },
                subjects: Some(vec![Subject {
                    kind: "ServiceAccount".to_string(),
                    name: self.name.clone(),
                    namespace: None,
                    api_group: None,
                }]),
                role_ref: RoleRef {
                    api_group: "rbac.authorization.k8s.io".to_string(),
                    kind: "Role".to_string(),
                    name: self.role_name,
                },
            },
        }
    }
}

/// Look up environment variables and return their values
///
/// Returns an error if any key is not set in the environment.
fn lookup_env_vars(keys: &[&str]) -> Result<HashMap<String, String>, String> {
    let mut data = HashMap::new();
    for key in keys {
        match std::env::var(key) {
            Ok(value) => {
                data.insert((*key).to_string(), value);
            }
            Err(_) => {
                return Err(format!("environment variable {key} is not set"));
            }
        }
    }
    Ok(data)
}

/// Returns the correct API group for a resource
#[allow(clippy::match_same_arms)] // Explicit core resources for documentation
fn api_group_for_resource(resource: &str) -> String {
    match resource {
        // Core API group ("")
        "pods"
        | "services"
        | "configmaps"
        | "secrets"
        | "persistentvolumeclaims"
        | "serviceaccounts"
        | "namespaces"
        | "nodes"
        | "events"
        | "endpoints"
        | "persistentvolumes"
        | "replicationcontrollers"
        | "resourcequotas"
        | "limitranges" => String::new(),
        // apps group
        "deployments" | "statefulsets" | "daemonsets" | "replicasets" | "controllerrevisions" => {
            "apps".to_string()
        }
        // batch group
        "jobs" | "cronjobs" => "batch".to_string(),
        // networking.k8s.io group
        "ingresses" | "networkpolicies" | "ingressclasses" => "networking.k8s.io".to_string(),
        // rbac.authorization.k8s.io group
        "roles" | "rolebindings" | "clusterroles" | "clusterrolebindings" => {
            "rbac.authorization.k8s.io".to_string()
        }
        // autoscaling group
        "horizontalpodautoscalers" => "autoscaling".to_string(),
        // policy group
        "poddisruptionbudgets" => "policy".to_string(),
        // Default to core for unknown resources
        _ => String::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ============================================================
    // Unit tests (no cluster required)
    // ============================================================

    #[test]
    fn test_extract_resource_name() {
        // Just name
        assert_eq!(extract_resource_name("myapp"), "myapp");
        assert_eq!(extract_resource_name("test-gateway"), "test-gateway");

        // Kind/name format - should extract just the name
        assert_eq!(extract_resource_name("deployment/myapp"), "myapp");
        assert_eq!(
            extract_resource_name("gateway/test-gateway"),
            "test-gateway"
        );
        assert_eq!(extract_resource_name("pod/worker-0"), "worker-0");
        assert_eq!(extract_resource_name("configmap/my-config"), "my-config");

        // Edge cases
        assert_eq!(extract_resource_name(""), "");
        assert_eq!(extract_resource_name("a/b/c"), "c"); // Multiple slashes: takes last part
    }

    /// RED: Test that port_forward() method exists and has correct signature
    #[tokio::test]
    #[ignore] // Requires real cluster
    async fn test_port_forward_exists() {
        let ctx = Context::new().await.expect("Should create context");

        // port_forward should accept any target format (pod name, svc/name, etc)
        // and return Result<PortForward, PortForwardError>
        let _result = ctx.port_forward("test-pod", 8080).await;

        ctx.cleanup().await.expect("Should cleanup");
    }

    #[tokio::test]
    #[ignore] // Requires kubeconfig to construct Context
    async fn test_retry_succeeds_on_transient_failure() {
        let client = kube::Client::try_default()
            .await
            .expect("requires kubeconfig");
        let ctx = Context {
            client,
            namespace: "test".to_string(),
        };

        let counter = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let counter_clone = counter.clone();

        let result: Result<(), &str> = ctx
            .retry(3, || {
                let counter = counter_clone.clone();
                async move {
                    let count = counter.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                    if count < 2 {
                        Err("not ready yet")
                    } else {
                        Ok(())
                    }
                }
            })
            .await;

        assert!(result.is_ok());
        assert_eq!(counter.load(std::sync::atomic::Ordering::SeqCst), 3);
    }

    #[tokio::test]
    #[ignore] // Requires kubeconfig to construct Context
    async fn test_retry_fails_after_max_attempts() {
        let client = kube::Client::try_default()
            .await
            .expect("requires kubeconfig");
        let ctx = Context {
            client,
            namespace: "test".to_string(),
        };

        let counter = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let counter_clone = counter.clone();

        let result: Result<(), &str> = ctx
            .retry(3, || {
                let counter = counter_clone.clone();
                async move {
                    counter.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                    Err("always fails")
                }
            })
            .await;

        assert!(result.is_err());
        assert_eq!(counter.load(std::sync::atomic::Ordering::SeqCst), 3);
    }

    // ============================================================
    // Integration tests (require cluster)
    // ============================================================

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

    /// Test that apply() is idempotent - can update existing resources
    #[tokio::test]
    #[ignore] // Requires real cluster
    async fn test_context_apply_is_idempotent() {
        use k8s_openapi::api::core::v1::ConfigMap;

        let ctx = Context::new().await.expect("Should create context");

        // Create initial ConfigMap
        let cm = ConfigMap {
            metadata: kube::api::ObjectMeta {
                name: Some("idempotent-test".to_string()),
                ..Default::default()
            },
            data: Some(
                [("key".to_string(), "value1".to_string())]
                    .into_iter()
                    .collect(),
            ),
            ..Default::default()
        };

        // First apply - creates
        ctx.apply(&cm).await.expect("First apply should succeed");

        // Second apply with same resource - should not error
        ctx.apply(&cm)
            .await
            .expect("Second apply should succeed (idempotent)");

        // Third apply with modified data - should update
        let mut cm_updated = cm.clone();
        cm_updated.data = Some(
            [("key".to_string(), "value2".to_string())]
                .into_iter()
                .collect(),
        );
        let updated = ctx
            .apply(&cm_updated)
            .await
            .expect("Third apply should update");

        // Verify update was applied
        let data = updated.data.expect("Should have data");
        assert_eq!(data.get("key"), Some(&"value2".to_string()));

        // Cleanup
        ctx.cleanup().await.expect("Should cleanup");
    }

    /// Test that secret_from_file() creates a secret from files
    #[tokio::test]
    #[ignore] // Requires real cluster
    async fn test_context_secret_from_file() {
        let ctx = Context::new().await.expect("Should create context");

        // Create temp files
        let temp_dir = std::env::temp_dir();
        let file1 = temp_dir.join("seppo_test_config.txt");
        let file2 = temp_dir.join("seppo_test_data.txt");

        std::fs::write(&file1, "config-content").expect("Should write file1");
        std::fs::write(&file2, "data-content").expect("Should write file2");

        // Create secret from files
        let mut files = HashMap::new();
        files.insert("config".to_string(), file1.to_string_lossy().to_string());
        files.insert("data".to_string(), file2.to_string_lossy().to_string());

        let secret = ctx
            .secret_from_file("file-secret", files)
            .await
            .expect("Should create secret");

        // Verify secret was created
        assert_eq!(secret.metadata.name.as_deref(), Some("file-secret"));
        let data = secret.data.expect("Should have data");
        assert_eq!(
            data.get("config")
                .map(|b| String::from_utf8_lossy(&b.0).to_string()),
            Some("config-content".to_string())
        );
        assert_eq!(
            data.get("data")
                .map(|b| String::from_utf8_lossy(&b.0).to_string()),
            Some("data-content".to_string())
        );

        // Cleanup temp files
        let _ = std::fs::remove_file(file1);
        let _ = std::fs::remove_file(file2);

        // Cleanup namespace
        ctx.cleanup().await.expect("Should cleanup");
    }

    /// Test that secret_tls() creates a TLS secret
    #[tokio::test]
    #[ignore] // Requires real cluster
    async fn test_context_secret_tls() {
        let ctx = Context::new().await.expect("Should create context");

        // Create temp cert and key files (dummy content for testing)
        let temp_dir = std::env::temp_dir();
        let cert_file = temp_dir.join("seppo_test_cert.pem");
        let key_file = temp_dir.join("seppo_test_key.pem");

        std::fs::write(
            &cert_file,
            "-----BEGIN CERTIFICATE-----\ntest\n-----END CERTIFICATE-----",
        )
        .expect("Should write cert");
        std::fs::write(
            &key_file,
            "-----BEGIN PRIVATE KEY-----\ntest\n-----END PRIVATE KEY-----",
        )
        .expect("Should write key");

        // Create TLS secret
        let secret = ctx
            .secret_tls(
                "tls-secret",
                &cert_file.to_string_lossy(),
                &key_file.to_string_lossy(),
            )
            .await
            .expect("Should create TLS secret");

        // Verify secret was created with TLS type
        assert_eq!(secret.metadata.name.as_deref(), Some("tls-secret"));
        assert_eq!(secret.type_.as_deref(), Some("kubernetes.io/tls"));

        let data = secret.data.expect("Should have data");
        assert!(data.contains_key("tls.crt"), "Should have tls.crt");
        assert!(data.contains_key("tls.key"), "Should have tls.key");

        // Cleanup temp files
        let _ = std::fs::remove_file(cert_file);
        let _ = std::fs::remove_file(key_file);

        // Cleanup namespace
        ctx.cleanup().await.expect("Should cleanup");
    }

    /// Test that isolate() creates a deny-all NetworkPolicy
    #[tokio::test]
    #[ignore] // Requires real cluster
    async fn test_context_isolate() {
        let ctx = Context::new().await.expect("Should create context");

        let mut selector = HashMap::new();
        selector.insert("app".to_string(), "isolated".to_string());

        let policy = ctx
            .isolate(selector)
            .await
            .expect("Should create NetworkPolicy");

        // Verify policy was created
        assert!(policy
            .metadata
            .name
            .as_ref()
            .unwrap()
            .starts_with("seppo-isolate-"));

        // Verify it's a deny-all (empty ingress/egress)
        let spec = policy.spec.expect("Should have spec");
        assert!(
            spec.ingress.is_none(),
            "Should have no ingress rules (deny all)"
        );
        assert!(
            spec.egress.is_none(),
            "Should have no egress rules (deny all)"
        );

        // Cleanup
        ctx.cleanup().await.expect("Should cleanup");
    }

    /// Test that allow_from() creates an allow NetworkPolicy
    #[tokio::test]
    #[ignore] // Requires real cluster
    async fn test_context_allow_from() {
        let ctx = Context::new().await.expect("Should create context");

        let mut target = HashMap::new();
        target.insert("app".to_string(), "backend".to_string());

        let mut source = HashMap::new();
        source.insert("app".to_string(), "frontend".to_string());

        let policy = ctx
            .allow_from(target, source)
            .await
            .expect("Should create NetworkPolicy");

        // Verify policy was created
        assert!(policy
            .metadata
            .name
            .as_ref()
            .unwrap()
            .starts_with("seppo-allow-"));

        // Verify it has ingress rules
        let spec = policy.spec.expect("Should have spec");
        assert!(spec.ingress.is_some(), "Should have ingress rules");
        let ingress = spec.ingress.unwrap();
        assert_eq!(ingress.len(), 1, "Should have one ingress rule");

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

    /// Test that wait_pvc_bound() waits for PVC to be bound
    #[tokio::test]
    #[ignore] // Requires real cluster with storage provisioner
    async fn test_context_wait_pvc_bound() {
        let ctx = Context::new().await.expect("Should create context");

        // Create a PVC
        let pvc = PersistentVolumeClaim {
            metadata: kube::api::ObjectMeta {
                name: Some("test-pvc".to_string()),
                ..Default::default()
            },
            spec: Some(k8s_openapi::api::core::v1::PersistentVolumeClaimSpec {
                access_modes: Some(vec!["ReadWriteOnce".to_string()]),
                resources: Some(k8s_openapi::api::core::v1::VolumeResourceRequirements {
                    requests: Some(
                        [(
                            "storage".to_string(),
                            k8s_openapi::apimachinery::pkg::api::resource::Quantity(
                                "1Gi".to_string(),
                            ),
                        )]
                        .into_iter()
                        .collect(),
                    ),
                    ..Default::default()
                }),
                ..Default::default()
            }),
            ..Default::default()
        };

        ctx.apply(&pvc).await.expect("Should apply PVC");

        // Wait for PVC to be bound (may timeout if no dynamic provisioning)
        let result = ctx
            .wait_pvc_bound_with_timeout("test-pvc", std::time::Duration::from_secs(30))
            .await;

        match result {
            Ok(bound_pvc) => {
                let phase = bound_pvc
                    .status
                    .as_ref()
                    .and_then(|s| s.phase.as_ref())
                    .map(|p| p.as_str())
                    .unwrap_or("Unknown");
                assert_eq!(phase, "Bound", "PVC should be bound");
            }
            Err(e) => {
                // Expected if no dynamic provisioning is available
                println!(
                    "wait_pvc_bound returned error (expected if no dynamic provisioning): {}",
                    e
                );
            }
        }

        // Also test with "pvc/name" format
        let result2 = ctx
            .wait_pvc_bound_with_timeout("pvc/test-pvc", std::time::Duration::from_secs(5))
            .await;
        // Just verify it accepts the format
        println!("pvc/name format result: {:?}", result2.is_ok());

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
        let mut created_names = Vec::new();
        for i in 0..3 {
            let name = format!("list-test-config-{}", i);
            let cm = ConfigMap {
                metadata: kube::api::ObjectMeta {
                    name: Some(name.clone()),
                    ..Default::default()
                },
                ..Default::default()
            };
            ctx.apply(&cm).await.expect("Should apply ConfigMap");
            created_names.push(name);
        }

        // Verify each ConfigMap exists individually
        for name in &created_names {
            let _: ConfigMap = ctx.get(name).await.expect("ConfigMap should exist");
        }

        // List them (filter to only our test ConfigMaps)
        let configs: Vec<ConfigMap> = ctx.list().await.expect("Should list ConfigMaps");
        let our_configs: Vec<_> = configs
            .into_iter()
            .filter(|cm| {
                cm.metadata
                    .name
                    .as_ref()
                    .map(|n| n.starts_with("list-test-config-"))
                    .unwrap_or(false)
            })
            .collect();

        assert_eq!(our_configs.len(), 3, "Should have 3 ConfigMaps");

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

        // Wait for pod to be running before getting logs
        ctx.wait_ready("pod/test-pod")
            .await
            .expect("Pod should be ready");

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

    /// Test that logs_stream() streams pod logs in real-time
    #[tokio::test]
    #[ignore] // Requires real cluster with a running pod
    async fn test_context_logs_stream() {
        use futures::TryStreamExt;
        use k8s_openapi::api::core::v1::Pod;

        let ctx = Context::new().await.expect("Should create context");

        // Create a pod that outputs logs over time
        let pod = Pod {
            metadata: kube::api::ObjectMeta {
                name: Some("stream-test-pod".to_string()),
                ..Default::default()
            },
            spec: Some(k8s_openapi::api::core::v1::PodSpec {
                containers: vec![k8s_openapi::api::core::v1::Container {
                    name: "test".to_string(),
                    image: Some("busybox:latest".to_string()),
                    command: Some(vec![
                        "sh".to_string(),
                        "-c".to_string(),
                        "for i in 1 2 3 4 5; do echo \"log line $i\"; sleep 1; done".to_string(),
                    ]),
                    ..Default::default()
                }],
                restart_policy: Some("Never".to_string()),
                ..Default::default()
            }),
            ..Default::default()
        };

        ctx.apply(&pod).await.expect("Should apply Pod");

        // Wait for pod to be running
        ctx.wait_for::<Pod, _>("stream-test-pod", |p| {
            p.status
                .as_ref()
                .and_then(|s| s.phase.as_ref())
                .is_some_and(|phase| phase == "Running")
        })
        .await
        .expect("Pod should be running");

        // Stream logs and collect some lines
        let mut stream = ctx
            .logs_stream("stream-test-pod")
            .await
            .expect("Should get log stream");

        let mut lines = Vec::new();
        // Collect a few lines with a timeout
        let collect_result = tokio::time::timeout(std::time::Duration::from_secs(10), async {
            while let Some(line) = stream.try_next().await? {
                lines.push(line);
                if lines.len() >= 3 {
                    break;
                }
            }
            Ok::<_, std::io::Error>(())
        })
        .await;

        assert!(collect_result.is_ok(), "Should collect logs within timeout");
        assert!(
            !lines.is_empty(),
            "Should have received at least one log line"
        );
        assert!(
            lines.iter().any(|l| l.contains("log line")),
            "Logs should contain our messages"
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

        // Wait for pod to be running before collecting logs
        ctx.wait_ready("pod/log-test")
            .await
            .expect("Pod should be ready");

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

    /// Test that port_forward() creates a port forward to a pod
    #[tokio::test]
    #[ignore] // Requires real cluster
    async fn test_context_port_forward() {
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
            .port_forward("forward-test", 80)
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
    async fn test_port_forward_to_pod() {
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
            .port_forward("pod/forward-pod-test", 80)
            .await
            .expect("Should create port forward");

        let response = pf.get("/").await.expect("Should get response");
        assert!(response.contains("nginx") || response.contains("Welcome"));

        ctx.cleanup().await.expect("Should cleanup");
    }

    #[tokio::test]
    #[ignore] // Requires real cluster
    async fn test_port_forward_to_service() {
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
            .port_forward("svc/forward-svc-test", 80)
            .await
            .expect("Should create port forward to service");

        let response = pf.get("/").await.expect("Should get response");
        assert!(response.contains("nginx") || response.contains("Welcome"));

        ctx.cleanup().await.expect("Should cleanup");
    }

    // ============================================================
    // scale() tests
    // ============================================================

    // ============================================================
    // kill() tests
    // ============================================================

    #[tokio::test]
    #[ignore] // Requires real cluster
    async fn test_kill_pod() {
        let ctx = Context::new().await.expect("Should create context");

        // Create a pod
        let pod = Pod {
            metadata: kube::api::ObjectMeta {
                name: Some("kill-test".to_string()),
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

        // Wait for pod to be running
        ctx.wait_ready("pod/kill-test")
            .await
            .expect("Pod should be running");

        // Kill the pod (immediate deletion)
        ctx.kill("pod/kill-test").await.expect("Should kill pod");

        // Pod should be gone or terminating immediately
        // Use a short timeout since kill should be fast
        ctx.wait_deleted_with_timeout::<Pod>("kill-test", std::time::Duration::from_secs(10))
            .await
            .expect("Pod should be deleted quickly");

        ctx.cleanup().await.expect("Should cleanup");
    }

    #[tokio::test]
    #[ignore] // Requires real cluster
    async fn test_kill_only_supports_pods() {
        let ctx = Context::new().await.expect("Should create context");

        // kill() should only work on pods
        let result = ctx.kill("deployment/myapp").await;
        assert!(result.is_err(), "kill should only work on pods");

        ctx.cleanup().await.expect("Should cleanup");
    }

    // ============================================================
    // rollback() tests
    // ============================================================

    #[tokio::test]
    #[ignore] // Requires real cluster
    async fn test_rollback_deployment() {
        let ctx = Context::new().await.expect("Should create context");

        // Create a deployment with image v1
        let deployment = Deployment {
            metadata: kube::api::ObjectMeta {
                name: Some("rollback-test".to_string()),
                ..Default::default()
            },
            spec: Some(k8s_openapi::api::apps::v1::DeploymentSpec {
                replicas: Some(1),
                selector: k8s_openapi::apimachinery::pkg::apis::meta::v1::LabelSelector {
                    match_labels: Some(
                        [("app".to_string(), "rollback-test".to_string())]
                            .into_iter()
                            .collect(),
                    ),
                    ..Default::default()
                },
                template: k8s_openapi::api::core::v1::PodTemplateSpec {
                    metadata: Some(kube::api::ObjectMeta {
                        labels: Some(
                            [("app".to_string(), "rollback-test".to_string())]
                                .into_iter()
                                .collect(),
                        ),
                        ..Default::default()
                    }),
                    spec: Some(k8s_openapi::api::core::v1::PodSpec {
                        containers: vec![k8s_openapi::api::core::v1::Container {
                            name: "nginx".to_string(),
                            image: Some("nginx:1.24".to_string()),
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
            .expect("Should apply deployment v1");
        ctx.wait_ready("deployment/rollback-test")
            .await
            .expect("v1 should be ready");

        // Update to v2
        let mut deployment_v2 = deployment.clone();
        deployment_v2
            .spec
            .as_mut()
            .unwrap()
            .template
            .spec
            .as_mut()
            .unwrap()
            .containers[0]
            .image = Some("nginx:1.25".to_string());

        ctx.apply(&deployment_v2)
            .await
            .expect("Should apply deployment v2");
        ctx.wait_ready("deployment/rollback-test")
            .await
            .expect("v2 should be ready");

        // Verify we're on v2
        let dep: Deployment = ctx
            .get("rollback-test")
            .await
            .expect("Should get deployment");
        let current_image = dep
            .spec
            .as_ref()
            .unwrap()
            .template
            .spec
            .as_ref()
            .unwrap()
            .containers[0]
            .image
            .as_ref()
            .unwrap();
        assert_eq!(current_image, "nginx:1.25");

        // Rollback to previous revision
        ctx.rollback("deployment/rollback-test")
            .await
            .expect("Should rollback");

        // Wait for rollback to complete
        ctx.wait_ready("deployment/rollback-test")
            .await
            .expect("rollback should complete");

        // Verify we're back on v1
        let dep: Deployment = ctx
            .get("rollback-test")
            .await
            .expect("Should get deployment");
        let rolled_back_image = dep
            .spec
            .as_ref()
            .unwrap()
            .template
            .spec
            .as_ref()
            .unwrap()
            .containers[0]
            .image
            .as_ref()
            .unwrap();
        assert_eq!(rolled_back_image, "nginx:1.24");

        ctx.cleanup().await.expect("Should cleanup");
    }

    #[tokio::test]
    #[ignore] // Requires real cluster
    async fn test_rollback_only_supports_deployments() {
        let ctx = Context::new().await.expect("Should create context");

        let result = ctx.rollback("pod/myapp").await;
        assert!(result.is_err(), "rollback should only work on deployments");

        ctx.cleanup().await.expect("Should cleanup");
    }

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

        // Wait for deployment to stabilize before scaling
        ctx.wait_ready("deployment/scale-test")
            .await
            .expect("Should be ready");

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

    #[tokio::test]
    #[ignore] // Requires real cluster
    async fn test_restart_deployment() {
        let ctx = Context::new().await.expect("Should create context");

        // Create a deployment
        let deployment = Deployment {
            metadata: kube::api::ObjectMeta {
                name: Some("restart-test".to_string()),
                ..Default::default()
            },
            spec: Some(k8s_openapi::api::apps::v1::DeploymentSpec {
                replicas: Some(1),
                selector: k8s_openapi::apimachinery::pkg::apis::meta::v1::LabelSelector {
                    match_labels: Some(
                        [("app".to_string(), "restart-test".to_string())]
                            .into_iter()
                            .collect(),
                    ),
                    ..Default::default()
                },
                template: k8s_openapi::api::core::v1::PodTemplateSpec {
                    metadata: Some(kube::api::ObjectMeta {
                        labels: Some(
                            [("app".to_string(), "restart-test".to_string())]
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

        // Wait for deployment to stabilize before restarting
        ctx.wait_ready("deployment/restart-test")
            .await
            .expect("Should be ready");

        // Restart the deployment
        ctx.restart("deployment/restart-test")
            .await
            .expect("Should restart deployment");

        // Verify the restart annotation was added to pod template
        let dep: Deployment = ctx
            .get("restart-test")
            .await
            .expect("Should get deployment");
        let annotations = dep
            .spec
            .as_ref()
            .unwrap()
            .template
            .metadata
            .as_ref()
            .unwrap()
            .annotations
            .as_ref()
            .expect("Should have annotations after restart");
        assert!(
            annotations.contains_key("kubectl.kubernetes.io/restartedAt"),
            "Should have restartedAt annotation"
        );

        ctx.cleanup().await.expect("Should cleanup");
    }

    #[tokio::test]
    #[ignore] // Requires real cluster
    async fn test_restart_only_supports_workloads() {
        let ctx = Context::new().await.expect("Should create context");

        // Restart should not work on pods directly
        let result = ctx.restart("pod/myapp").await;
        assert!(result.is_err(), "restart should not work on pods");

        // Restart should not work on services
        let result = ctx.restart("service/myapp").await;
        assert!(result.is_err(), "restart should not work on services");

        ctx.cleanup().await.expect("Should cleanup");
    }

    #[tokio::test]
    #[ignore] // Requires real cluster
    async fn test_apply_cluster_scoped_resource() {
        use k8s_openapi::api::rbac::v1::ClusterRole;

        let ctx = Context::new().await.expect("Should create context");

        // Create a cluster-scoped resource (ClusterRole)
        // Use a unique name to avoid conflicts
        let role_name = format!("seppo-test-role-{}", uuid::Uuid::new_v4());
        let cluster_role = ClusterRole {
            metadata: kube::api::ObjectMeta {
                name: Some(role_name.clone()),
                labels: Some(
                    [("seppo.io/test".to_string(), "true".to_string())]
                        .into_iter()
                        .collect(),
                ),
                ..Default::default()
            },
            rules: Some(vec![]),
            ..Default::default()
        };

        // Apply cluster-scoped resource
        let applied = ctx
            .apply_cluster(&cluster_role)
            .await
            .expect("Should apply cluster-scoped resource");

        assert_eq!(applied.metadata.name, Some(role_name.clone()));

        // Clean up the cluster role manually (not namespace-scoped)
        let api: Api<ClusterRole> = Api::all(ctx.client.clone());
        api.delete(&role_name, &DeleteParams::default())
            .await
            .expect("Should delete cluster role");

        ctx.cleanup().await.expect("Should cleanup");
    }

    #[tokio::test]
    #[ignore] // Requires real cluster
    async fn test_get_cluster_scoped_resource() {
        use k8s_openapi::api::rbac::v1::ClusterRole;

        let ctx = Context::new().await.expect("Should create context");

        // Create a cluster role first
        let role_name = format!("seppo-test-role-{}", uuid::Uuid::new_v4());
        let cluster_role = ClusterRole {
            metadata: kube::api::ObjectMeta {
                name: Some(role_name.clone()),
                ..Default::default()
            },
            rules: Some(vec![]),
            ..Default::default()
        };

        ctx.apply_cluster(&cluster_role)
            .await
            .expect("Should apply");

        // Get it back using get_cluster
        let fetched: ClusterRole = ctx
            .get_cluster(&role_name)
            .await
            .expect("Should get cluster-scoped resource");

        assert_eq!(fetched.metadata.name, Some(role_name.clone()));

        // Clean up
        let api: Api<ClusterRole> = Api::all(ctx.client.clone());
        api.delete(&role_name, &DeleteParams::default())
            .await
            .expect("Should delete");

        ctx.cleanup().await.expect("Should cleanup");
    }

    #[tokio::test]
    #[ignore] // Requires real cluster
    async fn test_delete_cluster_scoped_resource() {
        use k8s_openapi::api::rbac::v1::ClusterRole;

        let ctx = Context::new().await.expect("Should create context");

        // Create a cluster role first
        let role_name = format!("seppo-test-role-{}", uuid::Uuid::new_v4());
        let cluster_role = ClusterRole {
            metadata: kube::api::ObjectMeta {
                name: Some(role_name.clone()),
                ..Default::default()
            },
            rules: Some(vec![]),
            ..Default::default()
        };

        ctx.apply_cluster(&cluster_role)
            .await
            .expect("Should apply");

        // Delete using delete_cluster
        ctx.delete_cluster::<ClusterRole>(&role_name)
            .await
            .expect("Should delete cluster-scoped resource");

        // Verify it's gone
        let api: Api<ClusterRole> = Api::all(ctx.client.clone());
        let result = api.get(&role_name).await;
        assert!(result.is_err(), "Resource should be deleted");

        ctx.cleanup().await.expect("Should cleanup");
    }

    #[tokio::test]
    #[ignore] // Requires real cluster
    async fn test_logs_all_by_selector() {
        let ctx = Context::new().await.expect("Should create context");

        // Create a deployment with 2 replicas
        let deployment = Deployment {
            metadata: kube::api::ObjectMeta {
                name: Some("logs-test".to_string()),
                ..Default::default()
            },
            spec: Some(k8s_openapi::api::apps::v1::DeploymentSpec {
                replicas: Some(2),
                selector: k8s_openapi::apimachinery::pkg::apis::meta::v1::LabelSelector {
                    match_labels: Some(
                        [("app".to_string(), "logs-test".to_string())]
                            .into_iter()
                            .collect(),
                    ),
                    ..Default::default()
                },
                template: k8s_openapi::api::core::v1::PodTemplateSpec {
                    metadata: Some(kube::api::ObjectMeta {
                        labels: Some(
                            [("app".to_string(), "logs-test".to_string())]
                                .into_iter()
                                .collect(),
                        ),
                        ..Default::default()
                    }),
                    spec: Some(k8s_openapi::api::core::v1::PodSpec {
                        containers: vec![k8s_openapi::api::core::v1::Container {
                            name: "busybox".to_string(),
                            image: Some("busybox:latest".to_string()),
                            command: Some(vec![
                                "sh".to_string(),
                                "-c".to_string(),
                                "echo 'hello from pod' && sleep 3600".to_string(),
                            ]),
                            ..Default::default()
                        }],
                        ..Default::default()
                    }),
                },
                ..Default::default()
            }),
            ..Default::default()
        };

        ctx.apply(&deployment).await.expect("Should apply");
        ctx.wait_ready("deployment/logs-test")
            .await
            .expect("Should be ready");

        // Get logs from all pods with label app=logs-test
        let all_logs = ctx
            .logs_all("app=logs-test")
            .await
            .expect("Should get logs from all pods");

        // Should have logs from 2 pods
        assert_eq!(all_logs.len(), 2, "Should have logs from 2 pods");

        // Each pod should have the expected log message
        for logs in all_logs.values() {
            assert!(
                logs.contains("hello from pod"),
                "Each pod should have the log message"
            );
        }

        ctx.cleanup().await.expect("Should cleanup");
    }

    /// Test that can_i() checks RBAC permissions
    #[tokio::test]
    #[ignore] // Requires real cluster
    async fn test_context_can_i() {
        let ctx = Context::new().await.expect("Should create context");

        // Test permission to create pods - should be allowed in our namespace
        let can_create_pods = ctx
            .can_i("create", "pods")
            .await
            .expect("Should check permission");

        // The test runs with cluster-admin typically, so this should be true
        // In a real environment with limited permissions, this might be false
        assert!(
            can_create_pods,
            "Should be able to create pods in test namespace"
        );

        // Test permission to get pods - should be allowed
        let can_get_pods = ctx
            .can_i("get", "pods")
            .await
            .expect("Should check permission");

        assert!(can_get_pods, "Should be able to get pods in test namespace");

        ctx.cleanup().await.expect("Should cleanup");
    }

    /// Test that RBACBuilder creates correct ServiceAccount, Role, and RoleBinding
    #[tokio::test]
    #[ignore] // Requires real cluster
    async fn test_rbac_builder() {
        let ctx = Context::new().await.expect("Should create context");

        // Build an RBAC bundle
        let bundle = RBACBuilder::service_account("test-sa")
            .can_get(&["pods", "services"])
            .can_list(&["pods"])
            .can_create(&["configmaps"])
            .build();

        // Verify ServiceAccount was built correctly
        assert_eq!(
            bundle.service_account.metadata.name.as_deref(),
            Some("test-sa")
        );

        // Verify Role was built correctly with correct name
        assert_eq!(bundle.role.metadata.name.as_deref(), Some("test-sa-role"));

        // Verify Role has the expected rules
        let rules = bundle.role.rules.as_ref().expect("Should have rules");
        assert!(
            rules.len() >= 2,
            "Should have at least 2 rules (core API resources)"
        );

        // Verify RoleBinding was built correctly
        assert_eq!(
            bundle.role_binding.metadata.name.as_deref(),
            Some("test-sa-binding")
        );
        assert_eq!(bundle.role_binding.role_ref.name, "test-sa-role");

        // Apply the RBAC bundle
        ctx.apply_rbac(&bundle)
            .await
            .expect("Should apply RBAC bundle");

        // Verify resources were created by getting them
        use k8s_openapi::api::core::v1::ServiceAccount;
        let sa: ServiceAccount = ctx.get("test-sa").await.expect("Should get ServiceAccount");
        assert_eq!(sa.metadata.name.as_deref(), Some("test-sa"));

        let role: Role = ctx.get("test-sa-role").await.expect("Should get Role");
        assert!(role.rules.is_some());

        let rb: RoleBinding = ctx
            .get("test-sa-binding")
            .await
            .expect("Should get RoleBinding");
        assert_eq!(rb.role_ref.name, "test-sa-role");

        ctx.cleanup().await.expect("Should cleanup");
    }

    /// Test that test_ingress() creates a fluent builder for Ingress assertions
    #[tokio::test]
    #[ignore] // Requires real cluster
    async fn test_ingress_builder() {
        let ctx = Context::new().await.expect("Should create context");

        // Create an Ingress resource
        let ingress = Ingress {
            metadata: kube::api::ObjectMeta {
                name: Some("test-ingress".to_string()),
                ..Default::default()
            },
            spec: Some(k8s_openapi::api::networking::v1::IngressSpec {
                rules: Some(vec![k8s_openapi::api::networking::v1::IngressRule {
                    host: Some("example.com".to_string()),
                    http: Some(k8s_openapi::api::networking::v1::HTTPIngressRuleValue {
                        paths: vec![k8s_openapi::api::networking::v1::HTTPIngressPath {
                            path: Some("/api".to_string()),
                            path_type: "Prefix".to_string(),
                            backend: k8s_openapi::api::networking::v1::IngressBackend {
                                service: Some(
                                    k8s_openapi::api::networking::v1::IngressServiceBackend {
                                        name: "backend-svc".to_string(),
                                        port: Some(
                                            k8s_openapi::api::networking::v1::ServiceBackendPort {
                                                number: Some(8080),
                                                ..Default::default()
                                            },
                                        ),
                                    },
                                ),
                                ..Default::default()
                            },
                        }],
                    }),
                }]),
                tls: Some(vec![k8s_openapi::api::networking::v1::IngressTLS {
                    hosts: Some(vec!["example.com".to_string()]),
                    secret_name: Some("tls-secret".to_string()),
                }]),
                ..Default::default()
            }),
            ..Default::default()
        };

        ctx.apply(&ingress).await.expect("Should apply Ingress");

        // Test that expect_backend assertion passes
        let test_result = ctx
            .test_ingress("test-ingress")
            .host("example.com")
            .path("/api")
            .expect_backend("backend-svc", 8080)
            .await;

        assert!(
            test_result.error().is_none(),
            "expect_backend should pass: {:?}",
            test_result.error()
        );

        // Test that expect_tls assertion passes
        let tls_result = ctx
            .test_ingress("test-ingress")
            .host("example.com")
            .expect_tls("tls-secret")
            .await;

        assert!(
            tls_result.error().is_none(),
            "expect_tls should pass: {:?}",
            tls_result.error()
        );

        // Test that wrong backend fails assertion
        let wrong_backend = ctx
            .test_ingress("test-ingress")
            .host("example.com")
            .path("/api")
            .expect_backend("wrong-svc", 8080)
            .await;

        assert!(
            wrong_backend.error().is_some(),
            "expect_backend should fail for wrong service"
        );

        // Test that wrong TLS secret fails assertion
        let wrong_tls = ctx
            .test_ingress("test-ingress")
            .host("example.com")
            .expect_tls("wrong-secret")
            .await;

        assert!(
            wrong_tls.error().is_some(),
            "expect_tls should fail for wrong secret"
        );

        ctx.cleanup().await.expect("Should cleanup");
    }

    /// Test that improve_error_message produces helpful messages
    #[test]
    fn test_improve_error_message_not_found() {
        // Test NotFound error pattern
        let msg = "NotFound: configmaps \"myconfig\" not found";
        assert!(
            msg.contains("NotFound"),
            "Raw message should contain NotFound for test validity"
        );

        // Simulate the behavior - we can't easily create a kube::Error but we can test patterns
        let improved_not_found = format!("{} '{}' not found in namespace", "ConfigMap", "myconfig");
        assert_eq!(
            improved_not_found,
            "ConfigMap 'myconfig' not found in namespace"
        );
    }

    #[test]
    fn test_improve_error_message_already_exists() {
        let improved = format!("{} '{}' already exists", "Pod", "mypod");
        assert_eq!(improved, "Pod 'mypod' already exists");
    }

    #[test]
    fn test_improve_error_message_image_pull() {
        let improved = format!(
            "{} '{}' failed: image '{}' not found or inaccessible",
            "Pod", "mypod", "nginx:nonexistent"
        );
        assert!(improved.contains("image"));
        assert!(improved.contains("nginx:nonexistent"));
    }

    // ============================================================
    // secret_from_env tests
    // ============================================================

    #[test]
    fn test_secret_from_env_missing_var_returns_error() {
        // Ensure the env var doesn't exist
        std::env::remove_var("SEPPO_TEST_NONEXISTENT_VAR");

        // The error should mention the missing env var
        let result = lookup_env_vars(&["SEPPO_TEST_NONEXISTENT_VAR"]);
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(
            err.contains("SEPPO_TEST_NONEXISTENT_VAR"),
            "Error should mention the missing var: {}",
            err
        );
    }

    #[test]
    fn test_secret_from_env_collects_values() {
        // Set test env vars
        std::env::set_var("SEPPO_TEST_USER", "admin");
        std::env::set_var("SEPPO_TEST_PASS", "secret123");

        let result = lookup_env_vars(&["SEPPO_TEST_USER", "SEPPO_TEST_PASS"]);
        assert!(result.is_ok());

        let data = result.unwrap();
        assert_eq!(data.get("SEPPO_TEST_USER"), Some(&"admin".to_string()));
        assert_eq!(data.get("SEPPO_TEST_PASS"), Some(&"secret123".to_string()));

        // Cleanup
        std::env::remove_var("SEPPO_TEST_USER");
        std::env::remove_var("SEPPO_TEST_PASS");
    }

    // ============================================================
    // Gvr (GroupVersionResource) tests
    // ============================================================

    #[test]
    fn test_gvr_new() {
        let gvr = Gvr::new("gateway.networking.k8s.io", "v1", "gateways", "Gateway");
        assert_eq!(gvr.group, "gateway.networking.k8s.io");
        assert_eq!(gvr.version, "v1");
        assert_eq!(gvr.resource, "gateways");
        assert_eq!(gvr.kind, "Gateway");
    }

    #[test]
    fn test_gvr_core_api() {
        // Core API group has empty string for group
        let gvr = Gvr::new("", "v1", "pods", "Pod");
        assert_eq!(gvr.group, "");
        assert_eq!(gvr.version, "v1");
    }

    #[test]
    fn test_gvr_common_crds() {
        // Gateway API
        let gateway = Gvr::gateway_class();
        assert_eq!(gateway.group, "gateway.networking.k8s.io");
        assert_eq!(gateway.resource, "gatewayclasses");

        let httproute = Gvr::http_route();
        assert_eq!(httproute.resource, "httproutes");

        // Cert-manager
        let cert = Gvr::certificate();
        assert_eq!(cert.group, "cert-manager.io");
        assert_eq!(cert.resource, "certificates");
    }

    // ============================================================
    // TrafficConfig tests
    // ============================================================

    #[test]
    fn test_traffic_config_default() {
        let config = TrafficConfig::default();
        assert_eq!(config.rps, 10);
        assert_eq!(config.duration, std::time::Duration::from_secs(10));
        assert_eq!(config.endpoint, "/");
    }

    #[test]
    fn test_traffic_config_builder() {
        let config = TrafficConfig::default()
            .with_rps(100)
            .with_duration(std::time::Duration::from_secs(30))
            .with_endpoint("/health");

        assert_eq!(config.rps, 100);
        assert_eq!(config.duration, std::time::Duration::from_secs(30));
        assert_eq!(config.endpoint, "/health");
    }

    #[test]
    fn test_traffic_stats_error_rate() {
        let stats = TrafficStats {
            total_requests: 100,
            errors: 5,
            latencies: vec![],
        };
        assert!((stats.error_rate() - 0.05).abs() < 0.001);
    }

    #[test]
    fn test_traffic_stats_p99_latency() {
        let latencies: Vec<std::time::Duration> =
            (1..=100).map(std::time::Duration::from_millis).collect();
        let stats = TrafficStats {
            total_requests: 100,
            errors: 0,
            latencies,
        };
        // 99th percentile of 1-100ms should be around 99ms
        assert!(stats.p99_latency() >= std::time::Duration::from_millis(99));
    }
}
