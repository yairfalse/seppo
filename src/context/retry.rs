use super::Context;
use tracing::{debug, warn};

impl Context {
    /// Retry an operation with exponential backoff
    ///
    /// Useful for operations that may transiently fail, like waiting for
    /// a resource to be fully ready or handling API rate limits.
    ///
    /// Starts with 100ms backoff, doubling each time up to 5 seconds max.
    ///
    /// # Panics
    ///
    /// Panics if `max_attempts` is 0. Use at least 1 attempt.
    ///
    /// # Example
    ///
    /// ```ignore
    /// // Retry an HTTP request up to 5 times
    /// ctx.retry(5, || async {
    ///     let resp = pf.get("/health").await?;
    ///     if resp.contains("ok") { Ok(()) } else { Err("not ready".into()) }
    /// }).await?;
    /// ```
    pub async fn retry<F, Fut, E>(&self, max_attempts: usize, mut f: F) -> Result<(), E>
    where
        F: FnMut() -> Fut,
        Fut: std::future::Future<Output = Result<(), E>>,
        E: std::fmt::Display,
    {
        assert!(
            max_attempts > 0,
            "retry() requires at least 1 attempt, got max_attempts=0"
        );

        let mut backoff = std::time::Duration::from_millis(100);
        let max_backoff = std::time::Duration::from_secs(5);
        let mut last_error: Option<E> = None;

        for attempt in 1..=max_attempts {
            match f().await {
                Ok(()) => {
                    debug!(attempt = attempt, "Retry succeeded");
                    return Ok(());
                }
                Err(e) => {
                    let is_last_attempt = attempt == max_attempts;

                    if is_last_attempt {
                        warn!(
                            attempt = attempt,
                            max_attempts = max_attempts,
                            error = %e,
                            "Retry exhausted all attempts"
                        );
                    } else {
                        debug!(
                            attempt = attempt,
                            max_attempts = max_attempts,
                            backoff = ?backoff,
                            error = %e,
                            "Retry attempt failed, backing off"
                        );
                        tokio::time::sleep(backoff).await;
                        backoff = std::cmp::min(backoff * 2, max_backoff);
                    }

                    last_error = Some(e);
                }
            }
        }

        // This is reached when all attempts fail
        Err(last_error.expect("max_attempts > 0 guarantees at least one iteration"))
    }
}
