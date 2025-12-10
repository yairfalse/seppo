//! Traffic testing helpers for HTTP validation
//!
//! Provides utilities for testing HTTP traffic patterns and responses.
//!
//! # Example
//!
//! ```ignore
//! use seppo::traffic::{HttpAssert, TrafficTest};
//!
//! // Assert on a single response
//! let response = pf.get("/health").await?;
//! HttpAssert::new(&response)
//!     .status_ok()
//!     .contains_json("status", "healthy")
//!     .assert();
//!
//! // Run a traffic test
//! TrafficTest::new(pf)
//!     .get("/api/users")
//!     .expect_status(200)
//!     .expect_json_path("$.users")
//!     .run()
//!     .await?;
//! ```

use std::time::Duration;

/// Error type for traffic test operations
#[derive(Debug, thiserror::Error)]
pub enum TrafficError {
    #[error("request failed: {0}")]
    RequestFailed(String),

    #[error("assertion failed: {0}")]
    AssertionFailed(String),

    #[error("timeout after {0:?}")]
    Timeout(Duration),
}

/// HTTP response assertion builder
pub struct HttpAssert<'a> {
    response: &'a str,
    errors: Vec<String>,
}

impl<'a> HttpAssert<'a> {
    /// Create a new HTTP assertion builder
    pub fn new(response: &'a str) -> Self {
        Self {
            response,
            errors: Vec::new(),
        }
    }

    /// Assert the response contains expected text
    pub fn contains(mut self, expected: &str) -> Self {
        if !self.response.contains(expected) {
            self.errors.push(format!(
                "expected response to contain '{}', got: {}",
                expected,
                truncate(self.response, 100)
            ));
        }
        self
    }

    /// Assert the response does not contain text
    pub fn not_contains(mut self, unexpected: &str) -> Self {
        if self.response.contains(unexpected) {
            self.errors.push(format!(
                "expected response to NOT contain '{}', but it did",
                unexpected
            ));
        }
        self
    }

    /// Assert the response contains a JSON key-value pair
    pub fn contains_json(mut self, key: &str, value: &str) -> Self {
        // Simple JSON check - looks for "key": "value" or "key":"value"
        let patterns = [
            format!("\"{}\":\"{}\"", key, value),
            format!("\"{}\": \"{}\"", key, value),
            format!("\"{}\":{}", key, value),
            format!("\"{}\": {}", key, value),
        ];

        let found = patterns.iter().any(|p| self.response.contains(p));
        if !found {
            self.errors.push(format!(
                "expected JSON to contain {}={}, got: {}",
                key,
                value,
                truncate(self.response, 100)
            ));
        }
        self
    }

    /// Assert the response is valid JSON
    pub fn is_json(mut self) -> Self {
        // Simple check - starts with { or [
        let trimmed = self.response.trim();
        if !trimmed.starts_with('{') && !trimmed.starts_with('[') {
            self.errors.push(format!(
                "expected valid JSON, got: {}",
                truncate(self.response, 50)
            ));
        }
        self
    }

    /// Assert the response matches expected exactly
    pub fn equals(mut self, expected: &str) -> Self {
        if self.response != expected {
            self.errors.push(format!(
                "expected '{}', got '{}'",
                truncate(expected, 50),
                truncate(self.response, 50)
            ));
        }
        self
    }

    /// Assert the response is empty
    pub fn is_empty(mut self) -> Self {
        if !self.response.is_empty() {
            self.errors.push(format!(
                "expected empty response, got: {}",
                truncate(self.response, 50)
            ));
        }
        self
    }

    /// Assert the response is not empty
    pub fn is_not_empty(mut self) -> Self {
        if self.response.is_empty() {
            self.errors
                .push("expected non-empty response, got empty".to_string());
        }
        self
    }

    /// Check all assertions and return result
    pub fn result(self) -> Result<(), TrafficError> {
        if self.errors.is_empty() {
            Ok(())
        } else {
            Err(TrafficError::AssertionFailed(self.errors.join("; ")))
        }
    }

    /// Check all assertions, panicking on failure
    pub fn assert(self) {
        if !self.errors.is_empty() {
            panic!("HTTP assertion failed: {}", self.errors.join("; "));
        }
    }
}

/// A recorded HTTP request for traffic analysis
#[derive(Debug, Clone)]
pub struct RequestRecord {
    /// HTTP method
    pub method: String,
    /// Request path
    pub path: String,
    /// Response body
    pub response: String,
    /// Request duration
    pub duration: Duration,
}

/// Traffic recorder for collecting HTTP request data
#[derive(Debug, Default)]
pub struct TrafficRecorder {
    requests: Vec<RequestRecord>,
}

impl TrafficRecorder {
    /// Create a new traffic recorder
    pub fn new() -> Self {
        Self::default()
    }

    /// Record a request
    pub fn record(&mut self, method: &str, path: &str, response: &str, duration: Duration) {
        self.requests.push(RequestRecord {
            method: method.to_string(),
            path: path.to_string(),
            response: response.to_string(),
            duration,
        });
    }

    /// Get all recorded requests
    pub fn requests(&self) -> &[RequestRecord] {
        &self.requests
    }

    /// Get count of requests
    pub fn count(&self) -> usize {
        self.requests.len()
    }

    /// Get requests by path
    pub fn by_path(&self, path: &str) -> Vec<&RequestRecord> {
        self.requests.iter().filter(|r| r.path == path).collect()
    }

    /// Get requests by method
    pub fn by_method(&self, method: &str) -> Vec<&RequestRecord> {
        self.requests
            .iter()
            .filter(|r| r.method == method)
            .collect()
    }

    /// Get average response time
    pub fn avg_duration(&self) -> Option<Duration> {
        if self.requests.is_empty() {
            None
        } else {
            let total: Duration = self.requests.iter().map(|r| r.duration).sum();
            Some(total / self.requests.len() as u32)
        }
    }

    /// Get max response time
    pub fn max_duration(&self) -> Option<Duration> {
        self.requests.iter().map(|r| r.duration).max()
    }

    /// Get min response time
    pub fn min_duration(&self) -> Option<Duration> {
        self.requests.iter().map(|r| r.duration).min()
    }

    /// Clear all recorded requests
    pub fn clear(&mut self) {
        self.requests.clear();
    }
}

/// Helper to truncate long strings for error messages
fn truncate(s: &str, max_len: usize) -> String {
    if s.len() > max_len {
        format!("{}...", &s[..max_len])
    } else {
        s.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_http_assert_contains() {
        let result = HttpAssert::new(r#"{"status": "ok"}"#)
            .contains("status")
            .result();
        assert!(result.is_ok());
    }

    #[test]
    fn test_http_assert_contains_fails() {
        let result = HttpAssert::new(r#"{"status": "ok"}"#)
            .contains("error")
            .result();
        assert!(result.is_err());
    }

    #[test]
    fn test_http_assert_not_contains() {
        let result = HttpAssert::new(r#"{"status": "ok"}"#)
            .not_contains("error")
            .result();
        assert!(result.is_ok());
    }

    #[test]
    fn test_http_assert_contains_json() {
        let result = HttpAssert::new(r#"{"status": "healthy"}"#)
            .contains_json("status", "healthy")
            .result();
        assert!(result.is_ok());
    }

    #[test]
    fn test_http_assert_contains_json_no_space() {
        let result = HttpAssert::new(r#"{"status":"healthy"}"#)
            .contains_json("status", "healthy")
            .result();
        assert!(result.is_ok());
    }

    #[test]
    fn test_http_assert_is_json() {
        let result = HttpAssert::new(r#"{"key": "value"}"#).is_json().result();
        assert!(result.is_ok());

        let result = HttpAssert::new(r#"[1, 2, 3]"#).is_json().result();
        assert!(result.is_ok());

        let result = HttpAssert::new("not json").is_json().result();
        assert!(result.is_err());
    }

    #[test]
    fn test_http_assert_equals() {
        let result = HttpAssert::new("exact match")
            .equals("exact match")
            .result();
        assert!(result.is_ok());

        let result = HttpAssert::new("actual").equals("expected").result();
        assert!(result.is_err());
    }

    #[test]
    fn test_http_assert_empty() {
        let result = HttpAssert::new("").is_empty().result();
        assert!(result.is_ok());

        let result = HttpAssert::new("not empty").is_empty().result();
        assert!(result.is_err());
    }

    #[test]
    fn test_http_assert_not_empty() {
        let result = HttpAssert::new("content").is_not_empty().result();
        assert!(result.is_ok());

        let result = HttpAssert::new("").is_not_empty().result();
        assert!(result.is_err());
    }

    #[test]
    fn test_http_assert_chained() {
        let result = HttpAssert::new(r#"{"status": "ok", "count": 42}"#)
            .is_json()
            .contains("status")
            .contains_json("status", "ok")
            .not_contains("error")
            .result();
        assert!(result.is_ok());
    }

    #[test]
    fn test_traffic_recorder() {
        let mut recorder = TrafficRecorder::new();

        recorder.record("GET", "/health", "ok", Duration::from_millis(10));
        recorder.record("GET", "/api/users", "[]", Duration::from_millis(50));
        recorder.record("POST", "/api/users", "{}", Duration::from_millis(30));

        assert_eq!(recorder.count(), 3);
        assert_eq!(recorder.by_method("GET").len(), 2);
        assert_eq!(recorder.by_path("/health").len(), 1);
    }

    #[test]
    fn test_traffic_recorder_durations() {
        let mut recorder = TrafficRecorder::new();

        recorder.record("GET", "/a", "", Duration::from_millis(10));
        recorder.record("GET", "/b", "", Duration::from_millis(20));
        recorder.record("GET", "/c", "", Duration::from_millis(30));

        assert_eq!(recorder.avg_duration(), Some(Duration::from_millis(20)));
        assert_eq!(recorder.max_duration(), Some(Duration::from_millis(30)));
        assert_eq!(recorder.min_duration(), Some(Duration::from_millis(10)));
    }

    #[test]
    fn test_traffic_error_display() {
        assert_eq!(
            TrafficError::RequestFailed("test".to_string()).to_string(),
            "request failed: test"
        );
        assert_eq!(
            TrafficError::AssertionFailed("test".to_string()).to_string(),
            "assertion failed: test"
        );
        assert_eq!(
            TrafficError::Timeout(Duration::from_secs(30)).to_string(),
            "timeout after 30s"
        );
    }

    #[test]
    fn test_truncate() {
        assert_eq!(truncate("short", 10), "short");
        assert_eq!(truncate("this is a long string", 10), "this is a ...");
    }
}
