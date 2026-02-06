use crate::portforward::PortForward;

/// Configuration for traffic generation
///
/// # Example
///
/// ```ignore
/// let config = TrafficConfig::default()
///     .with_rps(100)
///     .with_duration(Duration::from_secs(30))
///     .with_endpoint("/health");
/// ```
#[derive(Debug, Clone)]
pub struct TrafficConfig {
    /// Requests per second
    pub rps: u32,
    /// How long to generate traffic
    pub duration: std::time::Duration,
    /// HTTP endpoint to hit
    pub endpoint: String,
}

impl Default for TrafficConfig {
    fn default() -> Self {
        Self {
            rps: 10,
            duration: std::time::Duration::from_secs(10),
            endpoint: "/".to_string(),
        }
    }
}

impl TrafficConfig {
    /// Set requests per second
    #[must_use]
    pub fn with_rps(mut self, rps: u32) -> Self {
        self.rps = rps;
        self
    }

    /// Set duration
    #[must_use]
    pub fn with_duration(mut self, duration: std::time::Duration) -> Self {
        self.duration = duration;
        self
    }

    /// Set endpoint path
    #[must_use]
    pub fn with_endpoint(mut self, endpoint: impl Into<String>) -> Self {
        self.endpoint = endpoint.into();
        self
    }
}

/// Statistics from a traffic test
#[derive(Debug, Clone)]
pub struct TrafficStats {
    /// Total requests made
    pub total_requests: u64,
    /// Number of failed requests
    pub errors: u64,
    /// Individual request latencies
    pub latencies: Vec<std::time::Duration>,
}

impl TrafficStats {
    /// Calculate error rate (0.0 to 1.0)
    #[must_use]
    #[allow(clippy::cast_precision_loss)] // Precision loss acceptable for statistics
    pub fn error_rate(&self) -> f64 {
        if self.total_requests == 0 {
            0.0
        } else {
            self.errors as f64 / self.total_requests as f64
        }
    }

    /// Calculate 99th percentile latency
    #[must_use]
    #[allow(
        clippy::cast_precision_loss,
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss
    )]
    pub fn p99_latency(&self) -> std::time::Duration {
        if self.latencies.is_empty() {
            return std::time::Duration::ZERO;
        }

        let mut sorted = self.latencies.clone();
        sorted.sort();

        let idx = ((sorted.len() as f64) * 0.99) as usize;
        let idx = idx.min(sorted.len() - 1);
        sorted[idx]
    }

    /// Calculate median latency
    #[must_use]
    pub fn median_latency(&self) -> std::time::Duration {
        if self.latencies.is_empty() {
            return std::time::Duration::ZERO;
        }

        let mut sorted = self.latencies.clone();
        sorted.sort();
        sorted[sorted.len() / 2]
    }
}

/// Handle to running traffic generation
///
/// Use `wait()` to block until traffic generation completes,
/// or `stop()` to stop early. Use `stats()` to get results.
pub struct TrafficHandle {
    pub(crate) _pf: PortForward,
    pub(crate) handle: tokio::task::JoinHandle<()>,
    pub(crate) stop_flag: std::sync::Arc<std::sync::atomic::AtomicBool>,
    pub(crate) total: std::sync::Arc<std::sync::atomic::AtomicU64>,
    pub(crate) errors: std::sync::Arc<std::sync::atomic::AtomicU64>,
    pub(crate) latencies: std::sync::Arc<std::sync::Mutex<Vec<std::time::Duration>>>,
}

impl TrafficHandle {
    /// Wait for traffic generation to complete
    pub async fn wait(self) {
        let _ = self.handle.await;
    }

    /// Stop traffic generation early
    pub fn stop(&self) {
        self.stop_flag
            .store(true, std::sync::atomic::Ordering::Relaxed);
    }

    /// Get current statistics
    ///
    /// # Panics
    ///
    /// Panics if the latencies mutex is poisoned.
    #[must_use]
    pub fn stats(&self) -> TrafficStats {
        use std::sync::atomic::Ordering;

        TrafficStats {
            total_requests: self.total.load(Ordering::Relaxed),
            errors: self.errors.load(Ordering::Relaxed),
            latencies: self.latencies.lock().unwrap().clone(),
        }
    }

    /// Get total requests made so far
    #[must_use]
    pub fn total_requests(&self) -> u64 {
        self.total.load(std::sync::atomic::Ordering::Relaxed)
    }

    /// Get error rate so far
    #[must_use]
    pub fn error_rate(&self) -> f64 {
        self.stats().error_rate()
    }
}
