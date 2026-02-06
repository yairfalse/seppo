//! Proc macros for seppo Kubernetes SDK
//!
//! Provides `#[seppo::test]` attribute macro for K8s integration tests.

use proc_macro::TokenStream;
use quote::quote;
use syn::{FnArg, ItemFn, Pat, PatType, ReturnType};

/// Attribute macro for Kubernetes integration tests.
///
/// Automatically creates a `Context` with an isolated namespace and
/// injects it into your test function. Handles cleanup on success and
/// keeps namespace on failure for debugging.
///
/// # Example
///
/// ```ignore
/// use seppo::Context;
/// use k8s_openapi::api::core::v1::ConfigMap;
///
/// #[seppo::test]
/// async fn test_my_app(ctx: Context) {
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
/// async fn test_with_result(ctx: Context) -> Result<(), Box<dyn std::error::Error>> {
///     ctx.apply(&deployment).await?;
///     Ok(())
/// }
/// ```
///
/// # What it does
///
/// The macro transforms your test function to:
/// 1. Create a new `Context` with isolated namespace
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
    let input_fn = syn::parse_macro_input!(item as ItemFn);
    test_impl(&input_fn).into()
}

/// Check if a function has a parameter named "ctx"
fn has_ctx_param(input_fn: &ItemFn) -> bool {
    input_fn.sig.inputs.iter().any(|arg| {
        if let FnArg::Typed(PatType { pat, .. }) = arg {
            if let Pat::Ident(ident) = pat.as_ref() {
                return ident.ident == "ctx";
            }
        }
        false
    })
}

/// Check if a function has an explicit return type (e.g., `-> Result<...>`)
fn has_result_return(input_fn: &ItemFn) -> bool {
    matches!(&input_fn.sig.output, ReturnType::Type(..))
}

/// Inner implementation that works with `proc_macro2` types for testability
fn test_impl(input_fn: &ItemFn) -> proc_macro2::TokenStream {
    let fn_name = &input_fn.sig.ident;
    let fn_block = &input_fn.block;
    let fn_vis = &input_fn.vis;
    let fn_attrs = &input_fn.attrs;

    let ctx = has_ctx_param(input_fn);
    let result_return = has_result_return(input_fn);

    if ctx {
        // Function expects ctx parameter - create and inject it
        let test_execution = if result_return {
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

                let ctx = seppo::Context::new().await
                    .unwrap_or_else(|e| panic!("Failed to create Context: {}", e));
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
                        // Collect and print diagnostics before re-panicking
                        let diag = ctx.collect_diagnostics().await;
                        eprintln!("{}", diag);

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
    }
}

#[cfg(test)]
mod tests {
    use super::{has_ctx_param, has_result_return, test_impl};
    use syn::ItemFn;

    fn parse_fn(code: &str) -> ItemFn {
        syn::parse_str(code).expect("Failed to parse test function")
    }

    #[test]
    fn test_has_ctx_param_with_ctx() {
        let f = parse_fn("async fn test_it(ctx: Context) {}");
        assert!(has_ctx_param(&f));
    }

    #[test]
    fn test_has_ctx_param_without_ctx() {
        let f = parse_fn("async fn test_it() {}");
        assert!(!has_ctx_param(&f));
    }

    #[test]
    fn test_has_ctx_param_different_name() {
        let f = parse_fn("async fn test_it(context: Context) {}");
        assert!(!has_ctx_param(&f), "Only 'ctx' name should match");
    }

    #[test]
    fn test_has_result_return_with_result() {
        let f = parse_fn(
            "async fn test_it(ctx: Context) -> Result<(), Box<dyn std::error::Error>> {}",
        );
        assert!(has_result_return(&f));
    }

    #[test]
    fn test_has_result_return_without_result() {
        let f = parse_fn("async fn test_it(ctx: Context) {}");
        assert!(!has_result_return(&f));
    }

    #[test]
    fn test_impl_with_ctx_generates_context_new() {
        let f = parse_fn("async fn test_k8s(ctx: Context) { ctx.apply(&cm).await.unwrap(); }");
        let output = test_impl(&f).to_string();

        assert!(output.contains("Context :: new"), "Should create Context");
        assert!(output.contains("tokio :: test"), "Should have tokio::test");
        assert!(output.contains("catch_unwind"), "Should wrap with catch_unwind");
        assert!(output.contains("cleanup"), "Should call cleanup on success");
        assert!(
            output.contains("collect_diagnostics"),
            "Should collect diagnostics on failure"
        );
    }

    #[test]
    fn test_impl_with_ctx_and_result_generates_error_handling() {
        let f = parse_fn(
            "async fn test_k8s(ctx: Context) -> Result<(), Box<dyn std::error::Error>> { Ok(()) }",
        );
        let output = test_impl(&f).to_string();

        assert!(
            output.contains("map_err"),
            "Should convert errors for Result return type"
        );
    }

    #[test]
    fn test_impl_without_ctx_generates_simple_wrapper() {
        let f = parse_fn("async fn test_simple() { assert!(true); }");
        let output = test_impl(&f).to_string();

        assert!(output.contains("tokio :: test"), "Should have tokio::test");
        assert!(
            !output.contains("Context :: new"),
            "Should NOT create Context without ctx param"
        );
        assert!(
            !output.contains("catch_unwind"),
            "Should NOT use catch_unwind without ctx"
        );
    }

    #[test]
    fn test_impl_preserves_function_name() {
        let f = parse_fn("async fn my_custom_test(ctx: Context) {}");
        let output = test_impl(&f).to_string();

        assert!(
            output.contains("my_custom_test"),
            "Should preserve function name"
        );
    }
}
