//! Configuration parsing for seppo.yaml
//!
//! Defines the structure of test environment configuration.

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_minimal_config() {
        let yaml = r#"
cluster:
  name: test-cluster
"#;

        let config = Config::from_str(yaml).expect("Should parse minimal config");

        assert_eq!(config.cluster.name, "test-cluster");
        assert_eq!(config.cluster.workers, 2); // default
    }

    #[test]
    fn test_parse_full_config() {
        let yaml = r#"
cluster:
  name: rauta-test
  workers: 3
  k8s_version: "1.31.0"

environment:
  images:
    - rauta:test
    - backend:latest

  manifests:
    - ./test/fixtures/namespace.yaml
    - ./test/fixtures/deployment.yaml

  wait:
    - condition: available
      resource: deployment/test-backend
      namespace: test
      timeout: 120s

  setup_script: ./scripts/setup.sh

  env:
    TEST_URL: "http://localhost:8080"
"#;

        let config = Config::from_str(yaml).expect("Should parse full config");

        // Cluster
        assert_eq!(config.cluster.name, "rauta-test");
        assert_eq!(config.cluster.workers, 3);
        assert_eq!(config.cluster.k8s_version, Some("1.31.0".to_string()));

        // Environment - images
        assert_eq!(config.environment.images.len(), 2);
        assert_eq!(config.environment.images[0], "rauta:test");

        // Environment - manifests
        assert_eq!(config.environment.manifests.len(), 2);
        assert_eq!(config.environment.manifests[0], "./test/fixtures/namespace.yaml");

        // Environment - wait conditions
        assert_eq!(config.environment.wait.len(), 1);
        assert_eq!(config.environment.wait[0].condition, "available");
        assert_eq!(config.environment.wait[0].resource, "deployment/test-backend");
        assert_eq!(config.environment.wait[0].namespace, Some("test".to_string()));
        assert_eq!(config.environment.wait[0].timeout, "120s");

        // Environment - setup script
        assert_eq!(config.environment.setup_script, Some("./scripts/setup.sh".to_string()));

        // Environment - env vars
        assert_eq!(config.environment.env.get("TEST_URL"), Some(&"http://localhost:8080".to_string()));
    }

    #[test]
    fn test_parse_config_from_file() {
        // Create a temp file
        let temp_dir = std::env::temp_dir();
        let config_path = temp_dir.join("test-seppo.yaml");

        std::fs::write(&config_path, r#"
cluster:
  name: file-test
  workers: 1
"#).expect("Should write temp file");

        let config = Config::from_file(&config_path).expect("Should parse from file");

        assert_eq!(config.cluster.name, "file-test");
        assert_eq!(config.cluster.workers, 1);

        // Cleanup
        std::fs::remove_file(&config_path).ok();
    }

    #[test]
    fn test_parse_config_missing_cluster_name_fails() {
        let yaml = r#"
cluster:
  workers: 2
"#;

        let result = Config::from_str(yaml);
        assert!(result.is_err(), "Should fail without cluster name");
    }

    #[test]
    fn test_parse_config_empty_fails() {
        let yaml = "";
        let result = Config::from_str(yaml);
        assert!(result.is_err(), "Should fail on empty config");
    }
}
