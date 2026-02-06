use k8s_openapi::api::core::v1::ServiceAccount;
use k8s_openapi::api::rbac::v1::{PolicyRule, Role, RoleBinding, RoleRef, Subject};
use std::collections::HashMap;

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
pub(crate) fn lookup_env_vars(keys: &[&str]) -> Result<HashMap<String, String>, String> {
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
pub(crate) fn api_group_for_resource(resource: &str) -> String {
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
