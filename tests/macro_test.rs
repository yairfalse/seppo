//! Integration tests using the #[seppo::test] macro

use k8s_openapi::api::core::v1::ConfigMap;
#[allow(unused_imports)] // Used in macro-expanded function signatures
use seppo::Context;

/// Test using the #[seppo::test] macro
/// The macro automatically:
/// - Creates an isolated namespace
/// - Injects Context as `ctx`
/// - Cleans up on success
/// - Keeps namespace on failure for debugging
#[seppo::test]
#[ignore] // Requires real cluster
async fn test_macro_creates_configmap(ctx: Context) {
    // Create a ConfigMap
    let cm = ConfigMap {
        metadata: kube::api::ObjectMeta {
            name: Some("macro-test".to_string()),
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
}

/// Test that the macro handles panics and keeps namespace
#[seppo::test]
#[ignore] // Requires real cluster
async fn test_macro_keeps_namespace_on_failure(ctx: Context) {
    // Create something first
    let cm = ConfigMap {
        metadata: kube::api::ObjectMeta {
            name: Some("before-panic".to_string()),
            ..Default::default()
        },
        ..Default::default()
    };
    ctx.apply(&cm).await.expect("Should apply");

    // This will panic - namespace should be kept for debugging
    // Uncomment to test failure behavior:
    // panic!("Intentional failure to test namespace preservation");

    // For normal test runs, just verify the ConfigMap exists
    let _: ConfigMap = ctx.get("before-panic").await.expect("Should exist");
}

/// Test without ctx parameter - should just work as regular async test
#[seppo::test]
async fn test_macro_without_ctx() {
    // No ctx needed - macro just wraps with #[tokio::test]
    let x = 1 + 1;
    assert_eq!(x, 2);
}
