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
