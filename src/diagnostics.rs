//! Failure diagnostics for test debugging
//!
//! Collects and formats diagnostic information when tests fail.

use k8s_openapi::api::core::v1::Event;
use std::collections::HashMap;
use std::fmt;

/// Collected diagnostic information from a failed test
#[derive(Debug, Default)]
pub struct Diagnostics {
    /// Test namespace
    pub namespace: String,
    /// Pod name -> logs
    pub pod_logs: HashMap<String, String>,
    /// Namespace events
    pub events: Vec<Event>,
}

impl Diagnostics {
    /// Create new empty diagnostics for a namespace
    pub fn new(namespace: String) -> Self {
        Self {
            namespace,
            pod_logs: HashMap::new(),
            events: Vec::new(),
        }
    }
}

impl fmt::Display for Diagnostics {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        writeln!(f, "\n[seppo] Test failed - namespace kept for debugging")?;
        writeln!(f, "[seppo] Namespace: {}", self.namespace)?;

        // Pod logs
        if !self.pod_logs.is_empty() {
            writeln!(f)?;
            for (pod_name, logs) in &self.pod_logs {
                writeln!(f, "--- Pod logs ({}) ---", pod_name)?;
                if logs.is_empty() {
                    writeln!(f, "(no logs)")?;
                } else {
                    // Limit log output to avoid overwhelming output
                    let lines: Vec<&str> = logs.lines().collect();
                    let max_lines = 50;
                    if lines.len() > max_lines {
                        for line in lines.iter().take(max_lines) {
                            writeln!(f, "{}", line)?;
                        }
                        writeln!(f, "... ({} more lines)", lines.len() - max_lines)?;
                    } else {
                        for line in &lines {
                            writeln!(f, "{}", line)?;
                        }
                    }
                }
                writeln!(f)?;
            }
        }

        // Events
        if !self.events.is_empty() {
            writeln!(f, "--- Events ---")?;
            for event in &self.events {
                let reason = event.reason.as_deref().unwrap_or("Unknown");
                let message = event.message.as_deref().unwrap_or("");
                let involved = event.involved_object.name.as_deref().unwrap_or("unknown");
                let kind = event.involved_object.kind.as_deref().unwrap_or("Unknown");
                writeln!(f, "{}/{}: {} - {}", kind, involved, reason, message)?;
            }
            writeln!(f)?;
        }

        // Debug commands
        writeln!(f, "[seppo] Debug commands:")?;
        writeln!(f, "  kubectl -n {} get all", self.namespace)?;
        writeln!(f, "  kubectl -n {} describe pods", self.namespace)?;
        writeln!(f, "  kubectl -n {} logs <pod>", self.namespace)?;

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_diagnostics_display_empty() {
        let diag = Diagnostics::new("seppo-test-abc123".to_string());
        let output = diag.to_string();

        assert!(output.contains("seppo-test-abc123"));
        assert!(output.contains("kubectl -n seppo-test-abc123 get all"));
    }

    #[test]
    fn test_diagnostics_display_with_logs() {
        let mut diag = Diagnostics::new("seppo-test-xyz".to_string());
        diag.pod_logs.insert(
            "my-pod".to_string(),
            "Error: connection refused".to_string(),
        );

        let output = diag.to_string();

        assert!(output.contains("--- Pod logs (my-pod) ---"));
        assert!(output.contains("Error: connection refused"));
    }
}
