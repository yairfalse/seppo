//! Logging configuration for Seppo
//!
//! Provides simple tracing-based logging. No OTEL - this is a test SDK,
//! not a production service.
//!
//! # Example
//!
//! ```no_run
//! use seppo::telemetry::init_logging;
//!
//! init_logging();
//! // Logs will go to stderr with the configured level
//! ```

use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;
use tracing_subscriber::EnvFilter;

/// Initialize logging with tracing-subscriber
///
/// Uses RUST_LOG env var for filtering (default: info).
/// Call once at the start of your test or application.
pub fn init_logging() {
    let _ = tracing_subscriber::registry()
        .with(EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")))
        .with(tracing_subscriber::fmt::layer())
        .try_init();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_init_logging() {
        // Should not panic when called multiple times
        init_logging();
        init_logging();
    }
}
