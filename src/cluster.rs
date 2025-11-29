//! Kind cluster management for integration testing
//!
//! Provides simple functions for creating, deleting, and managing Kind clusters.
//! For more control, use the `provider` module directly.
//!
//! **Note**: This module uses `KindProvider` internally. For multi-provider
//! support (Kind, Minikube, Existing), use `seppo::provider::get_provider()`.

use std::process::Command;

use crate::config::{ClusterConfig, ClusterProviderType};
use crate::provider::{ClusterProvider, KindProvider};

/// Create a kind cluster with specified name and default configuration
///
/// This is a convenience function that uses `KindProvider` with defaults:
/// - 2 worker nodes
/// - Default Kubernetes version
/// - Gateway API CRDs installed
///
/// For more control, use `seppo::provider::get_provider()` with a `Config`.
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
    let provider = KindProvider::new();
    let config = ClusterConfig {
        name: name.to_string(),
        provider: ClusterProviderType::Kind,
        workers: 2,
        k8s_version: None,
        driver: None,
        kubeconfig: None,
        context: None,
    };

    provider.create(&config).await?;

    // Install Gateway API CRDs (if needed for your tests)
    install_gateway_api_crds().await?;

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
    let provider = KindProvider::new();
    provider.delete(name).await?;
    Ok(())
}

/// Check if a kind cluster exists
pub async fn exists(name: &str) -> Result<bool, Box<dyn std::error::Error>> {
    let provider = KindProvider::new();
    Ok(provider.exists(name).await?)
}

/// Install Gateway API CRDs into the cluster
///
/// This is optional but useful for testing Gateway API controllers.
async fn install_gateway_api_crds() -> Result<(), Box<dyn std::error::Error>> {
    println!("Installing Gateway API CRDs");

    let output = Command::new("kubectl")
        .args([
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
    let provider = KindProvider::new();
    provider.load_image(cluster_name, image).await?;
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
            exists(cluster_name).await.unwrap(),
            "Cluster should exist after creation"
        );

        // Delete cluster
        delete(cluster_name).await.expect("Failed to delete cluster");

        // Verify it's gone
        assert!(
            !exists(cluster_name).await.unwrap(),
            "Cluster should not exist after deletion"
        );
    }
}
