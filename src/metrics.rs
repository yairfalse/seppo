//! Metrics collection for Kubernetes resources
//!
//! Provides access to resource metrics from the metrics-server API.

/// Metrics data for a Kubernetes resource
#[derive(Debug, Clone)]
pub struct SeppoMetrics {
    /// CPU usage (e.g., "250m")
    pub cpu: String,
    /// Memory usage (e.g., "512Mi")
    pub memory: String,
    /// CPU usage as percentage of limit
    pub cpu_percent: f64,
    /// Memory usage as percentage of limit
    pub memory_percent: f64,
}

/// Create a metrics collector
///
/// TODO: This is a placeholder - full implementation pending
pub fn metrics() -> SeppoMetrics {
    SeppoMetrics {
        cpu: "0m".to_string(),
        memory: "0Mi".to_string(),
        cpu_percent: 0.0,
        memory_percent: 0.0,
    }
}
