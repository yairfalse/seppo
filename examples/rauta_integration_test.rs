//! Example: Using Seppo for RAUTA Gateway API Controller testing
//!
//! This shows how RAUTA can use Seppo to:
//! 1. Create a Kind cluster
//! 2. Build and load the RAUTA Docker image
//! 3. Deploy RAUTA to the cluster
//! 4. Run integration tests
//! 5. Cleanup
//!
//! Run with: cargo run --example rauta_integration_test

use seppo::cluster;
use std::process::Command;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("RAUTA Integration Test Example");
    println!("This example shows how to use Seppo for testing.");
    println!();
    println!("In a real test, you would:");
    println!("1. cluster::create(\"rauta-test\")");
    println!("2. Build Docker image");
    println!("3. cluster::load_image(\"rauta-test\", \"rauta:test\")");
    println!("4. Deploy and run tests");
    println!("5. cluster::delete(\"rauta-test\")");

    Ok(())
}

/// Example test function (run with cargo test --example)
#[allow(dead_code)]
async fn example_test_rauta_gateway() -> Result<(), Box<dyn std::error::Error>> {
    let cluster_name = "rauta-test";

    // Step 1: Create Kind cluster
    println!("🔧 Creating Kind cluster for RAUTA testing...");
    cluster::create(cluster_name).await?;

    // Step 2: Build RAUTA Docker image
    println!("🏗️  Building RAUTA Docker image...");
    let build_status = Command::new("docker")
        .args([
            "build",
            "-t",
            "rauta:test",
            "-f",
            "docker/Dockerfile.control-local",
            ".",
        ])
        .current_dir("../rauta")
        .status()?;

    if !build_status.success() {
        return Err("Failed to build RAUTA image".into());
    }

    // Step 3: Load image into Kind cluster
    println!("📦 Loading RAUTA image into Kind cluster...");
    cluster::load_image(cluster_name, "rauta:test").await?;

    // Step 4: Deploy RAUTA
    println!("🚀 Deploying RAUTA to cluster...");
    Command::new("kubectl")
        .args(["apply", "-f", "deploy/rauta-daemonset.yaml"])
        .current_dir("../rauta")
        .status()?;

    // Step 5: Wait for RAUTA to be ready
    println!("⏳ Waiting for RAUTA pods to be ready...");
    std::thread::sleep(std::time::Duration::from_secs(10));

    // Step 6: Run your tests here
    println!("🧪 Running TLS validation tests...");

    // Create Kubernetes client
    let _client = kube::Client::try_default().await?;

    // Test 1: Deploy Gateway with valid TLS secret
    // ... (your test logic here)

    println!("✅ All tests passed!");

    // Step 7: Cleanup
    println!("🗑️  Cleaning up cluster...");
    cluster::delete(cluster_name).await?;

    Ok(())
}
