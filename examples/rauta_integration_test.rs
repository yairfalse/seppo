//! Example: Using Seppo for RAUTA Gateway API Controller testing
//!
//! This shows how RAUTA can use Seppo to:
//! 1. Connect to an existing cluster (via kubeconfig)
//! 2. Deploy RAUTA to an isolated namespace
//! 3. Run integration tests
//! 4. Cleanup
//!
//! Run with: cargo run --example rauta_integration_test

use seppo::Context;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("RAUTA Integration Test Example");
    println!("This example shows how to use Seppo for testing.");
    println!();
    println!("Seppo connects via your kubeconfig (same as kubectl).");
    println!("Point kubectl at your cluster, then use Seppo.");
    println!();
    println!("In a real test, you would:");
    println!("1. Context::new() — connects and creates isolated namespace");
    println!("2. ctx.apply(&deployment) — deploy your app");
    println!("3. ctx.wait_ready(\"deployment/myapp\") — wait for readiness");
    println!("4. ctx.port_forward(\"svc/myapp\", 8080) — test via HTTP");
    println!("5. ctx.cleanup() — tear down namespace");

    Ok(())
}
