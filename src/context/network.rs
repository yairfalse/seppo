use super::parsing::label_suffix;
use super::{Context, ContextError, RBACBundle};
use k8s_openapi::api::networking::v1::{
    NetworkPolicy, NetworkPolicyIngressRule, NetworkPolicyPeer, NetworkPolicySpec,
};
use std::collections::HashMap;
use tracing::debug;

impl Context {
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
}
