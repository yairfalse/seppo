use super::parsing::parse_resource_ref;
use super::types::ResourceKind;
use super::{Context, ContextError};
use k8s_openapi::api::apps::v1::{DaemonSet, Deployment, ReplicaSet, StatefulSet};
use k8s_openapi::api::authorization::v1::{
    ResourceAttributes, SelfSubjectAccessReview, SelfSubjectAccessReviewSpec,
};
use k8s_openapi::api::core::v1::Pod;
use kube::api::{Api, DeleteParams, ListParams, ObjectMeta, Patch, PatchParams, PostParams};
use tracing::{debug, info};

impl Context {
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
}
