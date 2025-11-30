//! Cluster providers for different Kubernetes environments
//!
//! Seppo supports multiple cluster providers:
//! - Kind (default): Local clusters using Docker
//! - Minikube: Local clusters with VM/Docker driver
//! - Existing: Use an existing cluster (no lifecycle management)

use crate::config::{ClusterConfig, ClusterProviderType};
use async_trait::async_trait;

mod existing;
mod kind;
mod minikube;

pub use existing::ExistingProvider;
pub use kind::KindProvider;
pub use minikube::MinikubeProvider;

/// Error type for provider operations
#[derive(Debug, thiserror::Error)]
pub enum ProviderError {
    #[error("Cluster creation failed: {0}")]
    CreateFailed(String),

    #[error("Cluster deletion failed: {0}")]
    DeleteFailed(String),

    #[error("Image load failed: {0}")]
    ImageLoadFailed(String),

    #[error("Command execution failed: {0}")]
    CommandFailed(String),

    #[error("Provider not available: {0}")]
    NotAvailable(String),
}

/// Trait for cluster providers
#[async_trait]
pub trait ClusterProvider: Send + Sync {
    /// Create a new cluster
    async fn create(&self, config: &ClusterConfig) -> Result<(), ProviderError>;

    /// Delete the cluster
    async fn delete(&self, name: &str) -> Result<(), ProviderError>;

    /// Load a Docker image into the cluster
    async fn load_image(&self, cluster: &str, image: &str) -> Result<(), ProviderError>;

    /// Check if cluster exists
    async fn exists(&self, name: &str) -> Result<bool, ProviderError>;

    /// Get kubeconfig path for the cluster
    async fn kubeconfig(&self, name: &str) -> Result<String, ProviderError>;

    /// Provider name for display
    fn name(&self) -> &'static str;
}

/// Get the appropriate provider for the given config
pub fn get_provider(config: &ClusterConfig) -> Result<Box<dyn ClusterProvider>, ProviderError> {
    match config.provider {
        ClusterProviderType::Kind => Ok(Box::new(KindProvider::new())),
        ClusterProviderType::Minikube => Ok(Box::new(MinikubeProvider::new())),
        ClusterProviderType::Existing => Ok(Box::new(ExistingProvider::new(
            config.kubeconfig.clone(),
            config.context.clone(),
        ))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_kind_provider_implements_trait() {
        let provider = KindProvider::new();

        // Should be able to call trait methods
        let exists = provider.exists("nonexistent-cluster").await.unwrap();
        assert!(!exists, "Nonexistent cluster should not exist");
    }

    #[test]
    fn test_get_provider_kind() {
        let config = ClusterConfig::kind("test-cluster");
        let provider = get_provider(&config);
        assert!(provider.is_ok(), "Should get Kind provider");
    }

    #[test]
    fn test_get_provider_minikube() {
        let config = ClusterConfig::minikube("test-cluster");
        let provider = get_provider(&config);
        assert!(provider.is_ok(), "Should get Minikube provider");
    }

    #[test]
    fn test_get_provider_existing() {
        let config = ClusterConfig::existing("test-cluster").kubeconfig("~/.kube/config");
        let provider = get_provider(&config);
        assert!(provider.is_ok(), "Should get Existing provider");
    }
}
