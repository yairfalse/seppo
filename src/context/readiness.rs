use super::parsing::parse_resource_ref;
use super::types::ResourceKind;
use super::{Context, ContextError};
use k8s_openapi::api::apps::v1::{DaemonSet, Deployment, StatefulSet};
use k8s_openapi::api::core::v1::{Pod, Service};
use tracing::{debug, info};

impl Context {
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
}
