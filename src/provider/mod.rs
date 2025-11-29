//! Cluster providers for different Kubernetes environments
//!
//! Seppo supports multiple cluster providers:
//! - Kind (default): Local clusters using Docker
//! - Minikube: Local clusters with VM/Docker driver
//! - Existing: Use an existing cluster (no lifecycle management)

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
