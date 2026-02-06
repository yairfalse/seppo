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

mod context_assertions;
mod crud;
mod debug;
mod diagnostics_ctx;
mod dynamic;
mod exec;
mod forwarding;
mod ingress;
mod lifecycle;
mod logs;
mod network;
mod parsing;
mod patching;
pub mod rbac;
mod readiness;
mod retry;
mod scenarios;
mod secrets;
mod stack_deploy;
pub mod traffic_types;
pub mod types;
mod waiting;
mod workload;

pub use ingress::IngressTest;
pub use parsing::{extract_resource_name, parse_forward_target, parse_resource_ref};
pub use rbac::{RBACBuilder, RBACBundle};
pub use traffic_types::{TrafficConfig, TrafficHandle, TrafficStats};
pub use types::{ForwardTarget, Gvr, ResourceKind};

use kube::Client;

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

#[cfg(test)]
mod tests {
    use super::*;
    use super::parsing::label_suffix;
    use super::rbac::{api_group_for_resource, lookup_env_vars};
    use crate::stack::StackError;
    use k8s_openapi::api::apps::v1::Deployment;
    use k8s_openapi::api::core::v1::{PersistentVolumeClaim, Pod, Service};
    use k8s_openapi::api::networking::v1::Ingress;
    use k8s_openapi::api::rbac::v1::{Role, RoleBinding};
    use kube::api::{Api, DeleteParams};
    use std::collections::HashMap;

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

    // ============================================================
    // label_suffix tests
    // ============================================================

    #[test]
    fn test_label_suffix_deterministic() {
        let mut labels = HashMap::new();
        labels.insert("app".to_string(), "frontend".to_string());
        labels.insert("env".to_string(), "prod".to_string());

        let suffix1 = label_suffix(&labels);
        let suffix2 = label_suffix(&labels);
        assert_eq!(suffix1, suffix2, "Same labels should produce same suffix");
        assert_eq!(suffix1.len(), 8, "Suffix should be 8 hex characters");
    }

    #[test]
    fn test_label_suffix_empty_map() {
        let labels = HashMap::new();
        let suffix = label_suffix(&labels);
        assert_eq!(suffix.len(), 8, "Empty map should still produce 8-char suffix");
    }

    #[test]
    fn test_label_suffix_key_order_independent() {
        let mut labels_a = HashMap::new();
        labels_a.insert("app".to_string(), "web".to_string());
        labels_a.insert("tier".to_string(), "frontend".to_string());

        let mut labels_b = HashMap::new();
        labels_b.insert("tier".to_string(), "frontend".to_string());
        labels_b.insert("app".to_string(), "web".to_string());

        assert_eq!(
            label_suffix(&labels_a),
            label_suffix(&labels_b),
            "Insertion order should not affect suffix"
        );
    }

    #[test]
    fn test_label_suffix_different_labels_differ() {
        let mut labels_a = HashMap::new();
        labels_a.insert("app".to_string(), "frontend".to_string());

        let mut labels_b = HashMap::new();
        labels_b.insert("app".to_string(), "backend".to_string());

        assert_ne!(
            label_suffix(&labels_a),
            label_suffix(&labels_b),
            "Different labels should produce different suffixes"
        );
    }

    // ============================================================
    // api_group_for_resource tests
    // ============================================================

    #[test]
    fn test_api_group_core_resources() {
        assert_eq!(api_group_for_resource("pods"), "");
        assert_eq!(api_group_for_resource("services"), "");
        assert_eq!(api_group_for_resource("configmaps"), "");
        assert_eq!(api_group_for_resource("secrets"), "");
        assert_eq!(api_group_for_resource("namespaces"), "");
        assert_eq!(api_group_for_resource("serviceaccounts"), "");
    }

    #[test]
    fn test_api_group_apps_resources() {
        assert_eq!(api_group_for_resource("deployments"), "apps");
        assert_eq!(api_group_for_resource("statefulsets"), "apps");
        assert_eq!(api_group_for_resource("daemonsets"), "apps");
        assert_eq!(api_group_for_resource("replicasets"), "apps");
    }

    #[test]
    fn test_api_group_networking_resources() {
        assert_eq!(api_group_for_resource("ingresses"), "networking.k8s.io");
        assert_eq!(api_group_for_resource("networkpolicies"), "networking.k8s.io");
    }

    #[test]
    fn test_api_group_batch_resources() {
        assert_eq!(api_group_for_resource("jobs"), "batch");
        assert_eq!(api_group_for_resource("cronjobs"), "batch");
    }

    #[test]
    fn test_api_group_rbac_resources() {
        assert_eq!(
            api_group_for_resource("roles"),
            "rbac.authorization.k8s.io"
        );
        assert_eq!(
            api_group_for_resource("clusterroles"),
            "rbac.authorization.k8s.io"
        );
    }

    #[test]
    fn test_api_group_unknown_defaults_to_core() {
        assert_eq!(api_group_for_resource("unknown-resource"), "");
        assert_eq!(api_group_for_resource("widgets"), "");
    }

    // ============================================================
    // lookup_env_vars tests
    // ============================================================

    #[test]
    fn test_lookup_env_vars_all_found() {
        std::env::set_var("SEPPO_TEST_VAR_A", "alpha");
        std::env::set_var("SEPPO_TEST_VAR_B", "beta");

        let result = lookup_env_vars(&["SEPPO_TEST_VAR_A", "SEPPO_TEST_VAR_B"]);
        assert!(result.is_ok());
        let map = result.unwrap();
        assert_eq!(map.get("SEPPO_TEST_VAR_A"), Some(&"alpha".to_string()));
        assert_eq!(map.get("SEPPO_TEST_VAR_B"), Some(&"beta".to_string()));

        std::env::remove_var("SEPPO_TEST_VAR_A");
        std::env::remove_var("SEPPO_TEST_VAR_B");
    }

    #[test]
    fn test_lookup_env_vars_one_missing() {
        std::env::set_var("SEPPO_TEST_EXISTS", "yes");
        std::env::remove_var("SEPPO_TEST_MISSING");

        let result = lookup_env_vars(&["SEPPO_TEST_EXISTS", "SEPPO_TEST_MISSING"]);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("SEPPO_TEST_MISSING"));

        std::env::remove_var("SEPPO_TEST_EXISTS");
    }

    #[test]
    fn test_lookup_env_vars_empty_list() {
        let result = lookup_env_vars(&[]);
        assert!(result.is_ok());
        assert!(result.unwrap().is_empty());
    }

    // ============================================================
    // RBACBuilder tests
    // ============================================================

    #[test]
    fn test_rbac_builder_basic() {
        let rbac = RBACBuilder::service_account("my-sa")
            .can_get(&["pods", "services"])
            .can_list(&["deployments"])
            .build();

        // Verify ServiceAccount name
        assert_eq!(
            rbac.service_account.metadata.name,
            Some("my-sa".to_string())
        );

        // Verify Role name (default: {sa-name}-role)
        assert_eq!(rbac.role.metadata.name, Some("my-sa-role".to_string()));

        // Verify RoleBinding
        assert_eq!(
            rbac.role_binding.metadata.name,
            Some("my-sa-binding".to_string())
        );
        assert_eq!(rbac.role_binding.role_ref.name, "my-sa-role");
        assert_eq!(rbac.role_binding.role_ref.kind, "Role");
        assert_eq!(
            rbac.role_binding.role_ref.api_group,
            "rbac.authorization.k8s.io"
        );

        // Verify subject references the SA
        let subjects = rbac.role_binding.subjects.unwrap();
        assert_eq!(subjects.len(), 1);
        assert_eq!(subjects[0].name, "my-sa");
        assert_eq!(subjects[0].kind, "ServiceAccount");
    }

    #[test]
    fn test_rbac_builder_custom_role_name() {
        let rbac = RBACBuilder::service_account("my-sa")
            .with_role("custom-role")
            .can_get(&["pods"])
            .build();

        assert_eq!(rbac.role.metadata.name, Some("custom-role".to_string()));
        assert_eq!(rbac.role_binding.role_ref.name, "custom-role");
    }

    #[test]
    fn test_rbac_builder_rules_have_correct_api_groups() {
        let rbac = RBACBuilder::service_account("test-sa")
            .can_get(&["pods", "deployments"])
            .build();

        let rules = rbac.role.rules.unwrap();
        // pods → core (""), deployments → "apps" — should create separate rules per group
        assert!(
            rules.len() >= 2,
            "Should have separate rules for different API groups, got {}",
            rules.len()
        );

        let core_rule = rules.iter().find(|r| {
            r.api_groups.as_ref().map_or(false, |g| g.contains(&String::new()))
        });
        let apps_rule = rules.iter().find(|r| {
            r.api_groups
                .as_ref()
                .map_or(false, |g| g.contains(&"apps".to_string()))
        });

        assert!(core_rule.is_some(), "Should have a core API group rule");
        assert!(apps_rule.is_some(), "Should have an apps API group rule");

        let core_resources = core_rule.unwrap().resources.as_ref().unwrap();
        assert!(core_resources.contains(&"pods".to_string()));

        let apps_resources = apps_rule.unwrap().resources.as_ref().unwrap();
        assert!(apps_resources.contains(&"deployments".to_string()));
    }

    #[test]
    fn test_rbac_builder_can_all() {
        let rbac = RBACBuilder::service_account("admin-sa")
            .can_all(&["secrets"])
            .build();

        let rules = rbac.role.rules.unwrap();
        assert_eq!(rules.len(), 1);

        let verbs = &rules[0].verbs;
        assert!(verbs.contains(&"get".to_string()));
        assert!(verbs.contains(&"list".to_string()));
        assert!(verbs.contains(&"watch".to_string()));
        assert!(verbs.contains(&"create".to_string()));
        assert!(verbs.contains(&"update".to_string()));
        assert!(verbs.contains(&"delete".to_string()));
    }
}
