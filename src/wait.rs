//! Rich wait errors with debugging context
//!
//! Provides detailed error information when wait operations timeout.
//!
//! # Example
//!
//! ```ignore
//! match ctx.wait_ready("deployment/myapp").await {
//!     Err(ContextError::WaitFailed(err)) => {
//!         println!("Resource: {}", err.resource);
//!         println!("Last state: {}", err.last_state);
//!         println!("Elapsed: {:?}", err.elapsed);
//!         for event in &err.events {
//!             println!("  {} - {}", event.reason, event.message);
//!         }
//!     }
//!     _ => {}
//! }
//! ```

use std::fmt;
use std::time::Duration;

/// A simplified event for wait error context
#[derive(Debug, Clone)]
pub struct WaitEvent {
    /// Event reason (e.g., "Pulling", "BackOff", "FailedScheduling")
    pub reason: String,
    /// Event message
    pub message: String,
    /// Timestamp as string
    pub timestamp: Option<String>,
}

/// Rich error context for wait operations
#[derive(Debug, Clone)]
pub struct WaitError {
    /// Resource reference (e.g., "deployment/myapp")
    pub resource: String,
    /// Description of the last observed state
    pub last_state: String,
    /// How long we waited before giving up
    pub elapsed: Duration,
    /// The timeout that was configured
    pub timeout: Duration,
    /// Recent events related to the resource
    pub events: Vec<WaitEvent>,
}

impl WaitError {
    /// Create a new WaitError
    pub fn new(resource: impl Into<String>, timeout: Duration, elapsed: Duration) -> Self {
        Self {
            resource: resource.into(),
            last_state: "unknown".to_string(),
            elapsed,
            timeout,
            events: Vec::new(),
        }
    }

    /// Set the last observed state
    pub fn with_state(mut self, state: impl Into<String>) -> Self {
        self.last_state = state.into();
        self
    }

    /// Add events to the error
    pub fn with_events(mut self, events: Vec<WaitEvent>) -> Self {
        self.events = events;
        self
    }
}

impl fmt::Display for WaitError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        writeln!(f)?;
        writeln!(f, "Wait timeout for {}", self.resource)?;
        writeln!(f, "├─ Last state: {}", self.last_state)?;
        writeln!(f, "├─ Elapsed: {:?}", self.elapsed)?;
        writeln!(f, "└─ Timeout: {:?}", self.timeout)?;

        if !self.events.is_empty() {
            writeln!(f)?;
            writeln!(f, "Recent events:")?;
            for (i, event) in self.events.iter().enumerate() {
                let prefix = if i == self.events.len() - 1 {
                    "└─"
                } else {
                    "├─"
                };
                let ts = event.timestamp.as_deref().unwrap_or("??:??:??");
                writeln!(f, "{} [{}] {}: {}", prefix, ts, event.reason, event.message)?;
            }
        }

        Ok(())
    }
}

impl std::error::Error for WaitError {}

/// Helper trait for extracting state description from K8s resources
pub trait ResourceState {
    /// Get a human-readable description of the resource's current state
    fn state_description(&self) -> String;
}

impl ResourceState for k8s_openapi::api::apps::v1::Deployment {
    fn state_description(&self) -> String {
        let spec_replicas = self.spec.as_ref().and_then(|s| s.replicas).unwrap_or(1);
        let ready = self
            .status
            .as_ref()
            .and_then(|s| s.ready_replicas)
            .unwrap_or(0);
        let available = self
            .status
            .as_ref()
            .and_then(|s| s.available_replicas)
            .unwrap_or(0);
        let unavailable = self
            .status
            .as_ref()
            .and_then(|s| s.unavailable_replicas)
            .unwrap_or(0);

        if unavailable > 0 {
            format!(
                "{}/{} ready, {} unavailable",
                ready, spec_replicas, unavailable
            )
        } else {
            format!(
                "{}/{} ready, {}/{} available",
                ready, spec_replicas, available, spec_replicas
            )
        }
    }
}

impl ResourceState for k8s_openapi::api::core::v1::Pod {
    fn state_description(&self) -> String {
        let phase = self
            .status
            .as_ref()
            .and_then(|s| s.phase.as_ref())
            .map(|s| s.as_str())
            .unwrap_or("Unknown");

        let containers = self
            .status
            .as_ref()
            .and_then(|s| s.container_statuses.as_ref());

        match containers {
            Some(statuses) => {
                let total = statuses.len();
                let ready = statuses.iter().filter(|c| c.ready).count();

                // Check for specific container states
                let waiting_reasons: Vec<&str> = statuses
                    .iter()
                    .filter_map(|c| {
                        c.state
                            .as_ref()
                            .and_then(|s| s.waiting.as_ref())
                            .and_then(|w| w.reason.as_deref())
                    })
                    .collect();

                if !waiting_reasons.is_empty() {
                    format!(
                        "phase={}, containers {}/{} ready, waiting: {}",
                        phase,
                        ready,
                        total,
                        waiting_reasons.join(", ")
                    )
                } else {
                    format!("phase={}, containers {}/{} ready", phase, ready, total)
                }
            }
            None => format!("phase={}, no container status", phase),
        }
    }
}

impl ResourceState for k8s_openapi::api::apps::v1::StatefulSet {
    fn state_description(&self) -> String {
        let spec_replicas = self.spec.as_ref().and_then(|s| s.replicas).unwrap_or(1);
        let ready = self
            .status
            .as_ref()
            .and_then(|s| s.ready_replicas)
            .unwrap_or(0);
        let current = self
            .status
            .as_ref()
            .map(|s| s.current_replicas)
            .unwrap_or(Some(0))
            .unwrap_or(0);

        format!(
            "{}/{} ready, {}/{} current",
            ready, spec_replicas, current, spec_replicas
        )
    }
}

impl ResourceState for k8s_openapi::api::apps::v1::DaemonSet {
    fn state_description(&self) -> String {
        let desired = self
            .status
            .as_ref()
            .map(|s| s.desired_number_scheduled)
            .unwrap_or(0);
        let ready = self.status.as_ref().map(|s| s.number_ready).unwrap_or(0);
        let available = self
            .status
            .as_ref()
            .and_then(|s| s.number_available)
            .unwrap_or(0);

        format!(
            "{}/{} ready, {}/{} available",
            ready, desired, available, desired
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_wait_error_display() {
        let err = WaitError::new(
            "deployment/myapp",
            Duration::from_secs(60),
            Duration::from_secs(60),
        )
        .with_state("0/3 ready, 3 unavailable");

        let output = err.to_string();
        assert!(output.contains("deployment/myapp"));
        assert!(output.contains("0/3 ready"));
        assert!(output.contains("60s"));
    }

    #[test]
    fn test_wait_error_with_events() {
        let events = vec![
            WaitEvent {
                reason: "Pulling".to_string(),
                message: "Pulling image nginx:latest".to_string(),
                timestamp: Some("10:42:01".to_string()),
            },
            WaitEvent {
                reason: "BackOff".to_string(),
                message: "Back-off pulling image".to_string(),
                timestamp: Some("10:42:30".to_string()),
            },
        ];

        let err = WaitError::new(
            "pod/myapp-xyz",
            Duration::from_secs(30),
            Duration::from_secs(30),
        )
        .with_state("phase=Pending, waiting: ImagePullBackOff")
        .with_events(events);

        let output = err.to_string();
        assert!(output.contains("pod/myapp-xyz"));
        assert!(output.contains("ImagePullBackOff"));
        assert!(output.contains("Pulling"));
        assert!(output.contains("BackOff"));
        assert!(output.contains("10:42:01"));
    }

    #[test]
    fn test_wait_error_builder() {
        let err = WaitError::new(
            "deployment/test",
            Duration::from_secs(120),
            Duration::from_secs(115),
        );

        assert_eq!(err.resource, "deployment/test");
        assert_eq!(err.timeout, Duration::from_secs(120));
        assert_eq!(err.elapsed, Duration::from_secs(115));
        assert_eq!(err.last_state, "unknown");
    }

    #[test]
    fn test_deployment_state_description() {
        use k8s_openapi::api::apps::v1::{Deployment, DeploymentSpec, DeploymentStatus};

        let deployment = Deployment {
            spec: Some(DeploymentSpec {
                replicas: Some(3),
                ..Default::default()
            }),
            status: Some(DeploymentStatus {
                ready_replicas: Some(1),
                available_replicas: Some(1),
                unavailable_replicas: Some(2),
                ..Default::default()
            }),
            ..Default::default()
        };

        let state = deployment.state_description();
        assert!(state.contains("1/3 ready"));
        assert!(state.contains("2 unavailable"));
    }

    #[test]
    fn test_pod_state_description() {
        use k8s_openapi::api::core::v1::{
            ContainerState, ContainerStateWaiting, ContainerStatus, Pod, PodStatus,
        };

        let pod = Pod {
            status: Some(PodStatus {
                phase: Some("Pending".to_string()),
                container_statuses: Some(vec![ContainerStatus {
                    name: "main".to_string(),
                    ready: false,
                    state: Some(ContainerState {
                        waiting: Some(ContainerStateWaiting {
                            reason: Some("ImagePullBackOff".to_string()),
                            ..Default::default()
                        }),
                        ..Default::default()
                    }),
                    ..Default::default()
                }]),
                ..Default::default()
            }),
            ..Default::default()
        };

        let state = pod.state_description();
        assert!(state.contains("phase=Pending"));
        assert!(state.contains("0/1 ready"));
        assert!(state.contains("ImagePullBackOff"));
    }
}
