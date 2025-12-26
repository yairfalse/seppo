//! Existing cluster provider
//!
//! Uses an existing Kubernetes cluster (no lifecycle management).

use async_trait::async_trait;
use tracing::{debug, info, instrument};

use super::{ClusterProvider, ProviderError};
use crate::config::ClusterConfig;

/// Existing cluster provider - uses a pre-existing cluster
pub struct ExistingProvider {
    kubeconfig: Option<String>,
    context: Option<String>,
}

impl ExistingProvider {
    #[must_use]
    pub fn new(kubeconfig: Option<String>, context: Option<String>) -> Self {
        Self {
            kubeconfig,
            context,
        }
    }
}

impl Default for ExistingProvider {
    fn default() -> Self {
        Self::new(None, None)
    }
}

#[async_trait]
impl ClusterProvider for ExistingProvider {
    #[instrument(skip(self), fields(cluster_name = %config.name, provider = "existing"))]
    async fn create(&self, config: &ClusterConfig) -> Result<(), ProviderError> {
        // No-op for existing clusters - just verify it exists
        info!("Using existing cluster: {}", config.name);

        if !self.exists(&config.name).await? {
            return Err(ProviderError::CreateFailed(format!(
                "Cluster {} not found. Ensure kubeconfig and context are correct.",
                config.name
            )));
        }

        Ok(())
    }

    #[instrument(skip(self), fields(cluster_name = %name, provider = "existing"))]
    async fn delete(&self, name: &str) -> Result<(), ProviderError> {
        // No-op for existing clusters - we don't delete clusters we didn't create
        debug!(
            "Skipping delete for existing cluster: {} (not managed by Seppo)",
            name
        );
        Ok(())
    }

    #[instrument(skip(self), fields(cluster_name = %_cluster, image = %_image, provider = "existing"))]
    async fn load_image(&self, _cluster: &str, _image: &str) -> Result<(), ProviderError> {
        // Cannot load images into remote/existing clusters
        // User must push to a registry that the cluster can pull from
        Err(ProviderError::NotAvailable(
            "Cannot load images into existing cluster. Push to a registry instead.".to_string(),
        ))
    }

    async fn exists(&self, _name: &str) -> Result<bool, ProviderError> {
        // Check if we can connect to the cluster
        let mut cmd = std::process::Command::new("kubectl");
        cmd.args(["cluster-info"]);

        if let Some(ref kubeconfig) = self.kubeconfig {
            cmd.args(["--kubeconfig", kubeconfig]);
        }

        if let Some(ref context) = self.context {
            cmd.args(["--context", context]);
        }

        let output = cmd
            .output()
            .map_err(|e| ProviderError::CommandFailed(e.to_string()))?;

        Ok(output.status.success())
    }

    async fn kubeconfig(&self, _name: &str) -> Result<String, ProviderError> {
        if let Some(path) = &self.kubeconfig {
            Ok(path.clone())
        } else {
            let home = std::env::var("HOME")
                .map_err(|_| ProviderError::CommandFailed("HOME not set".to_string()))?;
            Ok(format!("{home}/.kube/config"))
        }
    }

    fn name(&self) -> &'static str {
        "existing"
    }
}
