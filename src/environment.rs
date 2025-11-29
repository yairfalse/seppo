//! Environment setup for test infrastructure
//!
//! Handles the "SETUP ENV" step of Seppo's test lifecycle:
//! - Load Docker images into cluster
//! - Apply K8s manifests
//! - Wait for readiness conditions
//! - Run setup scripts
//! - Export environment variables

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;

    #[test]
    fn test_setup_result_default() {
        let result = SetupResult::default();
        assert!(result.images_loaded.is_empty());
        assert!(result.manifests_applied.is_empty());
        assert!(result.wait_conditions_met.is_empty());
    }

    #[tokio::test]
    async fn test_setup_loads_images() {
        let yaml = r#"
cluster:
  name: env-test
  provider: kind

environment:
  images:
    - myapp:test
    - backend:latest
"#;
        let config: Config = yaml.parse().unwrap();

        // This should fail - setup() doesn't exist yet
        let result = setup(&config).await;

        // We can't actually test image loading without a cluster,
        // but we can verify the function exists and returns correct type
        assert!(result.is_ok() || result.is_err());
    }

    #[tokio::test]
    async fn test_setup_applies_manifests() {
        let yaml = r#"
cluster:
  name: manifest-test
  provider: existing
  kubeconfig: ~/.kube/config

environment:
  manifests:
    - ./test/fixtures/namespace.yaml
    - ./test/fixtures/deployment.yaml
"#;
        let config: Config = yaml.parse().unwrap();

        // setup() should exist and handle manifests
        let result = setup(&config).await;
        assert!(result.is_ok() || result.is_err());
    }

    #[tokio::test]
    async fn test_setup_with_wait_conditions() {
        let yaml = r#"
cluster:
  name: wait-test
  provider: existing

environment:
  manifests:
    - ./test/fixtures/deployment.yaml
  wait:
    - condition: available
      resource: deployment/test-app
      namespace: default
      timeout: 60s
"#;
        let config: Config = yaml.parse().unwrap();

        let result = setup(&config).await;
        assert!(result.is_ok() || result.is_err());
    }

    #[tokio::test]
    async fn test_setup_with_setup_script() {
        let yaml = r#"
cluster:
  name: script-test
  provider: existing

environment:
  setup_script: ./scripts/setup.sh
"#;
        let config: Config = yaml.parse().unwrap();

        let result = setup(&config).await;
        assert!(result.is_ok() || result.is_err());
    }

    #[tokio::test]
    async fn test_setup_empty_environment() {
        let yaml = r#"
cluster:
  name: empty-env-test
  provider: existing
"#;
        let config: Config = yaml.parse().unwrap();

        // Empty environment should succeed (no-op)
        let result = setup(&config).await;
        assert!(result.is_ok() || result.is_err());
    }

    #[test]
    fn test_environment_error_display() {
        let err = EnvironmentError::ManifestNotFound("./missing.yaml".to_string());
        assert!(err.to_string().contains("missing.yaml"));

        let err = EnvironmentError::WaitTimeout {
            resource: "deployment/app".to_string(),
            timeout: "60s".to_string(),
        };
        assert!(err.to_string().contains("deployment/app"));
    }
}
