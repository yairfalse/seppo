//! Configuration types for Seppo
//!
//! These types define cluster and environment configuration.
//! Build them programmatically - no config files needed.
//!
//! # Example
//!
//! ```
//! use seppo::config::{ClusterConfig, EnvironmentConfig, WaitCondition};
//!
//! let cluster = ClusterConfig::kind("my-test")
//!     .workers(2)
//!     .k8s_version("1.31.0");
//!
//! let env = EnvironmentConfig::new()
//!     .image("myapp:test")
//!     .manifest("./k8s/deployment.yaml")
//!     .wait(WaitCondition::available("deployment/myapp"));
//! ```

use std::collections::HashMap;
use std::time::Duration;

/// Cluster configuration
#[derive(Debug, Clone)]
pub struct ClusterConfig {
    /// Cluster name
    pub name: String,

    /// Cluster provider
    pub provider: ClusterProviderType,

    /// Number of worker nodes
    pub workers: u32,

    /// Kubernetes version
    pub k8s_version: Option<String>,

    /// Minikube driver (docker, hyperkit, virtualbox)
    pub driver: Option<String>,

    /// Kubeconfig path (for provider: existing)
    pub kubeconfig: Option<String>,

    /// Kubectl context (for provider: existing)
    pub context: Option<String>,
}

impl ClusterConfig {
    /// Create a Kind cluster config
    pub fn kind(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            provider: ClusterProviderType::Kind,
            workers: 2,
            k8s_version: None,
            driver: None,
            kubeconfig: None,
            context: None,
        }
    }

    /// Create a Minikube cluster config
    pub fn minikube(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            provider: ClusterProviderType::Minikube,
            workers: 2,
            k8s_version: None,
            driver: Some("docker".to_string()),
            kubeconfig: None,
            context: None,
        }
    }

    /// Use an existing cluster
    pub fn existing(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            provider: ClusterProviderType::Existing,
            workers: 0,
            k8s_version: None,
            driver: None,
            kubeconfig: None,
            context: None,
        }
    }

    /// Set number of worker nodes
    pub fn workers(mut self, workers: u32) -> Self {
        self.workers = workers;
        self
    }

    /// Set Kubernetes version
    pub fn k8s_version(mut self, version: impl Into<String>) -> Self {
        self.k8s_version = Some(version.into());
        self
    }

    /// Set Minikube driver
    pub fn driver(mut self, driver: impl Into<String>) -> Self {
        self.driver = Some(driver.into());
        self
    }

    /// Set kubeconfig path (for existing clusters)
    pub fn kubeconfig(mut self, path: impl Into<String>) -> Self {
        self.kubeconfig = Some(path.into());
        self
    }

    /// Set kubectl context (for existing clusters)
    pub fn context(mut self, context: impl Into<String>) -> Self {
        self.context = Some(context.into());
        self
    }
}

/// Cluster provider type
#[derive(Debug, Clone, Default, PartialEq)]
pub enum ClusterProviderType {
    #[default]
    Kind,
    Minikube,
    Existing,
}

/// Environment setup configuration
#[derive(Debug, Clone, Default)]
pub struct EnvironmentConfig {
    /// Docker images to load into cluster
    pub images: Vec<String>,

    /// K8s manifests to apply (in order)
    pub manifests: Vec<String>,

    /// Wait conditions before running tests
    pub wait: Vec<WaitCondition>,

    /// Optional setup script to run after manifests
    pub setup_script: Option<String>,

    /// Environment variables to export for tests
    pub env: HashMap<String, String>,
}

impl EnvironmentConfig {
    /// Create new empty environment config
    pub fn new() -> Self {
        Self::default()
    }

    /// Add a Docker image to load
    pub fn image(mut self, image: impl Into<String>) -> Self {
        self.images.push(image.into());
        self
    }

    /// Add multiple Docker images
    pub fn images(mut self, images: impl IntoIterator<Item = impl Into<String>>) -> Self {
        self.images.extend(images.into_iter().map(Into::into));
        self
    }

    /// Add a manifest to apply
    pub fn manifest(mut self, path: impl Into<String>) -> Self {
        self.manifests.push(path.into());
        self
    }

    /// Add multiple manifests
    pub fn manifests(mut self, paths: impl IntoIterator<Item = impl Into<String>>) -> Self {
        self.manifests.extend(paths.into_iter().map(Into::into));
        self
    }

    /// Add a wait condition
    pub fn wait(mut self, condition: WaitCondition) -> Self {
        self.wait.push(condition);
        self
    }

    /// Set setup script path
    pub fn setup_script(mut self, path: impl Into<String>) -> Self {
        self.setup_script = Some(path.into());
        self
    }

    /// Add an environment variable
    pub fn env_var(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.env.insert(key.into(), value.into());
        self
    }
}

/// Wait condition for resource readiness
#[derive(Debug, Clone)]
pub struct WaitCondition {
    /// Condition type (available, ready, programmed, etc.)
    pub condition: String,

    /// Resource to wait for (e.g., deployment/test-backend)
    pub resource: String,

    /// Namespace
    pub namespace: String,

    /// Timeout
    pub timeout: Duration,

    /// Number of replicas to wait for
    pub replicas: Option<u32>,

    /// Label selector
    pub selector: Option<String>,
}

impl WaitCondition {
    /// Wait for resource to be available
    pub fn available(resource: impl Into<String>) -> Self {
        Self {
            condition: "available".to_string(),
            resource: resource.into(),
            namespace: "default".to_string(),
            timeout: Duration::from_secs(60),
            replicas: None,
            selector: None,
        }
    }

    /// Wait for resource to be ready
    pub fn ready(resource: impl Into<String>) -> Self {
        Self {
            condition: "ready".to_string(),
            resource: resource.into(),
            namespace: "default".to_string(),
            timeout: Duration::from_secs(60),
            replicas: None,
            selector: None,
        }
    }

    /// Wait for custom condition
    pub fn condition(condition: impl Into<String>, resource: impl Into<String>) -> Self {
        Self {
            condition: condition.into(),
            resource: resource.into(),
            namespace: "default".to_string(),
            timeout: Duration::from_secs(60),
            replicas: None,
            selector: None,
        }
    }

    /// Set namespace
    pub fn namespace(mut self, ns: impl Into<String>) -> Self {
        self.namespace = ns.into();
        self
    }

    /// Set timeout
    pub fn timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }

    /// Set timeout in seconds
    pub fn timeout_secs(mut self, secs: u64) -> Self {
        self.timeout = Duration::from_secs(secs);
        self
    }

    /// Set replicas to wait for
    pub fn replicas(mut self, count: u32) -> Self {
        self.replicas = Some(count);
        self
    }

    /// Set label selector
    pub fn selector(mut self, selector: impl Into<String>) -> Self {
        self.selector = Some(selector.into());
        self
    }

    /// Get timeout as string for kubectl (e.g., "60s")
    pub fn timeout_str(&self) -> String {
        format!("{}s", self.timeout.as_secs())
    }
}

/// Full test configuration (cluster + environment)
#[derive(Debug, Clone)]
pub struct Config {
    /// Cluster configuration
    pub cluster: ClusterConfig,

    /// Environment setup configuration
    pub environment: EnvironmentConfig,
}

impl Config {
    /// Create config with cluster and default environment
    pub fn new(cluster: ClusterConfig) -> Self {
        Self {
            cluster,
            environment: EnvironmentConfig::new(),
        }
    }

    /// Set environment configuration
    pub fn environment(mut self, env: EnvironmentConfig) -> Self {
        self.environment = env;
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_cluster_config_kind() {
        let config = ClusterConfig::kind("test").workers(3).k8s_version("1.31.0");

        assert_eq!(config.name, "test");
        assert_eq!(config.provider, ClusterProviderType::Kind);
        assert_eq!(config.workers, 3);
        assert_eq!(config.k8s_version, Some("1.31.0".to_string()));
    }

    #[test]
    fn test_cluster_config_minikube() {
        let config = ClusterConfig::minikube("test").driver("hyperkit");

        assert_eq!(config.provider, ClusterProviderType::Minikube);
        assert_eq!(config.driver, Some("hyperkit".to_string()));
    }

    #[test]
    fn test_cluster_config_existing() {
        let config = ClusterConfig::existing("prod")
            .kubeconfig("~/.kube/prod")
            .context("prod-context");

        assert_eq!(config.provider, ClusterProviderType::Existing);
        assert_eq!(config.kubeconfig, Some("~/.kube/prod".to_string()));
        assert_eq!(config.context, Some("prod-context".to_string()));
    }

    #[test]
    fn test_environment_config() {
        let env = EnvironmentConfig::new()
            .image("app:test")
            .image("sidecar:latest")
            .manifest("./k8s/ns.yaml")
            .manifest("./k8s/deploy.yaml")
            .wait(WaitCondition::available("deployment/app").namespace("test"))
            .setup_script("./setup.sh")
            .env_var("DEBUG", "true");

        assert_eq!(env.images.len(), 2);
        assert_eq!(env.manifests.len(), 2);
        assert_eq!(env.wait.len(), 1);
        assert_eq!(env.setup_script, Some("./setup.sh".to_string()));
        assert_eq!(env.env.get("DEBUG"), Some(&"true".to_string()));
    }

    #[test]
    fn test_wait_condition() {
        let cond = WaitCondition::available("deployment/myapp")
            .namespace("test")
            .timeout_secs(120);

        assert_eq!(cond.condition, "available");
        assert_eq!(cond.resource, "deployment/myapp");
        assert_eq!(cond.namespace, "test");
        assert_eq!(cond.timeout, Duration::from_secs(120));
        assert_eq!(cond.timeout_str(), "120s");
    }

    #[test]
    fn test_full_config() {
        let config = Config::new(ClusterConfig::kind("test").workers(2)).environment(
            EnvironmentConfig::new()
                .image("app:test")
                .manifest("./k8s/deploy.yaml"),
        );

        assert_eq!(config.cluster.name, "test");
        assert_eq!(config.environment.images.len(), 1);
    }
}
