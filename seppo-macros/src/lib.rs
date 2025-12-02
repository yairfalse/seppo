//! Proc macros for seppo Kubernetes testing framework
//!
//! Provides `#[seppo::test]` attribute macro for K8s integration tests.

use proc_macro::TokenStream;
use quote::quote;
use syn::{parse_macro_input, FnArg, ItemFn, Pat, PatType, ReturnType};

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
/// # With Result Return Type
///
/// ```ignore
/// #[seppo::test]
/// async fn test_with_result(ctx: TestContext) -> Result<(), Box<dyn std::error::Error>> {
///     ctx.apply(&deployment).await?;
///     Ok(())
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
/// - `SEPPO_KEEP_ALL=true` - Never cleanup, even on success (debug mode)
///
/// Note: Namespace is always kept on failure for debugging.
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

    // Check if function returns a Result type
    let has_result_return = matches!(&input_fn.sig.output, ReturnType::Type(..));

    let output = if has_ctx_param {
        // Function expects ctx parameter - create and inject it
        let test_execution = if has_result_return {
            // Handle Result return type - convert errors to panics for test failure
            quote! {
                let test_result: Result<(), Box<dyn std::error::Error + Send + Sync>> = (async {
                    let result = #fn_block;
                    // Convert any error type to Box<dyn Error>
                    result.map_err(|e| -> Box<dyn std::error::Error + Send + Sync> { Box::new(e) })
                }).await;

                match test_result {
                    Ok(()) => Ok(()),
                    Err(e) => Err(Box::new(std::io::Error::new(
                        std::io::ErrorKind::Other,
                        format!("{}", e)
                    )) as Box<dyn std::any::Any + Send>),
                }
            }
        } else {
            // No Result return - wrap in Ok
            quote! {
                (async {
                    #fn_block
                }).await;
                Ok::<(), Box<dyn std::any::Any + Send>>(())
            }
        };

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
                    #test_execution
                })
                .catch_unwind()
                .await;

                // Flatten the result: catch_unwind returns Result<Result<(), E>, PanicInfo>
                let final_result = match result {
                    Ok(Ok(())) => Ok(()),
                    Ok(Err(e)) => Err(e),
                    Err(panic_info) => Err(panic_info),
                };

                match final_result {
                    Ok(()) => {
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
                        eprintln!();

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
