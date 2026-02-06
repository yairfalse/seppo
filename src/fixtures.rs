//! Fixture builders for Kubernetes resources
//!
//! Provides fluent builders for creating K8s resources programmatically.
//! No YAML required.
//!
//! # Example
//!
//! ```ignore
//! use seppo::fixtures::{DeploymentFixture, PodFixture, ServiceFixture};
//!
//! // Create a deployment
//! let deployment = DeploymentFixture::new("my-app")
//!     .image("nginx:latest")
//!     .replicas(3)
//!     .port(80)
//!     .env("LOG_LEVEL", "debug")
//!     .build();
//!
//! ctx.apply(&deployment).await?;
//!
//! // Create a pod
//! let pod = PodFixture::new("test-pod")
//!     .image("busybox")
//!     .command(&["sleep", "infinity"])
//!     .build();
//!
//! ctx.apply(&pod).await?;
//! ```

use k8s_openapi::api::apps::v1::{Deployment, DeploymentSpec};
use k8s_openapi::api::core::v1::{
    Container, ContainerPort, EnvVar, Pod, PodSpec, PodTemplateSpec, Service, ServicePort,
    ServiceSpec,
};
use k8s_openapi::apimachinery::pkg::apis::meta::v1::{LabelSelector, ObjectMeta};
use std::collections::BTreeMap;

/// Builder for Deployment resources
#[derive(Clone)]
pub struct DeploymentFixture {
    name: String,
    namespace: Option<String>,
    image: String,
    replicas: i32,
    ports: Vec<u16>,
    env: Vec<(String, String)>,
    labels: BTreeMap<String, String>,
    command: Vec<String>,
    args: Vec<String>,
}

/// Builder for Pod resources
#[derive(Clone)]
pub struct PodFixture {
    name: String,
    namespace: Option<String>,
    image: String,
    command: Vec<String>,
    args: Vec<String>,
    env: Vec<(String, String)>,
    labels: BTreeMap<String, String>,
}

/// Builder for Service resources
#[derive(Clone)]
pub struct ServiceFixture {
    name: String,
    namespace: Option<String>,
    selector: BTreeMap<String, String>,
    ports: Vec<(u16, u16)>, // (port, targetPort)
    service_type: Option<String>,
}

impl DeploymentFixture {
    /// Create a new deployment fixture
    #[must_use]
    pub fn new(name: &str) -> Self {
        let mut labels = BTreeMap::new();
        labels.insert("app".to_string(), name.to_string());

        Self {
            name: name.to_string(),
            namespace: None,
            image: String::new(),
            replicas: 1,
            ports: Vec::new(),
            env: Vec::new(),
            labels,
            command: Vec::new(),
            args: Vec::new(),
        }
    }

    /// Set the namespace
    #[must_use]
    pub fn namespace(mut self, namespace: &str) -> Self {
        self.namespace = Some(namespace.to_string());
        self
    }

    /// Set the container image
    #[must_use]
    pub fn image(mut self, image: &str) -> Self {
        self.image = image.to_string();
        self
    }

    /// Set the replica count
    ///
    /// # Panics
    /// Panics if replicas is negative
    #[must_use]
    pub fn replicas(mut self, replicas: i32) -> Self {
        assert!(replicas >= 0, "replica count cannot be negative");
        self.replicas = replicas;
        self
    }

    /// Add a container port
    ///
    /// # Panics
    /// Panics if port is zero
    #[must_use]
    pub fn port(mut self, port: u16) -> Self {
        assert!(port > 0, "port must be in range 1-65535, got {port}");
        self.ports.push(port);
        self
    }

    /// Add an environment variable
    #[must_use]
    pub fn env(mut self, key: &str, value: &str) -> Self {
        self.env.push((key.to_string(), value.to_string()));
        self
    }

    /// Add a label
    #[must_use]
    pub fn label(mut self, key: &str, value: &str) -> Self {
        self.labels.insert(key.to_string(), value.to_string());
        self
    }

    /// Set the container command
    #[must_use]
    pub fn command(mut self, cmd: &[&str]) -> Self {
        self.command = cmd.iter().map(|s| (*s).to_string()).collect();
        self
    }

    /// Set the container args
    #[must_use]
    pub fn args(mut self, args: &[&str]) -> Self {
        self.args = args.iter().map(|s| (*s).to_string()).collect();
        self
    }

    /// Build the Deployment resource
    ///
    /// # Panics
    /// Panics if image is empty
    #[must_use]
    pub fn build(&self) -> Deployment {
        assert!(!self.image.is_empty(), "image must be set before building");

        let container_ports: Vec<ContainerPort> = self
            .ports
            .iter()
            .map(|p| ContainerPort {
                container_port: i32::from(*p),
                ..Default::default()
            })
            .collect();

        let env_vars: Vec<EnvVar> = self
            .env
            .iter()
            .map(|(k, v)| EnvVar {
                name: k.clone(),
                value: Some(v.clone()),
                ..Default::default()
            })
            .collect();

        let container = Container {
            name: self.name.clone(),
            image: Some(self.image.clone()),
            ports: if container_ports.is_empty() {
                None
            } else {
                Some(container_ports)
            },
            env: if env_vars.is_empty() {
                None
            } else {
                Some(env_vars)
            },
            command: if self.command.is_empty() {
                None
            } else {
                Some(self.command.clone())
            },
            args: if self.args.is_empty() {
                None
            } else {
                Some(self.args.clone())
            },
            ..Default::default()
        };

        Deployment {
            metadata: ObjectMeta {
                name: Some(self.name.clone()),
                namespace: self.namespace.clone(),
                labels: Some(self.labels.clone()),
                ..Default::default()
            },
            spec: Some(DeploymentSpec {
                replicas: Some(self.replicas),
                selector: LabelSelector {
                    match_labels: Some(self.labels.clone()),
                    ..Default::default()
                },
                template: PodTemplateSpec {
                    metadata: Some(ObjectMeta {
                        labels: Some(self.labels.clone()),
                        ..Default::default()
                    }),
                    spec: Some(PodSpec {
                        containers: vec![container],
                        ..Default::default()
                    }),
                },
                ..Default::default()
            }),
            ..Default::default()
        }
    }
}

impl PodFixture {
    /// Create a new pod fixture
    #[must_use]
    pub fn new(name: &str) -> Self {
        let mut labels = BTreeMap::new();
        labels.insert("app".to_string(), name.to_string());

        Self {
            name: name.to_string(),
            namespace: None,
            image: String::new(),
            command: Vec::new(),
            args: Vec::new(),
            env: Vec::new(),
            labels,
        }
    }

    /// Set the namespace
    #[must_use]
    pub fn namespace(mut self, namespace: &str) -> Self {
        self.namespace = Some(namespace.to_string());
        self
    }

    /// Set the container image
    #[must_use]
    pub fn image(mut self, image: &str) -> Self {
        self.image = image.to_string();
        self
    }

    /// Set the container command
    #[must_use]
    pub fn command(mut self, cmd: &[&str]) -> Self {
        self.command = cmd.iter().map(|s| (*s).to_string()).collect();
        self
    }

    /// Set the container args
    #[must_use]
    pub fn args(mut self, args: &[&str]) -> Self {
        self.args = args.iter().map(|s| (*s).to_string()).collect();
        self
    }

    /// Add an environment variable
    #[must_use]
    pub fn env(mut self, key: &str, value: &str) -> Self {
        self.env.push((key.to_string(), value.to_string()));
        self
    }

    /// Add a label
    #[must_use]
    pub fn label(mut self, key: &str, value: &str) -> Self {
        self.labels.insert(key.to_string(), value.to_string());
        self
    }

    /// Build the Pod resource
    ///
    /// # Panics
    /// Panics if image is empty
    #[must_use]
    pub fn build(&self) -> Pod {
        assert!(!self.image.is_empty(), "image must be set before building");

        let env_vars: Vec<EnvVar> = self
            .env
            .iter()
            .map(|(k, v)| EnvVar {
                name: k.clone(),
                value: Some(v.clone()),
                ..Default::default()
            })
            .collect();

        let container = Container {
            name: self.name.clone(),
            image: Some(self.image.clone()),
            command: if self.command.is_empty() {
                None
            } else {
                Some(self.command.clone())
            },
            args: if self.args.is_empty() {
                None
            } else {
                Some(self.args.clone())
            },
            env: if env_vars.is_empty() {
                None
            } else {
                Some(env_vars)
            },
            ..Default::default()
        };

        Pod {
            metadata: ObjectMeta {
                name: Some(self.name.clone()),
                namespace: self.namespace.clone(),
                labels: Some(self.labels.clone()),
                ..Default::default()
            },
            spec: Some(PodSpec {
                containers: vec![container],
                restart_policy: Some("Never".to_string()),
                ..Default::default()
            }),
            ..Default::default()
        }
    }
}

impl ServiceFixture {
    /// Create a new service fixture
    #[must_use]
    pub fn new(name: &str) -> Self {
        Self {
            name: name.to_string(),
            namespace: None,
            selector: BTreeMap::new(),
            ports: Vec::new(),
            service_type: None,
        }
    }

    /// Set the namespace
    #[must_use]
    pub fn namespace(mut self, namespace: &str) -> Self {
        self.namespace = Some(namespace.to_string());
        self
    }

    /// Add a selector
    #[must_use]
    pub fn selector(mut self, key: &str, value: &str) -> Self {
        self.selector.insert(key.to_string(), value.to_string());
        self
    }

    /// Add a port mapping (port -> targetPort)
    ///
    /// # Panics
    /// Panics if port or `target_port` is zero
    #[must_use]
    pub fn port(mut self, port: u16, target_port: u16) -> Self {
        assert!(port > 0, "port must be in range 1-65535, got {port}");
        assert!(
            target_port > 0,
            "target_port must be in range 1-65535, got {target_port}"
        );
        self.ports.push((port, target_port));
        self
    }

    /// Set service type to `ClusterIP`
    #[must_use]
    pub fn cluster_ip(mut self) -> Self {
        self.service_type = Some("ClusterIP".to_string());
        self
    }

    /// Set service type to `NodePort`
    #[must_use]
    pub fn node_port(mut self) -> Self {
        self.service_type = Some("NodePort".to_string());
        self
    }

    /// Set service type to `LoadBalancer`
    #[must_use]
    pub fn load_balancer(mut self) -> Self {
        self.service_type = Some("LoadBalancer".to_string());
        self
    }

    /// Build the Service resource
    #[must_use]
    pub fn build(&self) -> Service {
        let service_ports: Vec<ServicePort> = self
            .ports
            .iter()
            .map(|(port, target_port)| ServicePort {
                port: i32::from(*port),
                target_port: Some(
                    k8s_openapi::apimachinery::pkg::util::intstr::IntOrString::Int(i32::from(
                        *target_port,
                    )),
                ),
                ..Default::default()
            })
            .collect();

        Service {
            metadata: ObjectMeta {
                name: Some(self.name.clone()),
                namespace: self.namespace.clone(),
                ..Default::default()
            },
            spec: Some(ServiceSpec {
                selector: if self.selector.is_empty() {
                    None
                } else {
                    Some(self.selector.clone())
                },
                ports: if service_ports.is_empty() {
                    None
                } else {
                    Some(service_ports)
                },
                type_: self.service_type.clone(),
                ..Default::default()
            }),
            ..Default::default()
        }
    }
}

// ============================================================
// Builder functions (ilmari parity)
// ============================================================

/// Create a deployment builder
///
/// Shorthand for `DeploymentFixture::new()`, aligning with ilmari's Go API.
///
/// # Example
///
/// ```ignore
/// use seppo::deployment;
///
/// let deploy = deployment("api")
///     .image("nginx:latest")
///     .replicas(3)
///     .build();
/// ```
#[must_use]
pub fn deployment(name: &str) -> DeploymentFixture {
    DeploymentFixture::new(name)
}

/// Create a pod builder
///
/// Shorthand for `PodFixture::new()`, aligning with ilmari's Go API.
///
/// # Example
///
/// ```ignore
/// use seppo::pod;
///
/// let p = pod("worker")
///     .image("busybox")
///     .command(&["sleep", "infinity"])
///     .build();
/// ```
#[must_use]
pub fn pod(name: &str) -> PodFixture {
    PodFixture::new(name)
}

/// Create a service builder
///
/// Shorthand for `ServiceFixture::new()`, aligning with ilmari's Go API.
///
/// # Example
///
/// ```ignore
/// use seppo::service;
///
/// let svc = service("api")
///     .selector("app", "api")
///     .port(80, 8080)
///     .build();
/// ```
#[must_use]
pub fn service(name: &str) -> ServiceFixture {
    ServiceFixture::new(name)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_deployment_fixture_basic() {
        let deployment = DeploymentFixture::new("my-app")
            .image("nginx:latest")
            .replicas(3)
            .build();

        assert_eq!(deployment.metadata.name, Some("my-app".to_string()));
        assert_eq!(deployment.spec.as_ref().unwrap().replicas, Some(3));

        let container = &deployment
            .spec
            .as_ref()
            .unwrap()
            .template
            .spec
            .as_ref()
            .unwrap()
            .containers[0];
        assert_eq!(container.image, Some("nginx:latest".to_string()));
    }

    #[test]
    fn test_deployment_fixture_with_port_and_env() {
        let deployment = DeploymentFixture::new("my-app")
            .image("nginx:latest")
            .port(80)
            .env("LOG_LEVEL", "debug")
            .build();

        let container = &deployment
            .spec
            .as_ref()
            .unwrap()
            .template
            .spec
            .as_ref()
            .unwrap()
            .containers[0];

        let ports = container.ports.as_ref().unwrap();
        assert_eq!(ports[0].container_port, 80);

        let env = container.env.as_ref().unwrap();
        assert_eq!(env[0].name, "LOG_LEVEL");
        assert_eq!(env[0].value, Some("debug".to_string()));
    }

    #[test]
    fn test_deployment_fixture_with_labels() {
        let deployment = DeploymentFixture::new("my-app")
            .image("nginx:latest")
            .label("tier", "frontend")
            .build();

        let labels = deployment.metadata.labels.as_ref().unwrap();
        assert_eq!(labels.get("app"), Some(&"my-app".to_string()));
        assert_eq!(labels.get("tier"), Some(&"frontend".to_string()));
    }

    #[test]
    fn test_pod_fixture_basic() {
        let pod = PodFixture::new("test-pod")
            .image("busybox")
            .command(&["sleep", "infinity"])
            .build();

        assert_eq!(pod.metadata.name, Some("test-pod".to_string()));

        let container = &pod.spec.as_ref().unwrap().containers[0];
        assert_eq!(container.image, Some("busybox".to_string()));
        assert_eq!(
            container.command,
            Some(vec!["sleep".to_string(), "infinity".to_string()])
        );
    }

    #[test]
    fn test_pod_fixture_with_env() {
        let pod = PodFixture::new("test-pod")
            .image("busybox")
            .env("MY_VAR", "my_value")
            .build();

        let container = &pod.spec.as_ref().unwrap().containers[0];
        let env = container.env.as_ref().unwrap();
        assert_eq!(env[0].name, "MY_VAR");
        assert_eq!(env[0].value, Some("my_value".to_string()));
    }

    #[test]
    fn test_service_fixture_basic() {
        let service = ServiceFixture::new("my-service")
            .selector("app", "my-app")
            .port(80, 8080)
            .build();

        assert_eq!(service.metadata.name, Some("my-service".to_string()));

        let spec = service.spec.as_ref().unwrap();
        assert_eq!(
            spec.selector.as_ref().unwrap().get("app"),
            Some(&"my-app".to_string())
        );

        let ports = spec.ports.as_ref().unwrap();
        assert_eq!(ports[0].port, 80);
    }

    #[test]
    fn test_service_fixture_types() {
        let cluster_ip = ServiceFixture::new("svc1").cluster_ip().build();
        assert_eq!(
            cluster_ip.spec.as_ref().unwrap().type_,
            Some("ClusterIP".to_string())
        );

        let node_port = ServiceFixture::new("svc2").node_port().build();
        assert_eq!(
            node_port.spec.as_ref().unwrap().type_,
            Some("NodePort".to_string())
        );

        let load_balancer = ServiceFixture::new("svc3").load_balancer().build();
        assert_eq!(
            load_balancer.spec.as_ref().unwrap().type_,
            Some("LoadBalancer".to_string())
        );
    }

    #[test]
    fn test_deployment_fixture_with_command() {
        let deployment = DeploymentFixture::new("my-app")
            .image("busybox")
            .command(&["sh", "-c"])
            .args(&["echo hello"])
            .build();

        let container = &deployment
            .spec
            .as_ref()
            .unwrap()
            .template
            .spec
            .as_ref()
            .unwrap()
            .containers[0];
        assert_eq!(
            container.command,
            Some(vec!["sh".to_string(), "-c".to_string()])
        );
        assert_eq!(container.args, Some(vec!["echo hello".to_string()]));
    }

    #[test]
    fn test_deployment_fixture_with_namespace() {
        let deployment = DeploymentFixture::new("my-app")
            .image("nginx:latest")
            .namespace("my-namespace")
            .build();

        assert_eq!(
            deployment.metadata.namespace,
            Some("my-namespace".to_string())
        );
    }

    #[test]
    fn test_pod_fixture_with_namespace() {
        let pod = PodFixture::new("test-pod")
            .image("busybox")
            .namespace("my-namespace")
            .build();

        assert_eq!(pod.metadata.namespace, Some("my-namespace".to_string()));
    }

    #[test]
    fn test_pod_fixture_with_args() {
        let pod = PodFixture::new("test-pod")
            .image("busybox")
            .command(&["sh", "-c"])
            .args(&["echo hello && sleep infinity"])
            .build();

        let container = &pod.spec.as_ref().unwrap().containers[0];
        assert_eq!(
            container.args,
            Some(vec!["echo hello && sleep infinity".to_string()])
        );
    }

    #[test]
    fn test_pod_fixture_with_label() {
        let pod = PodFixture::new("test-pod")
            .image("busybox")
            .label("tier", "backend")
            .label("env", "test")
            .build();

        let labels = pod.metadata.labels.as_ref().unwrap();
        assert_eq!(labels.get("app"), Some(&"test-pod".to_string()));
        assert_eq!(labels.get("tier"), Some(&"backend".to_string()));
        assert_eq!(labels.get("env"), Some(&"test".to_string()));
    }

    #[test]
    fn test_service_fixture_with_namespace() {
        let service = ServiceFixture::new("my-service")
            .namespace("my-namespace")
            .selector("app", "my-app")
            .port(80, 8080)
            .build();

        assert_eq!(service.metadata.namespace, Some("my-namespace".to_string()));
    }

    #[test]
    #[should_panic(expected = "replica count cannot be negative")]
    fn test_deployment_negative_replicas_panics() {
        let _ = DeploymentFixture::new("my-app")
            .image("nginx:latest")
            .replicas(-1);
    }

    #[test]
    #[should_panic(expected = "port must be in range 1-65535")]
    fn test_deployment_invalid_port_zero_panics() {
        let _ = DeploymentFixture::new("my-app")
            .image("nginx:latest")
            .port(0);
    }

    #[test]
    #[should_panic(expected = "port must be in range 1-65535")]
    fn test_service_invalid_port_panics() {
        let _ = ServiceFixture::new("my-service").port(0, 8080);
    }

    #[test]
    #[should_panic(expected = "target_port must be in range 1-65535")]
    fn test_service_invalid_target_port_panics() {
        let _ = ServiceFixture::new("my-service").port(80, 0);
    }

    #[test]
    #[should_panic(expected = "image must be set before building")]
    fn test_deployment_empty_image_panics() {
        let _ = DeploymentFixture::new("my-app").build();
    }

    #[test]
    #[should_panic(expected = "image must be set before building")]
    fn test_pod_empty_image_panics() {
        let _ = PodFixture::new("test-pod").build();
    }

    #[test]
    fn test_deployment_valid_port_range() {
        // Test boundary values
        let deployment = DeploymentFixture::new("my-app")
            .image("nginx:latest")
            .port(1)
            .port(65535)
            .build();

        let container = &deployment
            .spec
            .as_ref()
            .unwrap()
            .template
            .spec
            .as_ref()
            .unwrap()
            .containers[0];

        let ports = container.ports.as_ref().unwrap();
        assert_eq!(ports[0].container_port, 1);
        assert_eq!(ports[1].container_port, 65535);
    }

    #[test]
    fn test_service_valid_port_range() {
        // Test boundary values
        let service = ServiceFixture::new("my-service")
            .port(1, 1)
            .port(65535, 65535)
            .build();

        let ports = service.spec.as_ref().unwrap().ports.as_ref().unwrap();
        assert_eq!(ports[0].port, 1);
        assert_eq!(ports[1].port, 65535);
    }

    // ============================================================
    // Builder function tests (ilmari parity)
    // ============================================================

    #[test]
    fn test_deployment_builder_function() {
        // Test the lowercase deployment() function
        let deploy = deployment("api").image("nginx:latest").build();

        assert_eq!(deploy.metadata.name, Some("api".to_string()));
        let container = &deploy
            .spec
            .as_ref()
            .unwrap()
            .template
            .spec
            .as_ref()
            .unwrap()
            .containers[0];
        assert_eq!(container.image, Some("nginx:latest".to_string()));
    }

    #[test]
    fn test_pod_builder_function() {
        // Test the lowercase pod() function
        let p = pod("worker").image("busybox").build();

        assert_eq!(p.metadata.name, Some("worker".to_string()));
        let container = &p.spec.as_ref().unwrap().containers[0];
        assert_eq!(container.image, Some("busybox".to_string()));
    }

    #[test]
    fn test_service_builder_function() {
        // Test the lowercase service() function
        let svc = service("api").selector("app", "api").port(80, 8080).build();

        assert_eq!(svc.metadata.name, Some("api".to_string()));
    }
}
