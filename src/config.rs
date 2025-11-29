//! Configuration parsing for seppo.yaml
//!
//! Defines the structure of test environment configuration.

use serde::Deserialize;
use std::collections::HashMap;
use std::path::Path;

/// Main configuration for Seppo test environment
#[derive(Debug, Clone, Deserialize)]
pub struct Config {
    /// Cluster configuration
    pub cluster: ClusterConfig,

    /// Environment setup configuration
    #[serde(default)]
    pub environment: EnvironmentConfig,
}

/// Cluster configuration
#[derive(Debug, Clone, Deserialize)]
pub struct ClusterConfig {
    /// Cluster name (required)
    pub name: String,

    /// Number of worker nodes (default: 2)
    #[serde(default = "default_workers")]
    pub workers: u32,

    /// Kubernetes version (optional, uses Kind default)
    pub k8s_version: Option<String>,
}

fn default_workers() -> u32 {
    2
}

/// Environment setup configuration
#[derive(Debug, Clone, Default, Deserialize)]
pub struct EnvironmentConfig {
    /// Docker images to load into cluster
    #[serde(default)]
    pub images: Vec<String>,

    /// K8s manifests to apply (in order)
    #[serde(default)]
    pub manifests: Vec<String>,

    /// Wait conditions before running tests
    #[serde(default)]
    pub wait: Vec<WaitCondition>,

    /// Optional setup script to run after manifests
    pub setup_script: Option<String>,

    /// Environment variables to export for tests
    #[serde(default)]
    pub env: HashMap<String, String>,
}

/// Wait condition for resource readiness
#[derive(Debug, Clone, Deserialize)]
pub struct WaitCondition {
    /// Condition type (available, ready, programmed, etc.)
    pub condition: String,

    /// Resource to wait for (e.g., deployment/test-backend)
    pub resource: String,

    /// Namespace (optional, defaults to "default")
    pub namespace: Option<String>,

    /// Timeout (e.g., "120s")
    #[serde(default = "default_timeout")]
    pub timeout: String,

    /// Number of replicas to wait for (optional)
    pub replicas: Option<u32>,

    /// Label selector (optional)
    pub selector: Option<String>,
}

fn default_timeout() -> String {
    "60s".to_string()
}

impl Config {
    /// Parse config from YAML string
    pub fn from_str(yaml: &str) -> Result<Self, ConfigError> {
        if yaml.trim().is_empty() {
            return Err(ConfigError::Empty);
        }

        serde_yaml::from_str(yaml).map_err(|e| ConfigError::ParseError(e.to_string()))
    }

    /// Parse config from file path
    pub fn from_file<P: AsRef<Path>>(path: P) -> Result<Self, ConfigError> {
        let path = path.as_ref();

        if !path.exists() {
            return Err(ConfigError::NotFound(path.display().to_string()));
        }

        let content = std::fs::read_to_string(path)
            .map_err(|e| ConfigError::ReadError(e.to_string()))?;

        Self::from_str(&content)
    }
}

/// Configuration errors
#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    #[error("Config file not found: {0}")]
    NotFound(String),

    #[error("Failed to read config: {0}")]
    ReadError(String),

    #[error("Failed to parse config: {0}")]
    ParseError(String),

    #[error("Config is empty")]
    Empty,
}

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
