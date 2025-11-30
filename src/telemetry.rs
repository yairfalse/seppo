//! OpenTelemetry integration for Seppo
//!
//! Provides tracing and metrics export via OTLP, following the same patterns
//! as ahti and tapio for consistent observability across the stack.
//!
//! # Example
//!
//! ```no_run
//! use seppo::telemetry::{TelemetryConfig, init_telemetry};
//!
//! #[tokio::main]
//! async fn main() -> Result<(), Box<dyn std::error::Error>> {
//!     let config = TelemetryConfig::from_env();
//!     let _guard = init_telemetry(&config)?;
//!
//!     // Your code here - spans and metrics will be exported
//!
//!     Ok(())
//! }
//! ```

use opentelemetry::trace::TracerProvider;
use opentelemetry::{global, KeyValue};
use opentelemetry_otlp::{MetricExporter, SpanExporter, WithExportConfig};
use opentelemetry_sdk::metrics::SdkMeterProvider;
use opentelemetry_sdk::trace::SdkTracerProvider;
use opentelemetry_sdk::Resource;
use std::time::Duration;
use tracing::info;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;
use tracing_subscriber::EnvFilter;

/// Telemetry configuration
///
/// Follows the same pattern as ahti/tapio for consistency.
#[derive(Debug, Clone)]
pub struct TelemetryConfig {
    /// OTLP endpoint (default: http://localhost:4317)
    pub otlp_endpoint: String,
    /// Service name for OTEL resource
    pub service_name: String,
    /// Service version
    pub version: String,
    /// Cluster identifier (for K8s context)
    pub cluster_id: String,
    /// Namespace (for K8s context)
    pub namespace: String,
    /// Metric export interval
    pub metric_interval: Duration,
    /// Enable telemetry (can be disabled for testing)
    pub enabled: bool,
}

impl Default for TelemetryConfig {
    fn default() -> Self {
        Self {
            otlp_endpoint: "http://localhost:4317".to_string(),
            service_name: "seppo".to_string(),
            version: env!("CARGO_PKG_VERSION").to_string(),
            cluster_id: "default".to_string(),
            namespace: "seppo-system".to_string(),
            metric_interval: Duration::from_secs(10),
            enabled: true,
        }
    }
}

impl TelemetryConfig {
    /// Load configuration from environment variables
    ///
    /// Follows standard OTEL env vars plus seppo-specific ones:
    /// - OTEL_EXPORTER_OTLP_ENDPOINT
    /// - SEPPO_SERVICE_NAME
    /// - SEPPO_VERSION
    /// - SEPPO_CLUSTER_ID
    /// - SEPPO_NAMESPACE
    /// - SEPPO_TELEMETRY_ENABLED
    pub fn from_env() -> Self {
        Self {
            otlp_endpoint: std::env::var("OTEL_EXPORTER_OTLP_ENDPOINT")
                .unwrap_or_else(|_| "http://localhost:4317".to_string()),
            service_name: std::env::var("SEPPO_SERVICE_NAME")
                .unwrap_or_else(|_| "seppo".to_string()),
            version: std::env::var("SEPPO_VERSION")
                .unwrap_or_else(|_| env!("CARGO_PKG_VERSION").to_string()),
            cluster_id: std::env::var("SEPPO_CLUSTER_ID")
                .unwrap_or_else(|_| "default".to_string()),
            namespace: std::env::var("SEPPO_NAMESPACE")
                .unwrap_or_else(|_| "seppo-system".to_string()),
            metric_interval: std::env::var("SEPPO_METRIC_INTERVAL_SECS")
                .ok()
                .and_then(|v| v.parse().ok())
                .map(Duration::from_secs)
                .unwrap_or(Duration::from_secs(10)),
            enabled: std::env::var("SEPPO_TELEMETRY_ENABLED")
                .map(|v| v != "false")
                .unwrap_or(true),
        }
    }
}

/// Guard that shuts down telemetry when dropped
pub struct TelemetryGuard {
    tracer_provider: Option<SdkTracerProvider>,
    meter_provider: Option<SdkMeterProvider>,
}

impl Drop for TelemetryGuard {
    fn drop(&mut self) {
        if let Some(provider) = self.tracer_provider.take() {
            if let Err(e) = provider.shutdown() {
                eprintln!("Failed to shutdown tracer provider: {}", e);
            }
        }
        if let Some(provider) = self.meter_provider.take() {
            if let Err(e) = provider.shutdown() {
                eprintln!("Failed to shutdown meter provider: {}", e);
            }
        }
    }
}

/// Initialize OpenTelemetry with tracing and metrics
///
/// Returns a guard that will shutdown telemetry when dropped.
/// Keep this guard alive for the duration of your application.
pub fn init_telemetry(config: &TelemetryConfig) -> Result<TelemetryGuard, TelemetryError> {
    if !config.enabled {
        // Just set up basic tracing without OTLP export
        tracing_subscriber::registry()
            .with(EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")))
            .with(tracing_subscriber::fmt::layer())
            .init();

        return Ok(TelemetryGuard {
            tracer_provider: None,
            meter_provider: None,
        });
    }

    // Create resource with semantic conventions
    let resource = Resource::builder()
        .with_attributes([
            KeyValue::new("service.name", config.service_name.clone()),
            KeyValue::new("service.version", config.version.clone()),
            KeyValue::new("k8s.cluster.name", config.cluster_id.clone()),
            KeyValue::new("k8s.namespace.name", config.namespace.clone()),
            KeyValue::new("telemetry.sdk.name", "opentelemetry"),
            KeyValue::new("telemetry.sdk.language", "rust"),
        ])
        .build();

    // Setup tracer provider with OTLP exporter
    let tracer_provider = init_tracer_provider(config, resource.clone())?;
    global::set_tracer_provider(tracer_provider.clone());

    // Setup meter provider with OTLP exporter
    let meter_provider = init_meter_provider(config, resource)?;
    global::set_meter_provider(meter_provider.clone());

    // Setup tracing subscriber with OpenTelemetry layer
    let telemetry_layer = tracing_opentelemetry::layer()
        .with_tracer(tracer_provider.tracer("seppo"));

    tracing_subscriber::registry()
        .with(EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")))
        .with(tracing_subscriber::fmt::layer())
        .with(telemetry_layer)
        .init();

    info!(
        endpoint = %config.otlp_endpoint,
        service = %config.service_name,
        "Telemetry initialized"
    );

    Ok(TelemetryGuard {
        tracer_provider: Some(tracer_provider),
        meter_provider: Some(meter_provider),
    })
}

fn init_tracer_provider(
    config: &TelemetryConfig,
    resource: Resource,
) -> Result<SdkTracerProvider, TelemetryError> {
    let exporter = SpanExporter::builder()
        .with_tonic()
        .with_endpoint(&config.otlp_endpoint)
        .build()
        .map_err(|e| TelemetryError::TracerInit(e.to_string()))?;

    let provider = SdkTracerProvider::builder()
        .with_resource(resource)
        .with_batch_exporter(exporter)
        .build();

    Ok(provider)
}

fn init_meter_provider(
    config: &TelemetryConfig,
    resource: Resource,
) -> Result<SdkMeterProvider, TelemetryError> {
    let exporter = MetricExporter::builder()
        .with_tonic()
        .with_endpoint(&config.otlp_endpoint)
        .build()
        .map_err(|e| TelemetryError::MeterInit(e.to_string()))?;

    let provider = SdkMeterProvider::builder()
        .with_periodic_exporter(exporter)
        .with_resource(resource)
        .build();

    Ok(provider)
}

/// Telemetry initialization errors
#[derive(Debug, thiserror::Error)]
pub enum TelemetryError {
    #[error("Failed to initialize tracer: {0}")]
    TracerInit(String),

    #[error("Failed to initialize meter: {0}")]
    MeterInit(String),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_config_default() {
        let config = TelemetryConfig::default();
        assert_eq!(config.service_name, "seppo");
        assert!(config.enabled);
    }

    #[test]
    fn test_config_from_env() {
        // Test with no env vars set - should use defaults
        let config = TelemetryConfig::from_env();
        assert_eq!(config.service_name, "seppo");
    }

    #[test]
    fn test_disabled_telemetry() {
        let config = TelemetryConfig {
            enabled: false,
            ..Default::default()
        };
        assert!(!config.enabled);
    }
}
