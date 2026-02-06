use super::parsing::parse_forward_target;
use super::types::ForwardTarget;
use super::{Context, ContextError};
use crate::portforward::{PortForward, PortForwardError};
use k8s_openapi::api::apps::v1::Deployment;
use k8s_openapi::api::core::v1::{Pod, Service};
use kube::api::{Api, ListParams};
use tracing::debug;

impl Context {
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
}
