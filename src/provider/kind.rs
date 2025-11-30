//! Kind cluster provider
//!
//! Creates and manages Kind (Kubernetes in Docker) clusters.

use async_trait::async_trait;
use std::process::Command;
use tracing::{debug, info, instrument};

use super::{ClusterProvider, ProviderError};
use crate::config::ClusterConfig;

/// Kind cluster provider
pub struct KindProvider;

impl KindProvider {
    pub fn new() -> Self {
        Self
    }
}

impl Default for KindProvider {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl ClusterProvider for KindProvider {
    #[instrument(skip(self), fields(cluster_name = %config.name, provider = "kind"))]
    async fn create(&self, config: &ClusterConfig) -> Result<(), ProviderError> {
        // Check if cluster already exists
        if self.exists(&config.name).await? {
            debug!("Cluster {} already exists, reusing", config.name);
            return Ok(());
        }

        info!("Creating Kind cluster: {}", config.name);

        // Generate Kind config
        let worker_nodes: String = (0..config.workers)
            .map(|_| "- role: worker\n")
            .collect();

        let kind_config = format!(
            r#"kind: Cluster
apiVersion: kind.x-k8s.io/v1alpha4
nodes:
- role: control-plane
{}networking:
  disableDefaultCNI: false
  podSubnet: "10.244.0.0/16"
"#,
            worker_nodes
        );

        // Write config to temp file
        let config_path = "/tmp/seppo-kind-config.yaml";
        std::fs::write(config_path, &kind_config)
            .map_err(|e| ProviderError::CreateFailed(e.to_string()))?;

        // Build command
        let mut cmd = Command::new("kind");
        cmd.args(["create", "cluster", "--name", &config.name, "--config", config_path]);

        // Add K8s version if specified
        if let Some(ref version) = config.k8s_version {
            cmd.args(["--image", &format!("kindest/node:v{}", version)]);
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

    #[instrument(skip(self), fields(cluster_name = %name, provider = "kind"))]
    async fn delete(&self, name: &str) -> Result<(), ProviderError> {
        info!("Deleting Kind cluster: {}", name);

        let output = Command::new("kind")
            .args(["delete", "cluster", "--name", name])
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

    #[instrument(skip(self), fields(cluster_name = %cluster, image = %image, provider = "kind"))]
    async fn load_image(&self, cluster: &str, image: &str) -> Result<(), ProviderError> {
        debug!("Loading image {} into cluster {}", image, cluster);

        let output = Command::new("kind")
            .args(["load", "docker-image", image, "--name", cluster])
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
        let output = Command::new("kind")
            .args(["get", "clusters"])
            .output()
            .map_err(|e| ProviderError::CommandFailed(e.to_string()))?;

        let clusters = String::from_utf8_lossy(&output.stdout);
        Ok(clusters.lines().any(|line| line.trim() == name))
    }

    async fn kubeconfig(&self, name: &str) -> Result<String, ProviderError> {
        let output = Command::new("kind")
            .args(["get", "kubeconfig", "--name", name])
            .output()
            .map_err(|e| ProviderError::CommandFailed(e.to_string()))?;

        if !output.status.success() {
            return Err(ProviderError::CommandFailed(
                String::from_utf8_lossy(&output.stderr).to_string()
            ));
        }

        Ok(String::from_utf8_lossy(&output.stdout).to_string())
    }

    fn name(&self) -> &'static str {
        "kind"
    }
}
