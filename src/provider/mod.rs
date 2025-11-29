//! Cluster providers for different Kubernetes environments
//!
//! Seppo supports multiple cluster providers:
//! - Kind (default): Local clusters using Docker
//! - Minikube: Local clusters with VM/Docker driver
//! - Existing: Use an existing cluster (no lifecycle management)

use async_trait::async_trait;
use crate::config::{ClusterConfig, ClusterProviderType};

mod kind;
mod minikube;
mod existing;

pub use kind::KindProvider;
pub use minikube::MinikubeProvider;
pub use existing::ExistingProvider;

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

    #[tokio::test]
    async fn test_get_provider_from_config() {
        use crate::config::Config;

        let yaml = r#"
cluster:
  name: test-cluster
  provider: kind
"#;
        let config: Config = yaml.parse().unwrap();

        let provider = get_provider(&config.cluster);
        assert!(provider.is_ok(), "Should get Kind provider");
    }

    #[tokio::test]
    async fn test_get_provider_default_is_kind() {
        use crate::config::Config;

        let yaml = r#"
cluster:
  name: test-cluster
"#;
        let config: Config = yaml.parse().unwrap();

        // No provider specified, should default to Kind
        let provider = get_provider(&config.cluster);
        assert!(provider.is_ok(), "Should default to Kind provider");
    }

    #[tokio::test]
    async fn test_get_provider_minikube() {
        use crate::config::Config;

        let yaml = r#"
cluster:
  name: test-cluster
  provider: minikube
"#;
        let config: Config = yaml.parse().unwrap();

        let provider = get_provider(&config.cluster);
        assert!(provider.is_ok(), "Should get Minikube provider");
    }

    #[tokio::test]
    async fn test_get_provider_existing() {
        use crate::config::Config;

        let yaml = r#"
cluster:
  name: test-cluster
  provider: existing
  kubeconfig: ~/.kube/config
"#;
        let config: Config = yaml.parse().unwrap();

        let provider = get_provider(&config.cluster);
        assert!(provider.is_ok(), "Should get Existing provider");
    }
}
