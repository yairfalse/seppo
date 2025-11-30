//! Metrics for test orchestration
//!
//! Provides OpenTelemetry metrics for monitoring Seppo operations,
//! following the same patterns as ahti and tapio.

use opentelemetry::metrics::{Counter, Histogram, Meter};
use opentelemetry::{global, KeyValue};
use std::sync::OnceLock;

static METRICS: OnceLock<SeppoMetrics> = OnceLock::new();

/// Get the global metrics instance
pub fn metrics() -> &'static SeppoMetrics {
    METRICS.get_or_init(|| SeppoMetrics::new(global::meter("seppo")))
}

/// Seppo metrics for test orchestration
pub struct SeppoMetrics {
    // Cluster operations
    /// Total cluster create operations
    pub clusters_created: Counter<u64>,
    /// Total cluster delete operations
    pub clusters_deleted: Counter<u64>,
    /// Total images loaded into clusters
    pub images_loaded: Counter<u64>,
    /// Cluster operation duration in milliseconds
    pub cluster_operation_duration: Histogram<f64>,

    // Environment setup
    /// Total manifests applied
    pub manifests_applied: Counter<u64>,
    /// Total wait conditions checked
    pub wait_conditions_checked: Counter<u64>,
    /// Total setup scripts executed
    pub setup_scripts_executed: Counter<u64>,
    /// Environment setup duration in milliseconds
    pub environment_setup_duration: Histogram<f64>,

    // Test execution
    /// Total test runs executed
    pub tests_executed: Counter<u64>,
    /// Total test runs passed
    pub tests_passed: Counter<u64>,
    /// Total test runs failed
    pub tests_failed: Counter<u64>,
    /// Test execution duration in milliseconds
    pub test_duration: Histogram<f64>,

    // Errors
    /// Total errors by type
    pub errors_total: Counter<u64>,
}

impl SeppoMetrics {
    /// Create new metrics instance
    pub fn new(meter: Meter) -> Self {
        Self {
            // Cluster operations
            clusters_created: meter
                .u64_counter("seppo_clusters_created_total")
                .with_description("Total number of clusters created")
                .build(),
            clusters_deleted: meter
                .u64_counter("seppo_clusters_deleted_total")
                .with_description("Total number of clusters deleted")
                .build(),
            images_loaded: meter
                .u64_counter("seppo_images_loaded_total")
                .with_description("Total number of images loaded into clusters")
                .build(),
            cluster_operation_duration: meter
                .f64_histogram("seppo_cluster_operation_duration_ms")
                .with_description("Cluster operation duration in milliseconds")
                .build(),

            // Environment setup
            manifests_applied: meter
                .u64_counter("seppo_manifests_applied_total")
                .with_description("Total number of manifests applied")
                .build(),
            wait_conditions_checked: meter
                .u64_counter("seppo_wait_conditions_checked_total")
                .with_description("Total number of wait conditions checked")
                .build(),
            setup_scripts_executed: meter
                .u64_counter("seppo_setup_scripts_executed_total")
                .with_description("Total number of setup scripts executed")
                .build(),
            environment_setup_duration: meter
                .f64_histogram("seppo_environment_setup_duration_ms")
                .with_description("Environment setup duration in milliseconds")
                .build(),

            // Test execution
            tests_executed: meter
                .u64_counter("seppo_tests_executed_total")
                .with_description("Total number of tests executed")
                .build(),
            tests_passed: meter
                .u64_counter("seppo_tests_passed_total")
                .with_description("Total number of tests passed")
                .build(),
            tests_failed: meter
                .u64_counter("seppo_tests_failed_total")
                .with_description("Total number of tests failed")
                .build(),
            test_duration: meter
                .f64_histogram("seppo_test_duration_ms")
                .with_description("Test execution duration in milliseconds")
                .build(),

            // Errors
            errors_total: meter
                .u64_counter("seppo_errors_total")
                .with_description("Total number of errors by type")
                .build(),
        }
    }

    // Convenience methods for recording metrics

    /// Record a cluster creation
    pub fn record_cluster_created(&self, provider: &str, duration_ms: f64) {
        let attrs = &[KeyValue::new("provider", provider.to_string())];
        self.clusters_created.add(1, attrs);
        self.cluster_operation_duration.record(
            duration_ms,
            &[
                KeyValue::new("provider", provider.to_string()),
                KeyValue::new("operation", "create"),
            ],
        );
    }

    /// Record a cluster deletion
    pub fn record_cluster_deleted(&self, provider: &str, duration_ms: f64) {
        let attrs = &[KeyValue::new("provider", provider.to_string())];
        self.clusters_deleted.add(1, attrs);
        self.cluster_operation_duration.record(
            duration_ms,
            &[
                KeyValue::new("provider", provider.to_string()),
                KeyValue::new("operation", "delete"),
            ],
        );
    }

    /// Record an image load
    pub fn record_image_loaded(&self, provider: &str) {
        self.images_loaded
            .add(1, &[KeyValue::new("provider", provider.to_string())]);
    }

    /// Record manifest application
    pub fn record_manifest_applied(&self, success: bool) {
        self.manifests_applied
            .add(1, &[KeyValue::new("success", success)]);
    }

    /// Record wait condition check
    pub fn record_wait_condition(&self, condition: &str, success: bool) {
        self.wait_conditions_checked.add(
            1,
            &[
                KeyValue::new("condition", condition.to_string()),
                KeyValue::new("success", success),
            ],
        );
    }

    /// Record setup script execution
    pub fn record_setup_script(&self, success: bool) {
        self.setup_scripts_executed
            .add(1, &[KeyValue::new("success", success)]);
    }

    /// Record environment setup duration
    pub fn record_environment_setup(&self, duration_ms: f64, success: bool) {
        self.environment_setup_duration
            .record(duration_ms, &[KeyValue::new("success", success)]);
    }

    /// Record a test execution
    pub fn record_test(&self, name: &str, passed: bool, duration_ms: f64) {
        let attrs = &[KeyValue::new("test_name", name.to_string())];
        self.tests_executed.add(1, attrs);

        if passed {
            self.tests_passed.add(1, attrs);
        } else {
            self.tests_failed.add(1, attrs);
        }

        self.test_duration.record(
            duration_ms,
            &[
                KeyValue::new("test_name", name.to_string()),
                KeyValue::new("passed", passed),
            ],
        );
    }

    /// Record an error
    pub fn record_error(&self, error_type: &str, operation: &str) {
        self.errors_total.add(
            1,
            &[
                KeyValue::new("error_type", error_type.to_string()),
                KeyValue::new("operation", operation.to_string()),
            ],
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_metrics_creation() {
        let meter = global::meter("test");
        let metrics = SeppoMetrics::new(meter);

        // Just verify metrics can be created without panicking
        metrics.record_cluster_created("kind", 1000.0);
        metrics.record_test("my_test", true, 500.0);
        metrics.record_error("timeout", "create_cluster");
    }
}
