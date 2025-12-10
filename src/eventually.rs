//! Async condition helpers for testing
//!
//! Provides `eventually` and `consistently` for checking conditions over time.
//!
//! # Example
//!
//! ```ignore
//! use seppo::{eventually, consistently};
//! use std::time::Duration;
//!
//! // Wait for condition to become true
//! eventually(|| async {
//!     ctx.get::<Pod>("my-pod").await.is_ok()
//! }).await?;
//!
//! // With custom timeout
//! eventually(|| async { check_something().await })
//!     .timeout(Duration::from_secs(60))
//!     .interval(Duration::from_millis(500))
//!     .await_condition()
//!     .await?;
//!
//! // Verify condition stays true
//! consistently(|| async {
//!     service_is_healthy().await
//! }).await?;
//! ```

use std::future::Future;
use std::time::Duration;
use tokio::time::{sleep, Instant};

/// Error type for eventually/consistently operations
#[derive(Debug, thiserror::Error)]
pub enum ConditionError {
    #[error("condition not met within {0:?}")]
    Timeout(Duration),

    #[error("condition failed after {attempts} attempts over {elapsed:?}: {last_error}")]
    EventuallyFailed {
        attempts: u32,
        elapsed: Duration,
        last_error: String,
    },

    #[error("condition became false after {elapsed:?}: {reason}")]
    ConsistentlyFailed { elapsed: Duration, reason: String },
}

/// Builder for eventually checks
pub struct Eventually<F, Fut>
where
    F: Fn() -> Fut,
    Fut: Future<Output = bool>,
{
    condition: F,
    timeout: Duration,
    interval: Duration,
}

/// Builder for consistently checks
pub struct Consistently<F, Fut>
where
    F: Fn() -> Fut,
    Fut: Future<Output = bool>,
{
    condition: F,
    duration: Duration,
    interval: Duration,
}

/// Create an eventually check that retries until condition is true
///
/// Default timeout: 30 seconds
/// Default interval: 250ms
pub fn eventually<F, Fut>(condition: F) -> Eventually<F, Fut>
where
    F: Fn() -> Fut,
    Fut: Future<Output = bool>,
{
    Eventually {
        condition,
        timeout: Duration::from_secs(30),
        interval: Duration::from_millis(250),
    }
}

/// Create a consistently check that verifies condition stays true
///
/// Default duration: 5 seconds
/// Default interval: 250ms
pub fn consistently<F, Fut>(condition: F) -> Consistently<F, Fut>
where
    F: Fn() -> Fut,
    Fut: Future<Output = bool>,
{
    Consistently {
        condition,
        duration: Duration::from_secs(5),
        interval: Duration::from_millis(250),
    }
}

impl<F, Fut> Eventually<F, Fut>
where
    F: Fn() -> Fut,
    Fut: Future<Output = bool>,
{
    /// Set the timeout duration
    pub fn timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }

    /// Set the polling interval
    pub fn interval(mut self, interval: Duration) -> Self {
        self.interval = interval;
        self
    }

    /// Run the check, retrying until success or timeout
    pub async fn await_condition(self) -> Result<(), ConditionError> {
        let start = Instant::now();
        let mut attempts = 0u32;

        loop {
            attempts += 1;

            if (self.condition)().await {
                return Ok(());
            }

            let elapsed = start.elapsed();
            if elapsed >= self.timeout {
                return Err(ConditionError::EventuallyFailed {
                    attempts,
                    elapsed,
                    last_error: "condition returned false".to_string(),
                });
            }

            sleep(self.interval).await;
        }
    }
}

impl<F, Fut> Consistently<F, Fut>
where
    F: Fn() -> Fut,
    Fut: Future<Output = bool>,
{
    /// Set the duration to check for
    pub fn duration(mut self, duration: Duration) -> Self {
        self.duration = duration;
        self
    }

    /// Set the polling interval
    pub fn interval(mut self, interval: Duration) -> Self {
        self.interval = interval;
        self
    }

    /// Run the check, verifying condition stays true
    pub async fn await_condition(self) -> Result<(), ConditionError> {
        let start = Instant::now();

        loop {
            if !(self.condition)().await {
                return Err(ConditionError::ConsistentlyFailed {
                    elapsed: start.elapsed(),
                    reason: "condition returned false".to_string(),
                });
            }

            let elapsed = start.elapsed();
            if elapsed >= self.duration {
                return Ok(());
            }

            sleep(self.interval).await;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU32, Ordering};
    use std::sync::Arc;

    #[tokio::test]
    async fn test_eventually_succeeds_immediately() {
        let result = eventually(|| async { true })
            .timeout(Duration::from_millis(100))
            .await_condition()
            .await;

        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_eventually_succeeds_after_retries() {
        let counter = Arc::new(AtomicU32::new(0));
        let counter_clone = counter.clone();

        let result = eventually(move || {
            let c = counter_clone.clone();
            async move {
                let count = c.fetch_add(1, Ordering::SeqCst);
                count >= 3 // Succeed on 4th attempt
            }
        })
        .timeout(Duration::from_secs(1))
        .interval(Duration::from_millis(10))
        .await_condition()
        .await;

        assert!(result.is_ok());
        assert!(counter.load(Ordering::SeqCst) >= 4);
    }

    #[tokio::test]
    async fn test_eventually_times_out() {
        let result = eventually(|| async { false })
            .timeout(Duration::from_millis(100))
            .interval(Duration::from_millis(10))
            .await_condition()
            .await;

        assert!(result.is_err());
        match result.unwrap_err() {
            ConditionError::EventuallyFailed { attempts, .. } => {
                assert!(attempts > 1);
            }
            _ => panic!("expected EventuallyFailed"),
        }
    }

    #[tokio::test]
    async fn test_consistently_succeeds() {
        let result = consistently(|| async { true })
            .duration(Duration::from_millis(100))
            .interval(Duration::from_millis(20))
            .await_condition()
            .await;

        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_consistently_fails_when_condition_changes() {
        let counter = Arc::new(AtomicU32::new(0));
        let counter_clone = counter.clone();

        let result = consistently(move || {
            let c = counter_clone.clone();
            async move {
                let count = c.fetch_add(1, Ordering::SeqCst);
                count < 3 // Fail on 4th check
            }
        })
        .duration(Duration::from_secs(1))
        .interval(Duration::from_millis(10))
        .await_condition()
        .await;

        assert!(result.is_err());
        match result.unwrap_err() {
            ConditionError::ConsistentlyFailed { .. } => {}
            _ => panic!("expected ConsistentlyFailed"),
        }
    }

    #[tokio::test]
    async fn test_eventually_default_timeout() {
        let ev = eventually(|| async { true });
        assert_eq!(ev.timeout, Duration::from_secs(30));
        assert_eq!(ev.interval, Duration::from_millis(250));
    }

    #[tokio::test]
    async fn test_consistently_default_duration() {
        let cons = consistently(|| async { true });
        assert_eq!(cons.duration, Duration::from_secs(5));
        assert_eq!(cons.interval, Duration::from_millis(250));
    }
}
