//! Stack builder for multi-service deployments
//!
//! Provides a fluent API for defining and deploying multiple services.

use k8s_openapi::api::apps::v1::{Deployment, DeploymentSpec};
use k8s_openapi::api::core::v1::{
    Container, ContainerPort, PodSpec, PodTemplateSpec, Service, ServicePort, ServiceSpec,
};
use k8s_openapi::apimachinery::pkg::apis::meta::v1::LabelSelector;
use std::collections::BTreeMap;

/// Error type for stack operations
#[derive(Debug, thiserror::Error)]
pub enum StackError {
    #[error("Failed to deploy service '{0}': {1}")]
    DeployError(String, String),

    #[error("No services defined in stack")]
    EmptyStack,
}

/// A service definition within a stack
#[derive(Debug, Clone)]
pub struct ServiceDef {
    pub name: String,
    pub image: String,
    pub replicas: i32,
    pub port: Option<i32>,
}

impl ServiceDef {
    fn new(name: &str) -> Self {
        Self {
            name: name.to_string(),
            image: String::new(),
            replicas: 1,
            port: None,
        }
    }
}

/// Builder for defining a stack of services
///
/// # Example
///
/// ```ignore
/// let stack = Stack::new()
///     .service("frontend")
///         .image("fe:test")
///         .replicas(4)
///         .port(80)
///     .service("backend")
///         .image("be:test")
///         .replicas(2)
///         .port(8080)
///         .build();  // <-- This is required
///
/// ctx.up(&stack).await?;
/// ```
#[derive(Debug, Clone, Default)]
pub struct Stack {
    services: Vec<ServiceDef>,
}

impl Stack {
    /// Create a new empty stack
    pub fn new() -> Self {
        Self::default()
    }

    /// Add a service to the stack and return a builder for it
    pub fn service(mut self, name: &str) -> ServiceBuilder {
        self.services.push(ServiceDef::new(name));
        ServiceBuilder { stack: self }
    }

    /// Get the service definitions
    pub fn services(&self) -> &[ServiceDef] {
        &self.services
    }

    /// Generate Kubernetes Deployment for a service
    pub fn deployment_for(&self, svc: &ServiceDef, namespace: &str) -> Deployment {
        let labels: BTreeMap<String, String> = [("app".to_string(), svc.name.clone())]
            .into_iter()
            .collect();

        let mut container = Container {
            name: svc.name.clone(),
            image: Some(svc.image.clone()),
            ..Default::default()
        };

        if let Some(port) = svc.port {
            container.ports = Some(vec![ContainerPort {
                container_port: port,
                ..Default::default()
            }]);
        }

        Deployment {
            metadata: kube::api::ObjectMeta {
                name: Some(svc.name.clone()),
                namespace: Some(namespace.to_string()),
                labels: Some(labels.clone()),
                ..Default::default()
            },
            spec: Some(DeploymentSpec {
                replicas: Some(svc.replicas),
                selector: LabelSelector {
                    match_labels: Some(labels.clone()),
                    ..Default::default()
                },
                template: PodTemplateSpec {
                    metadata: Some(kube::api::ObjectMeta {
                        labels: Some(labels),
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

    /// Generate Kubernetes Service for a service definition (if it has a port)
    pub fn service_for(&self, svc: &ServiceDef, namespace: &str) -> Option<Service> {
        let port = svc.port?;

        let selector: BTreeMap<String, String> = [("app".to_string(), svc.name.clone())]
            .into_iter()
            .collect();

        Some(Service {
            metadata: kube::api::ObjectMeta {
                name: Some(svc.name.clone()),
                namespace: Some(namespace.to_string()),
                ..Default::default()
            },
            spec: Some(ServiceSpec {
                selector: Some(selector),
                ports: Some(vec![ServicePort {
                    port,
                    ..Default::default()
                }]),
                ..Default::default()
            }),
            ..Default::default()
        })
    }
}

/// Builder for configuring a service within a stack
#[derive(Debug, Clone)]
pub struct ServiceBuilder {
    stack: Stack,
}

impl ServiceBuilder {
    /// Set the container image for this service
    pub fn image(mut self, image: &str) -> Self {
        if let Some(svc) = self.stack.services.last_mut() {
            svc.image = image.to_string();
        }
        self
    }

    /// Set the number of replicas for this service
    pub fn replicas(mut self, count: i32) -> Self {
        if let Some(svc) = self.stack.services.last_mut() {
            svc.replicas = count;
        }
        self
    }

    /// Set the port for this service
    pub fn port(mut self, port: i32) -> Self {
        if let Some(svc) = self.stack.services.last_mut() {
            svc.port = Some(port);
        }
        self
    }

    /// Add another service to the stack
    pub fn service(self, name: &str) -> ServiceBuilder {
        self.stack.service(name)
    }

    /// Finish building and return the stack
    pub fn build(self) -> Stack {
        self.stack
    }
}

// Allow ServiceBuilder to be used as Stack
impl From<ServiceBuilder> for Stack {
    fn from(builder: ServiceBuilder) -> Self {
        builder.stack
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_stack_builder() {
        let stack = Stack::new()
            .service("frontend")
            .image("fe:test")
            .replicas(4)
            .port(80)
            .service("backend")
            .image("be:test")
            .replicas(2)
            .port(8080)
            .build();

        assert_eq!(stack.services().len(), 2);

        let frontend = &stack.services()[0];
        assert_eq!(frontend.name, "frontend");
        assert_eq!(frontend.image, "fe:test");
        assert_eq!(frontend.replicas, 4);
        assert_eq!(frontend.port, Some(80));

        let backend = &stack.services()[1];
        assert_eq!(backend.name, "backend");
        assert_eq!(backend.image, "be:test");
        assert_eq!(backend.replicas, 2);
        assert_eq!(backend.port, Some(8080));
    }

    #[test]
    fn test_deployment_generation() {
        let stack = Stack::new()
            .service("myapp")
            .image("myapp:v1")
            .replicas(3)
            .port(8080)
            .build();

        let svc = &stack.services()[0];
        let deployment = stack.deployment_for(svc, "test-ns");

        assert_eq!(deployment.metadata.name, Some("myapp".to_string()));
        assert_eq!(deployment.metadata.namespace, Some("test-ns".to_string()));
        assert_eq!(deployment.spec.as_ref().unwrap().replicas, Some(3));
    }

    #[test]
    fn test_service_generation() {
        let stack = Stack::new()
            .service("myapp")
            .image("myapp:v1")
            .port(8080)
            .build();

        let svc_def = &stack.services()[0];
        let k8s_svc = stack.service_for(svc_def, "test-ns").unwrap();

        assert_eq!(k8s_svc.metadata.name, Some("myapp".to_string()));
        assert_eq!(
            k8s_svc.spec.as_ref().unwrap().ports.as_ref().unwrap()[0].port,
            8080
        );
    }

    #[test]
    fn test_service_without_port() {
        let stack = Stack::new()
            .service("worker")
            .image("worker:v1")
            .replicas(5)
            .build();

        let svc_def = &stack.services()[0];
        let k8s_svc = stack.service_for(svc_def, "test-ns");

        assert!(k8s_svc.is_none()); // No service created without port
    }
}
