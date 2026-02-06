use super::{ContextError, ForwardTarget, ResourceKind};
use std::collections::HashMap;

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
pub(crate) fn label_suffix(labels: &HashMap<String, String>) -> String {
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
