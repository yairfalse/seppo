//! Kind cluster management for integration testing
//!
//! Provides functions for creating, deleting, and managing Kind clusters
//! for Kubernetes controller and application testing.

use std::process::Command;

/// Create a kind cluster with specified name and configuration
///
/// # Examples
///
/// ```no_run
/// use seppo::cluster;
///
/// #[tokio::main]
/// async fn main() -> Result<(), Box<dyn std::error::Error>> {
///     cluster::create("my-test-cluster").await?;
///     Ok(())
/// }
/// ```
pub async fn create(name: &str) -> Result<(), Box<dyn std::error::Error>> {
    println!("🔧 Creating kind cluster: {}", name);

    // Check if cluster already exists
    if cluster_exists(name)? {
        println!("✅ Cluster {} already exists, reusing", name);
        return Ok(());
    }

    // Create cluster with custom config
    let config = format!(
        r#"
kind: Cluster
apiVersion: kind.x-k8s.io/v1alpha4
nodes:
- role: control-plane
- role: worker
- role: worker
networking:
  disableDefaultCNI: false
  podSubnet: "10.244.0.0/16"
"#
    );

    std::fs::write("/tmp/kind-config.yaml", config)?;

    let output = Command::new("kind")
        .args(&[
            "create",
            "cluster",
            "--name",
            name,
            "--config",
            "/tmp/kind-config.yaml",
        ])
        .output()?;

    if !output.status.success() {
        return Err(format!(
            "Failed to create cluster: {}",
            String::from_utf8_lossy(&output.stderr)
        )
        .into());
    }

    // Install Gateway API CRDs (if needed for your tests)
    install_gateway_api_crds().await?;

    println!("✅ Cluster {} created successfully", name);
    Ok(())
}

/// Delete a kind cluster
///
/// # Examples
///
/// ```no_run
/// use seppo::cluster;
///
/// #[tokio::main]
/// async fn main() -> Result<(), Box<dyn std::error::Error>> {
///     cluster::delete("my-test-cluster").await?;
///     Ok(())
/// }
/// ```
pub async fn delete(name: &str) -> Result<(), Box<dyn std::error::Error>> {
    println!("🗑️  Deleting kind cluster: {}", name);

    let output = Command::new("kind")
        .args(&["delete", "cluster", "--name", name])
        .output()?;

    if !output.status.success() {
        return Err(format!(
            "Failed to delete cluster: {}",
            String::from_utf8_lossy(&output.stderr)
        )
        .into());
    }

    println!("✅ Cluster {} deleted", name);
    Ok(())
}

/// Check if a kind cluster exists
fn cluster_exists(name: &str) -> Result<bool, Box<dyn std::error::Error>> {
    let output = Command::new("kind").args(&["get", "clusters"]).output()?;

    let clusters = String::from_utf8(output.stdout)?;
    Ok(clusters.lines().any(|line| line.trim() == name))
}

/// Install Gateway API CRDs into the cluster
///
/// This is optional but useful for testing Gateway API controllers.
async fn install_gateway_api_crds() -> Result<(), Box<dyn std::error::Error>> {
    println!("📦 Installing Gateway API CRDs");

    let output = Command::new("kubectl")
        .args(&[
            "apply",
            "-f",
            "https://github.com/kubernetes-sigs/gateway-api/releases/download/v1.0.0/standard-install.yaml",
        ])
        .output()?;

    if !output.status.success() {
        return Err(format!(
            "Failed to install Gateway API CRDs: {}",
            String::from_utf8_lossy(&output.stderr)
        )
        .into());
    }

    Ok(())
}

/// Load a Docker image into the kind cluster
///
/// # Examples
///
/// ```no_run
/// use seppo::cluster;
///
/// #[tokio::main]
/// async fn main() -> Result<(), Box<dyn std::error::Error>> {
///     cluster::create("test-cluster").await?;
///     cluster::load_image("test-cluster", "my-app:latest").await?;
///     Ok(())
/// }
/// ```
pub async fn load_image(
    cluster_name: &str,
    image: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    println!("📦 Loading image {} into cluster {}", image, cluster_name);

    let output = Command::new("kind")
        .args(&["load", "docker-image", image, "--name", cluster_name])
        .output()?;

    if !output.status.success() {
        return Err(format!(
            "Failed to load image: {}",
            String::from_utf8_lossy(&output.stderr)
        )
        .into());
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    #[ignore] // Run manually: cargo test -- --ignored
    async fn test_create_and_delete_cluster() {
        let cluster_name = "seppo-test-cluster";

        // Create cluster
        create(cluster_name).await.expect("Failed to create cluster");

        // Verify it exists
        assert!(
            cluster_exists(cluster_name).unwrap(),
            "Cluster should exist after creation"
        );

        // Delete cluster
        delete(cluster_name).await.expect("Failed to delete cluster");

        // Verify it's gone
        assert!(
            !cluster_exists(cluster_name).unwrap(),
            "Cluster should not exist after deletion"
        );
    }
}
