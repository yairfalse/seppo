//! Minikube cluster provider
//!
//! Creates and manages Minikube clusters.

use async_trait::async_trait;
use std::process::Command;
use tracing::{debug, info, instrument};

use super::{ClusterProvider, ProviderError};
use crate::config::ClusterConfig;

/// Minikube cluster provider
pub struct MinikubeProvider;

impl MinikubeProvider {
    pub fn new() -> Self {
        Self
    }
}

impl Default for MinikubeProvider {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl ClusterProvider for MinikubeProvider {
    #[instrument(skip(self), fields(cluster_name = %config.name, provider = "minikube"))]
    async fn create(&self, config: &ClusterConfig) -> Result<(), ProviderError> {
        // Check if cluster already exists
        if self.exists(&config.name).await? {
            debug!("Cluster {} already exists, reusing", config.name);
            return Ok(());
        }

        info!("Creating Minikube cluster: {}", config.name);

        let mut cmd = Command::new("minikube");
        cmd.args(["start", "--profile", &config.name]);

        // Add driver if specified
        if let Some(ref driver) = config.driver {
            cmd.args(["--driver", driver]);
        }

        // Add K8s version if specified
        if let Some(ref version) = config.k8s_version {
            cmd.args(["--kubernetes-version", &format!("v{}", version)]);
        }

        // Add nodes (workers + 1 control plane)
        if config.workers > 0 {
            cmd.args(["--nodes", &(config.workers + 1).to_string()]);
        }

        let output = cmd.output()
            .map_err(|e| ProviderError::CommandFailed(e.to_string()))?;

        if !output.status.success() {
            return Err(ProviderError::CreateFailed(
                String::from_utf8_lossy(&output.stderr).to_string()
            ));
        }

        info!("Cluster {} created successfully", config.name);
        Ok(())
    }

    #[instrument(skip(self), fields(cluster_name = %name, provider = "minikube"))]
    async fn delete(&self, name: &str) -> Result<(), ProviderError> {
        info!("Deleting Minikube cluster: {}", name);

        let output = Command::new("minikube")
            .args(["delete", "--profile", name])
            .output()
            .map_err(|e| ProviderError::CommandFailed(e.to_string()))?;

        if !output.status.success() {
            return Err(ProviderError::DeleteFailed(
                String::from_utf8_lossy(&output.stderr).to_string()
            ));
        }

        debug!("Cluster {} deleted", name);
        Ok(())
    }

    #[instrument(skip(self), fields(cluster_name = %cluster, image = %image, provider = "minikube"))]
    async fn load_image(&self, cluster: &str, image: &str) -> Result<(), ProviderError> {
        debug!("Loading image {} into cluster {}", image, cluster);

        let output = Command::new("minikube")
            .args(["image", "load", image, "--profile", cluster])
            .output()
            .map_err(|e| ProviderError::CommandFailed(e.to_string()))?;

        if !output.status.success() {
            return Err(ProviderError::ImageLoadFailed(
                String::from_utf8_lossy(&output.stderr).to_string()
            ));
        }

        Ok(())
    }

    async fn exists(&self, name: &str) -> Result<bool, ProviderError> {
        let output = Command::new("minikube")
            .args(["profile", "list", "-o", "json"])
            .output()
            .map_err(|e| ProviderError::CommandFailed(e.to_string()))?;

        let stdout = String::from_utf8_lossy(&output.stdout);

        // Simple check - if profile name appears in output
        Ok(stdout.contains(&format!("\"{}\"", name)))
    }

    async fn kubeconfig(&self, _name: &str) -> Result<String, ProviderError> {
        // Minikube uses ~/.kube/config with context set to profile name
        let home = std::env::var("HOME")
            .map_err(|_| ProviderError::CommandFailed("HOME not set".to_string()))?;

        Ok(format!("{}/.kube/config", home))
    }

    fn name(&self) -> &'static str {
        "minikube"
    }
}
