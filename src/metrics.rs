//! Metrics collection for Kubernetes resources
//!
//! Provides access to resource metrics from the metrics-server API.
//!
//! # Example
//!
//! ```ignore
//! let metrics = ctx.metrics("pod/api-xyz").await?;
//!
//! println!("CPU: {}", metrics.cpu);           // "250m"
//! println!("Memory: {}", metrics.memory);     // "512Mi"
//! println!("CPU %: {}", metrics.cpu_percent); // 25.0
//! println!("Mem %: {}", metrics.memory_percent); // 50.0
//! ```

use kube::api::{Api, DynamicObject};
use kube::discovery::ApiResource;
use kube::Client;
use serde::Deserialize;

/// Error type for metrics operations
#[derive(Debug, thiserror::Error)]
pub enum MetricsError {
    #[error("metrics-server not available: {0}")]
    ServerNotAvailable(String),

    #[error("pod not found: {0}")]
    PodNotFound(String),

    #[error("invalid resource format: {0}")]
    InvalidFormat(String),

    #[error("kubernetes error: {0}")]
    KubeError(String),
}

/// Metrics data for a Kubernetes resource
#[derive(Debug, Clone)]
pub struct PodMetrics {
    /// Pod name
    pub name: String,
    /// CPU usage (e.g., "250m")
    pub cpu: String,
    /// Memory usage (e.g., "512Mi")
    pub memory: String,
    /// CPU usage as percentage of limit (0.0 if no limit set)
    pub cpu_percent: f64,
    /// Memory usage as percentage of limit (0.0 if no limit set)
    pub memory_percent: f64,
}

impl PodMetrics {
    /// Create metrics from raw API response
    pub(crate) fn from_raw(name: String, cpu: String, memory: String) -> Self {
        Self {
            name,
            cpu,
            memory,
            cpu_percent: 0.0, // Percentage requires fetching pod limits
            memory_percent: 0.0,
        }
    }
}

/// Response container from metrics-server API
#[derive(Debug, Deserialize)]
struct ContainerMetrics {
    usage: ResourceUsage,
}

#[derive(Debug, Deserialize)]
struct ResourceUsage {
    cpu: String,
    memory: String,
}

/// Fetch metrics for a pod from the metrics-server API
pub(crate) async fn fetch_pod_metrics(
    client: &Client,
    namespace: &str,
    pod_name: &str,
) -> Result<PodMetrics, MetricsError> {
    // Define the PodMetrics API resource
    let ar = ApiResource {
        group: "metrics.k8s.io".to_string(),
        version: "v1beta1".to_string(),
        kind: "PodMetrics".to_string(),
        api_version: "metrics.k8s.io/v1beta1".to_string(),
        plural: "pods".to_string(),
    };

    let api: Api<DynamicObject> = Api::namespaced_with(client.clone(), namespace, &ar);

    let obj = api.get(pod_name).await.map_err(|e| {
        let msg = e.to_string();
        if msg.contains("404") || msg.contains("not found") {
            MetricsError::PodNotFound(pod_name.to_string())
        } else if msg.contains("metrics.k8s.io") || msg.contains("the server could not find") {
            MetricsError::ServerNotAvailable(msg)
        } else {
            MetricsError::KubeError(msg)
        }
    })?;

    // Parse containers from the dynamic object
    let containers: Vec<ContainerMetrics> = obj
        .data
        .get("containers")
        .and_then(|v| serde_json::from_value(v.clone()).ok())
        .unwrap_or_default();

    // Aggregate container metrics
    let mut total_cpu = 0u64;
    let mut total_memory = 0u64;

    for container in &containers {
        total_cpu += parse_cpu(&container.usage.cpu);
        total_memory += parse_memory(&container.usage.memory);
    }

    let name = obj.metadata.name.unwrap_or_else(|| pod_name.to_string());

    Ok(PodMetrics::from_raw(
        name,
        format_cpu(total_cpu),
        format_memory(total_memory),
    ))
}

/// Parse CPU value like "250m" or "1" into nanocores
fn parse_cpu(cpu: &str) -> u64 {
    if cpu.ends_with('n') {
        cpu.trim_end_matches('n').parse().unwrap_or(0)
    } else if cpu.ends_with('m') {
        cpu.trim_end_matches('m').parse::<u64>().unwrap_or(0) * 1_000_000
    } else {
        cpu.parse::<u64>().unwrap_or(0) * 1_000_000_000
    }
}

/// Format nanocores as human-readable (e.g., "250m")
fn format_cpu(nanocores: u64) -> String {
    if nanocores >= 1_000_000_000 {
        format!("{}", nanocores / 1_000_000_000)
    } else {
        format!("{}m", nanocores / 1_000_000)
    }
}

/// Parse memory value like "512Mi" or "1Gi" into bytes
fn parse_memory(memory: &str) -> u64 {
    if memory.ends_with("Ki") {
        memory.trim_end_matches("Ki").parse::<u64>().unwrap_or(0) * 1024
    } else if memory.ends_with("Mi") {
        memory.trim_end_matches("Mi").parse::<u64>().unwrap_or(0) * 1024 * 1024
    } else if memory.ends_with("Gi") {
        memory.trim_end_matches("Gi").parse::<u64>().unwrap_or(0) * 1024 * 1024 * 1024
    } else {
        memory.parse().unwrap_or(0)
    }
}

/// Format bytes as human-readable (e.g., "512Mi")
fn format_memory(bytes: u64) -> String {
    if bytes >= 1024 * 1024 * 1024 {
        format!("{}Gi", bytes / (1024 * 1024 * 1024))
    } else if bytes >= 1024 * 1024 {
        format!("{}Mi", bytes / (1024 * 1024))
    } else if bytes >= 1024 {
        format!("{}Ki", bytes / 1024)
    } else {
        format!("{bytes}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_cpu_millicores() {
        assert_eq!(parse_cpu("250m"), 250_000_000);
        assert_eq!(parse_cpu("1000m"), 1_000_000_000);
    }

    #[test]
    fn test_parse_cpu_cores() {
        assert_eq!(parse_cpu("1"), 1_000_000_000);
        assert_eq!(parse_cpu("2"), 2_000_000_000);
    }

    #[test]
    fn test_parse_cpu_nanocores() {
        assert_eq!(parse_cpu("250000000n"), 250_000_000);
    }

    #[test]
    fn test_format_cpu() {
        assert_eq!(format_cpu(250_000_000), "250m");
        assert_eq!(format_cpu(1_000_000_000), "1");
        assert_eq!(format_cpu(1_500_000_000), "1");
    }

    #[test]
    fn test_parse_memory() {
        assert_eq!(parse_memory("512Mi"), 512 * 1024 * 1024);
        assert_eq!(parse_memory("1Gi"), 1024 * 1024 * 1024);
        assert_eq!(parse_memory("1024Ki"), 1024 * 1024);
    }

    #[test]
    fn test_format_memory() {
        assert_eq!(format_memory(512 * 1024 * 1024), "512Mi");
        assert_eq!(format_memory(1024 * 1024 * 1024), "1Gi");
        assert_eq!(format_memory(1024 * 1024), "1Mi");
    }
}
