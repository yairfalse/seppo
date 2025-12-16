//! Kind cluster management for integration testing
//!
//! Provides simple functions for creating, deleting, and managing Kind clusters.
//! For more control, use the `provider` module directly.
//!
//! **Note**: This module uses `KindProvider` internally. For multi-provider
//! support (Kind, Minikube, Existing), use `seppo::provider::get_provider()`.

use std::process::Command;
use tracing::info;

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
    info!("Installing Gateway API CRDs");

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
pub async fn load_image(cluster_name: &str, image: &str) -> Result<(), Box<dyn std::error::Error>> {
    let provider = KindProvider::new();
    provider.load_image(cluster_name, image).await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Check if Kind can create clusters in this environment
    /// Returns false on systems where Docker uses cgroup v2 with systemd driver
    /// which has known issues with Kind cluster creation
    fn kind_cluster_creation_supported() -> bool {
        // Check Docker's cgroup configuration
        let output = std::process::Command::new("docker")
            .args(["info", "--format", "{{.CgroupDriver}} {{.CgroupVersion}}"])
            .output();

        match output {
            Ok(o) if o.status.success() => {
                let info = String::from_utf8_lossy(&o.stdout);
                // Known problematic: systemd + cgroup v2 on some systems
                // If we detect this, check if we're on a known-broken setup
                if info.contains("systemd") && info.contains("2") {
                    // Check kernel version - Ubuntu 24.04 with kernel 6.8+ has issues
                    if let Ok(uname) = std::process::Command::new("uname").arg("-r").output() {
                        let kernel = String::from_utf8_lossy(&uname.stdout);
                        // Parse kernel version and check if >= 6.8
                        let version_str = kernel.trim().split('-').next().unwrap_or("").trim();
                        let mut parts = version_str.split('.');
                        let major = parts.next().and_then(|s| s.parse::<u32>().ok()).unwrap_or(0);
                        let minor = parts.next().and_then(|s| s.parse::<u32>().ok()).unwrap_or(0);
                        if major > 6 || (major == 6 && minor >= 8) {
                            return std::env::var("SEPPO_FORCE_KIND_TEST").is_ok();
                        }
                    }
                }
                true
            }
            _ => false,
        }
    }

    #[tokio::test]
    #[ignore] // Run manually: cargo test -- --ignored
    async fn test_create_and_delete_cluster() {
        if !kind_cluster_creation_supported() {
            eprintln!("Skipping test: Kind cluster creation not supported in this environment (cgroup v2/systemd issue)");
            return;
        }

        let cluster_name = "seppo-test-cluster";

        // Create cluster
        create(cluster_name)
            .await
            .expect("Failed to create cluster");

        // Verify it exists
        assert!(
            exists(cluster_name).await.unwrap(),
            "Cluster should exist after creation"
        );

        // Delete cluster
        delete(cluster_name)
            .await
            .expect("Failed to delete cluster");

        // Verify it's gone
        assert!(
            !exists(cluster_name).await.unwrap(),
            "Cluster should not exist after deletion"
        );
    }
}
