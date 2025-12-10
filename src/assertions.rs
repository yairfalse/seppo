//! Semantic assertions for Kubernetes resources
//!
//! Provides fluent assertions for testing K8s resource state.
//!
//! # Example
//!
//! ```ignore
//! use seppo::assertions::PodAssertion;
//!
//! // Assert pod state
//! ctx.assert_pod("my-pod").is_running().await?;
//! ctx.assert_pod("my-pod").has_label("app", "myapp").await?;
//!
//! // Assert deployment state
//! ctx.assert_deployment("my-app").has_replicas(3).await?;
//! ctx.assert_deployment("my-app").is_available().await?;
//!
//! // Assert service state
//! ctx.assert_service("my-svc").has_port(8080).await?;
//! ```

use k8s_openapi::api::apps::v1::Deployment;
use k8s_openapi::api::core::v1::{Pod, Service};
use kube::api::Api;
use kube::Client;

/// Error type for assertion failures
#[derive(Debug, thiserror::Error)]
pub enum AssertionError {
    #[error("resource not found: {0}")]
    NotFound(String),

    #[error("assertion failed: {message}")]
    Failed { message: String },

    #[error("kubernetes error: {0}")]
    KubeError(String),
}

/// Pod assertion builder
pub struct PodAssertion {
    client: Client,
    namespace: String,
    name: String,
}

/// Deployment assertion builder
pub struct DeploymentAssertion {
    client: Client,
    namespace: String,
    name: String,
}

/// Service assertion builder
pub struct ServiceAssertion {
    client: Client,
    namespace: String,
    name: String,
}

impl PodAssertion {
    pub(crate) fn new(client: Client, namespace: String, name: String) -> Self {
        Self {
            client,
            namespace,
            name,
        }
    }

    async fn get_pod(&self) -> Result<Pod, AssertionError> {
        let pods: Api<Pod> = Api::namespaced(self.client.clone(), &self.namespace);
        pods.get(&self.name).await.map_err(|e| match e {
            kube::Error::Api(ae) if ae.code == 404 => {
                AssertionError::NotFound(format!("pod/{}", self.name))
            }
            _ => AssertionError::KubeError(e.to_string()),
        })
    }

    /// Assert the pod is in Running phase
    pub async fn is_running(&self) -> Result<(), AssertionError> {
        let pod = self.get_pod().await?;
        let phase = pod
            .status
            .as_ref()
            .and_then(|s| s.phase.as_ref())
            .map(|s| s.as_str())
            .unwrap_or("Unknown");

        if phase == "Running" {
            Ok(())
        } else {
            Err(AssertionError::Failed {
                message: format!("expected pod/{} to be Running, got {}", self.name, phase),
            })
        }
    }

    /// Assert the pod is in Pending phase
    pub async fn is_pending(&self) -> Result<(), AssertionError> {
        let pod = self.get_pod().await?;
        let phase = pod
            .status
            .as_ref()
            .and_then(|s| s.phase.as_ref())
            .map(|s| s.as_str())
            .unwrap_or("Unknown");

        if phase == "Pending" {
            Ok(())
        } else {
            Err(AssertionError::Failed {
                message: format!("expected pod/{} to be Pending, got {}", self.name, phase),
            })
        }
    }

    /// Assert the pod has a specific label
    pub async fn has_label(&self, key: &str, value: &str) -> Result<(), AssertionError> {
        let pod = self.get_pod().await?;
        let labels = pod.metadata.labels.as_ref();

        match labels.and_then(|l| l.get(key)) {
            Some(v) if v == value => Ok(()),
            Some(v) => Err(AssertionError::Failed {
                message: format!(
                    "expected pod/{} to have label {}={}, got {}={}",
                    self.name, key, value, key, v
                ),
            }),
            None => Err(AssertionError::Failed {
                message: format!(
                    "expected pod/{} to have label {}, not found",
                    self.name, key
                ),
            }),
        }
    }

    /// Assert the pod has a specific annotation
    pub async fn has_annotation(&self, key: &str, value: &str) -> Result<(), AssertionError> {
        let pod = self.get_pod().await?;
        let annotations = pod.metadata.annotations.as_ref();

        match annotations.and_then(|a| a.get(key)) {
            Some(v) if v == value => Ok(()),
            Some(v) => Err(AssertionError::Failed {
                message: format!(
                    "expected pod/{} to have annotation {}={}, got {}={}",
                    self.name, key, value, key, v
                ),
            }),
            None => Err(AssertionError::Failed {
                message: format!(
                    "expected pod/{} to have annotation {}, not found",
                    self.name, key
                ),
            }),
        }
    }

    /// Assert all containers in the pod are ready
    pub async fn containers_ready(&self) -> Result<(), AssertionError> {
        let pod = self.get_pod().await?;
        let container_statuses = pod
            .status
            .as_ref()
            .and_then(|s| s.container_statuses.as_ref());

        match container_statuses {
            Some(statuses) => {
                for status in statuses {
                    if !status.ready {
                        return Err(AssertionError::Failed {
                            message: format!(
                                "container {} in pod/{} is not ready",
                                status.name, self.name
                            ),
                        });
                    }
                }
                Ok(())
            }
            None => Err(AssertionError::Failed {
                message: format!("pod/{} has no container statuses", self.name),
            }),
        }
    }
}

impl DeploymentAssertion {
    pub(crate) fn new(client: Client, namespace: String, name: String) -> Self {
        Self {
            client,
            namespace,
            name,
        }
    }

    async fn get_deployment(&self) -> Result<Deployment, AssertionError> {
        let deployments: Api<Deployment> = Api::namespaced(self.client.clone(), &self.namespace);
        deployments.get(&self.name).await.map_err(|e| match e {
            kube::Error::Api(ae) if ae.code == 404 => {
                AssertionError::NotFound(format!("deployment/{}", self.name))
            }
            _ => AssertionError::KubeError(e.to_string()),
        })
    }

    /// Assert the deployment has the expected number of replicas
    pub async fn has_replicas(&self, expected: i32) -> Result<(), AssertionError> {
        let deployment = self.get_deployment().await?;
        let actual = deployment
            .status
            .as_ref()
            .and_then(|s| s.ready_replicas)
            .unwrap_or(0);

        if actual == expected {
            Ok(())
        } else {
            Err(AssertionError::Failed {
                message: format!(
                    "expected deployment/{} to have {} ready replicas, got {}",
                    self.name, expected, actual
                ),
            })
        }
    }

    /// Assert the deployment is available (has minimum availability)
    pub async fn is_available(&self) -> Result<(), AssertionError> {
        let deployment = self.get_deployment().await?;
        let conditions = deployment
            .status
            .as_ref()
            .and_then(|s| s.conditions.as_ref());

        match conditions {
            Some(conds) => {
                let available = conds
                    .iter()
                    .find(|c| c.type_ == "Available")
                    .map(|c| c.status == "True")
                    .unwrap_or(false);

                if available {
                    Ok(())
                } else {
                    Err(AssertionError::Failed {
                        message: format!("deployment/{} is not available", self.name),
                    })
                }
            }
            None => Err(AssertionError::Failed {
                message: format!("deployment/{} has no conditions", self.name),
            }),
        }
    }

    /// Assert the deployment has a specific label
    pub async fn has_label(&self, key: &str, value: &str) -> Result<(), AssertionError> {
        let deployment = self.get_deployment().await?;
        let labels = deployment.metadata.labels.as_ref();

        match labels.and_then(|l| l.get(key)) {
            Some(v) if v == value => Ok(()),
            Some(v) => Err(AssertionError::Failed {
                message: format!(
                    "expected deployment/{} to have label {}={}, got {}={}",
                    self.name, key, value, key, v
                ),
            }),
            None => Err(AssertionError::Failed {
                message: format!(
                    "expected deployment/{} to have label {}, not found",
                    self.name, key
                ),
            }),
        }
    }
}

impl ServiceAssertion {
    pub(crate) fn new(client: Client, namespace: String, name: String) -> Self {
        Self {
            client,
            namespace,
            name,
        }
    }

    async fn get_service(&self) -> Result<Service, AssertionError> {
        let services: Api<Service> = Api::namespaced(self.client.clone(), &self.namespace);
        services.get(&self.name).await.map_err(|e| match e {
            kube::Error::Api(ae) if ae.code == 404 => {
                AssertionError::NotFound(format!("service/{}", self.name))
            }
            _ => AssertionError::KubeError(e.to_string()),
        })
    }

    /// Assert the service exposes a specific port
    pub async fn has_port(&self, port: i32) -> Result<(), AssertionError> {
        let service = self.get_service().await?;
        let ports = service.spec.as_ref().and_then(|s| s.ports.as_ref());

        match ports {
            Some(ports) => {
                let has_port = ports.iter().any(|p| p.port == port);
                if has_port {
                    Ok(())
                } else {
                    let available: Vec<i32> = ports.iter().map(|p| p.port).collect();
                    Err(AssertionError::Failed {
                        message: format!(
                            "expected service/{} to have port {}, available ports: {:?}",
                            self.name, port, available
                        ),
                    })
                }
            }
            None => Err(AssertionError::Failed {
                message: format!("service/{} has no ports defined", self.name),
            }),
        }
    }

    /// Assert the service is of type ClusterIP
    pub async fn is_cluster_ip(&self) -> Result<(), AssertionError> {
        let service = self.get_service().await?;
        let svc_type = service
            .spec
            .as_ref()
            .and_then(|s| s.type_.as_ref())
            .map(|s| s.as_str())
            .unwrap_or("ClusterIP"); // default is ClusterIP

        if svc_type == "ClusterIP" {
            Ok(())
        } else {
            Err(AssertionError::Failed {
                message: format!(
                    "expected service/{} to be ClusterIP, got {}",
                    self.name, svc_type
                ),
            })
        }
    }

    /// Assert the service is of type LoadBalancer
    pub async fn is_load_balancer(&self) -> Result<(), AssertionError> {
        let service = self.get_service().await?;
        let svc_type = service
            .spec
            .as_ref()
            .and_then(|s| s.type_.as_ref())
            .map(|s| s.as_str())
            .unwrap_or("ClusterIP");

        if svc_type == "LoadBalancer" {
            Ok(())
        } else {
            Err(AssertionError::Failed {
                message: format!(
                    "expected service/{} to be LoadBalancer, got {}",
                    self.name, svc_type
                ),
            })
        }
    }

    /// Assert the service is of type NodePort
    pub async fn is_node_port(&self) -> Result<(), AssertionError> {
        let service = self.get_service().await?;
        let svc_type = service
            .spec
            .as_ref()
            .and_then(|s| s.type_.as_ref())
            .map(|s| s.as_str())
            .unwrap_or("ClusterIP");

        if svc_type == "NodePort" {
            Ok(())
        } else {
            Err(AssertionError::Failed {
                message: format!(
                    "expected service/{} to be NodePort, got {}",
                    self.name, svc_type
                ),
            })
        }
    }

    /// Assert the service has a specific selector
    pub async fn has_selector(&self, key: &str, value: &str) -> Result<(), AssertionError> {
        let service = self.get_service().await?;
        let selector = service.spec.as_ref().and_then(|s| s.selector.as_ref());

        match selector.and_then(|s| s.get(key)) {
            Some(v) if v == value => Ok(()),
            Some(v) => Err(AssertionError::Failed {
                message: format!(
                    "expected service/{} to have selector {}={}, got {}={}",
                    self.name, key, value, key, v
                ),
            }),
            None => Err(AssertionError::Failed {
                message: format!(
                    "expected service/{} to have selector {}, not found",
                    self.name, key
                ),
            }),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use k8s_openapi::api::apps::v1::{DeploymentCondition, DeploymentStatus};
    use k8s_openapi::api::core::v1::{ContainerStatus, PodStatus, ServicePort, ServiceSpec};
    use k8s_openapi::apimachinery::pkg::apis::meta::v1::ObjectMeta;
    use std::collections::BTreeMap;

    #[test]
    fn test_assertion_error_display() {
        let err = AssertionError::NotFound("pod/test".to_string());
        assert_eq!(err.to_string(), "resource not found: pod/test");

        let err = AssertionError::Failed {
            message: "expected Running, got Pending".to_string(),
        };
        assert_eq!(
            err.to_string(),
            "assertion failed: expected Running, got Pending"
        );

        let err = AssertionError::KubeError("connection refused".to_string());
        assert_eq!(err.to_string(), "kubernetes error: connection refused");
    }

    // Helper to create a Pod with specific phase
    fn make_pod_with_phase(phase: &str) -> Pod {
        Pod {
            status: Some(PodStatus {
                phase: Some(phase.to_string()),
                ..Default::default()
            }),
            ..Default::default()
        }
    }

    // Helper to create a Pod with labels
    fn make_pod_with_labels(labels: &[(&str, &str)]) -> Pod {
        let mut label_map = BTreeMap::new();
        for (k, v) in labels {
            label_map.insert(k.to_string(), v.to_string());
        }
        Pod {
            metadata: ObjectMeta {
                labels: Some(label_map),
                ..Default::default()
            },
            ..Default::default()
        }
    }

    // Helper to create a Pod with container statuses
    fn make_pod_with_containers(statuses: &[(&str, bool)]) -> Pod {
        let container_statuses: Vec<ContainerStatus> = statuses
            .iter()
            .map(|(name, ready)| ContainerStatus {
                name: name.to_string(),
                ready: *ready,
                ..Default::default()
            })
            .collect();
        Pod {
            status: Some(PodStatus {
                phase: Some("Running".to_string()),
                container_statuses: Some(container_statuses),
                ..Default::default()
            }),
            ..Default::default()
        }
    }

    #[test]
    fn test_pod_phase_running_check() {
        let running_pod = make_pod_with_phase("Running");
        let phase = running_pod
            .status
            .as_ref()
            .and_then(|s| s.phase.as_ref())
            .map(|s| s.as_str())
            .unwrap_or("Unknown");
        assert_eq!(phase, "Running");
    }

    #[test]
    fn test_pod_phase_pending_check() {
        let pending_pod = make_pod_with_phase("Pending");
        let phase = pending_pod
            .status
            .as_ref()
            .and_then(|s| s.phase.as_ref())
            .map(|s| s.as_str())
            .unwrap_or("Unknown");
        assert_eq!(phase, "Pending");
    }

    #[test]
    fn test_pod_phase_unknown_when_missing() {
        let pod = Pod::default();
        let phase = pod
            .status
            .as_ref()
            .and_then(|s| s.phase.as_ref())
            .map(|s| s.as_str())
            .unwrap_or("Unknown");
        assert_eq!(phase, "Unknown");
    }

    #[test]
    fn test_pod_label_check() {
        let pod = make_pod_with_labels(&[("app", "myapp"), ("env", "prod")]);
        let labels = pod.metadata.labels.as_ref().unwrap();

        assert_eq!(labels.get("app"), Some(&"myapp".to_string()));
        assert_eq!(labels.get("env"), Some(&"prod".to_string()));
        assert_eq!(labels.get("nonexistent"), None);
    }

    #[test]
    fn test_pod_containers_ready_check() {
        let pod = make_pod_with_containers(&[("main", true), ("sidecar", true)]);
        let statuses = pod
            .status
            .as_ref()
            .and_then(|s| s.container_statuses.as_ref())
            .unwrap();

        assert!(statuses.iter().all(|c| c.ready));
    }

    #[test]
    fn test_pod_containers_not_ready_check() {
        let pod = make_pod_with_containers(&[("main", true), ("sidecar", false)]);
        let statuses = pod
            .status
            .as_ref()
            .and_then(|s| s.container_statuses.as_ref())
            .unwrap();

        let not_ready: Vec<_> = statuses.iter().filter(|c| !c.ready).collect();
        assert_eq!(not_ready.len(), 1);
        assert_eq!(not_ready[0].name, "sidecar");
    }

    #[test]
    fn test_deployment_replicas_check() {
        let deployment = Deployment {
            status: Some(DeploymentStatus {
                ready_replicas: Some(3),
                ..Default::default()
            }),
            ..Default::default()
        };

        let ready = deployment
            .status
            .as_ref()
            .and_then(|s| s.ready_replicas)
            .unwrap_or(0);
        assert_eq!(ready, 3);
    }

    #[test]
    fn test_deployment_available_condition() {
        let deployment = Deployment {
            status: Some(DeploymentStatus {
                conditions: Some(vec![DeploymentCondition {
                    type_: "Available".to_string(),
                    status: "True".to_string(),
                    ..Default::default()
                }]),
                ..Default::default()
            }),
            ..Default::default()
        };

        let is_available = deployment
            .status
            .as_ref()
            .and_then(|s| s.conditions.as_ref())
            .and_then(|conds| conds.iter().find(|c| c.type_ == "Available"))
            .map(|c| c.status == "True")
            .unwrap_or(false);

        assert!(is_available);
    }

    #[test]
    fn test_deployment_not_available_condition() {
        let deployment = Deployment {
            status: Some(DeploymentStatus {
                conditions: Some(vec![DeploymentCondition {
                    type_: "Available".to_string(),
                    status: "False".to_string(),
                    ..Default::default()
                }]),
                ..Default::default()
            }),
            ..Default::default()
        };

        let is_available = deployment
            .status
            .as_ref()
            .and_then(|s| s.conditions.as_ref())
            .and_then(|conds| conds.iter().find(|c| c.type_ == "Available"))
            .map(|c| c.status == "True")
            .unwrap_or(false);

        assert!(!is_available);
    }

    #[test]
    fn test_service_port_check() {
        let service = Service {
            spec: Some(ServiceSpec {
                ports: Some(vec![
                    ServicePort {
                        port: 80,
                        ..Default::default()
                    },
                    ServicePort {
                        port: 443,
                        ..Default::default()
                    },
                ]),
                ..Default::default()
            }),
            ..Default::default()
        };

        let ports = service
            .spec
            .as_ref()
            .and_then(|s| s.ports.as_ref())
            .unwrap();

        assert!(ports.iter().any(|p| p.port == 80));
        assert!(ports.iter().any(|p| p.port == 443));
        assert!(!ports.iter().any(|p| p.port == 8080));
    }

    #[test]
    fn test_service_type_check() {
        let cluster_ip = Service {
            spec: Some(ServiceSpec {
                type_: Some("ClusterIP".to_string()),
                ..Default::default()
            }),
            ..Default::default()
        };

        let lb = Service {
            spec: Some(ServiceSpec {
                type_: Some("LoadBalancer".to_string()),
                ..Default::default()
            }),
            ..Default::default()
        };

        let default_service = Service::default();

        // ClusterIP check
        let svc_type = cluster_ip
            .spec
            .as_ref()
            .and_then(|s| s.type_.as_ref())
            .map(|s| s.as_str())
            .unwrap_or("ClusterIP");
        assert_eq!(svc_type, "ClusterIP");

        // LoadBalancer check
        let svc_type = lb
            .spec
            .as_ref()
            .and_then(|s| s.type_.as_ref())
            .map(|s| s.as_str())
            .unwrap_or("ClusterIP");
        assert_eq!(svc_type, "LoadBalancer");

        // Default (nil spec) should default to ClusterIP
        let svc_type = default_service
            .spec
            .as_ref()
            .and_then(|s| s.type_.as_ref())
            .map(|s| s.as_str())
            .unwrap_or("ClusterIP");
        assert_eq!(svc_type, "ClusterIP");
    }

    #[test]
    fn test_service_selector_check() {
        let mut selector = BTreeMap::new();
        selector.insert("app".to_string(), "myapp".to_string());

        let service = Service {
            spec: Some(ServiceSpec {
                selector: Some(selector),
                ..Default::default()
            }),
            ..Default::default()
        };

        let sel = service.spec.as_ref().and_then(|s| s.selector.as_ref());
        assert_eq!(sel.and_then(|s| s.get("app")), Some(&"myapp".to_string()));
        assert_eq!(sel.and_then(|s| s.get("missing")), None);
    }

    #[test]
    fn test_error_message_pod_running() {
        // Test error message format for pod phase assertions
        let name = "my-pod";
        let phase = "Pending";
        let err = AssertionError::Failed {
            message: format!("expected pod/{} to be Running, got {}", name, phase),
        };
        assert!(err.to_string().contains("pod/my-pod"));
        assert!(err.to_string().contains("Running"));
        assert!(err.to_string().contains("Pending"));
    }

    #[test]
    fn test_error_message_label_mismatch() {
        let name = "my-pod";
        let key = "app";
        let expected = "frontend";
        let actual = "backend";
        let err = AssertionError::Failed {
            message: format!(
                "expected pod/{} to have label {}={}, got {}={}",
                name, key, expected, key, actual
            ),
        };
        assert!(err.to_string().contains("pod/my-pod"));
        assert!(err.to_string().contains("app=frontend"));
        assert!(err.to_string().contains("app=backend"));
    }

    #[test]
    fn test_error_message_container_not_ready() {
        let container_name = "sidecar";
        let pod_name = "my-pod";
        let err = AssertionError::Failed {
            message: format!(
                "container {} in pod/{} is not ready",
                container_name, pod_name
            ),
        };
        assert!(err.to_string().contains("sidecar"));
        assert!(err.to_string().contains("pod/my-pod"));
        assert!(err.to_string().contains("not ready"));
    }
}
