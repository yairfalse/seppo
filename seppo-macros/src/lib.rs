//! Proc macros for seppo Kubernetes testing framework
//!
//! Provides `#[seppo::test]` attribute macro for K8s integration tests.

use proc_macro::TokenStream;
use quote::quote;
use syn::{parse_macro_input, FnArg, ItemFn, Pat, PatType};

/// Attribute macro for Kubernetes integration tests.
///
/// Automatically creates a `TestContext` with an isolated namespace and
/// injects it into your test function. Handles cleanup on success and
/// keeps namespace on failure for debugging.
///
/// # Example
///
/// ```ignore
/// use seppo::TestContext;
/// use k8s_openapi::api::core::v1::ConfigMap;
///
/// #[seppo::test]
/// async fn test_my_app(ctx: TestContext) {
///     let cm = ConfigMap { /* ... */ };
///     ctx.apply(&cm).await.unwrap();
///
///     // Test assertions...
/// }
/// ```
///
/// # What it does
///
/// The macro transforms your test function to:
/// 1. Create a new `TestContext` with isolated namespace
/// 2. Run your test with the context
/// 3. On success: cleanup the namespace
/// 4. On failure: keep namespace for debugging
///
/// # Environment Variables
///
/// - `SEPPO_KEEP_ON_FAILURE=true` (default) - Keep namespace on test failure
/// - `SEPPO_KEEP_ALL=true` - Never cleanup (debug mode)
#[proc_macro_attribute]
pub fn test(_attr: TokenStream, item: TokenStream) -> TokenStream {
    let input_fn = parse_macro_input!(item as ItemFn);

    let fn_name = &input_fn.sig.ident;
    let fn_block = &input_fn.block;
    let fn_vis = &input_fn.vis;
    let fn_attrs = &input_fn.attrs;

    // Check if function takes a TestContext parameter named "ctx"
    let has_ctx_param = input_fn.sig.inputs.iter().any(|arg| {
        if let FnArg::Typed(PatType { pat, .. }) = arg {
            if let Pat::Ident(ident) = pat.as_ref() {
                return ident.ident == "ctx";
            }
        }
        false
    });

    let output = if has_ctx_param {
        // Function expects ctx parameter - create and inject it
        quote! {
            #(#fn_attrs)*
            #[tokio::test]
            #fn_vis async fn #fn_name() {
                use std::panic::{catch_unwind, AssertUnwindSafe};
                use futures::FutureExt;

                let ctx = seppo::TestContext::new().await
                    .unwrap_or_else(|e| panic!("Failed to create TestContext: {}", e));
                let namespace = ctx.namespace.clone();

                // Run test and catch any panics
                let result = AssertUnwindSafe(async {
                    #fn_block
                })
                .catch_unwind()
                .await;

                match result {
                    Ok(_) => {
                        // Success - cleanup unless SEPPO_KEEP_ALL is set
                        if std::env::var("SEPPO_KEEP_ALL").is_ok() {
                            eprintln!("[seppo] SEPPO_KEEP_ALL set - keeping namespace: {}", namespace);
                        } else {
                            if let Err(e) = ctx.cleanup().await {
                                eprintln!("[seppo] Warning: cleanup failed: {}", e);
                            }
                        }
                    }
                    Err(panic_info) => {
                        // Failure - keep namespace for debugging
                        eprintln!("\n[seppo] Test failed - namespace kept for debugging");
                        eprintln!("[seppo] Namespace: {}", namespace);
                        eprintln!("[seppo] Debug commands:");
                        eprintln!("  kubectl -n {} get all", namespace);
                        eprintln!("  kubectl -n {} describe pods", namespace);
                        eprintln!("  kubectl -n {} logs <pod>", namespace);
                        eprintln!("");

                        // Re-panic to fail the test
                        std::panic::resume_unwind(panic_info);
                    }
                }
            }
        }
    } else {
        // No ctx parameter - just wrap with tokio::test
        quote! {
            #(#fn_attrs)*
            #[tokio::test]
            #fn_vis async fn #fn_name() {
                #fn_block
            }
        }
    };

    output.into()
}
