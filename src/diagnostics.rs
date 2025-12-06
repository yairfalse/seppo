//! Failure diagnostics for test debugging
//!
//! Collects and formats diagnostic information when tests fail.

use k8s_openapi::api::core::v1::Event;
use std::collections::HashMap;
use std::fmt;

const LINE_WIDTH: usize = 80;
const HEAVY_LINE: &str = "━";
const LIGHT_LINE: &str = "─";

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

    fn heavy_line() -> String {
        HEAVY_LINE.repeat(LINE_WIDTH)
    }

    fn section_header(title: &str) -> String {
        let title_with_spaces = format!(" {} ", title);
        let remaining = LINE_WIDTH.saturating_sub(title_with_spaces.len() + 3);
        format!(
            "{}{}{}",
            LIGHT_LINE.repeat(3),
            title_with_spaces,
            LIGHT_LINE.repeat(remaining)
        )
    }
}

impl fmt::Display for Diagnostics {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        writeln!(f)?;
        writeln!(f, "{}", Self::heavy_line())?;
        writeln!(f, "  SEPPO TEST FAILED")?;
        writeln!(f, "{}", Self::heavy_line())?;
        writeln!(f)?;
        writeln!(f, "  Namespace: {} (kept for debugging)", self.namespace)?;

        // Pod logs
        if !self.pod_logs.is_empty() {
            writeln!(f)?;
            writeln!(f, "{}", Self::section_header("Pod Logs"))?;

            // Sort pod names for consistent output
            let mut pod_names: Vec<_> = self.pod_logs.keys().collect();
            pod_names.sort();

            for pod_name in pod_names {
                let logs = &self.pod_logs[pod_name];
                writeln!(f)?;
                writeln!(f, "[{}]", pod_name)?;

                if logs.is_empty() {
                    writeln!(f, "  (no logs)")?;
                } else {
                    let lines: Vec<&str> = logs.lines().collect();
                    let max_lines = 50;

                    for line in lines.iter().take(max_lines) {
                        writeln!(f, "  {}", line)?;
                    }

                    if lines.len() > max_lines {
                        writeln!(f, "  ... ({} more lines)", lines.len() - max_lines)?;
                    }
                }
            }
        }

        // Events
        if !self.events.is_empty() {
            writeln!(f)?;
            writeln!(
                f,
                "{}",
                Self::section_header(&format!("Events ({})", self.events.len()))
            )?;
            writeln!(f)?;

            // Sort events by timestamp, placing events without timestamps at the end
            let mut events: Vec<_> = self.events.iter().collect();
            use chrono::prelude::*;
            events.sort_by_key(|event| {
                // Use a far-future date for missing timestamps so they sort last
                event.last_timestamp
                    .as_ref()
                    .map(|t| t.0)
                    .unwrap_or(DateTime::<Utc>::MAX)
            });

            for event in events {
                let timestamp = event
                    .last_timestamp
                    .as_ref()
                    .map(|t| t.0.format("%H:%M:%S").to_string())
                    .unwrap_or_else(|| "??:??:??".to_string());

                let kind = event.involved_object.kind.as_deref().unwrap_or("?");
                let name = event.involved_object.name.as_deref().unwrap_or("?");
                let reason = event.reason.as_deref().unwrap_or("Unknown");
                let message = event.message.as_deref().unwrap_or("");

                // Truncate message if too long
                let max_msg_len = 45;
                let msg_display = if message.len() > max_msg_len {
                    format!("{}...", &message[..max_msg_len])
                } else {
                    message.to_string()
                };

                writeln!(
                    f,
                    "  • {}  {:12}  {:10}  {}",
                    timestamp,
                    format!("{}/{}", kind, name),
                    reason,
                    msg_display
                )?;
            }
        }

        // Debug commands
        writeln!(f)?;
        writeln!(f, "{}", Self::section_header("Debug"))?;
        writeln!(f)?;
        writeln!(f, "  kubectl -n {} get all", self.namespace)?;
        writeln!(f, "  kubectl -n {} describe pods", self.namespace)?;
        writeln!(f, "  kubectl -n {} logs <pod>", self.namespace)?;
        writeln!(f, "  kubectl delete ns {}  # cleanup", self.namespace)?;
        writeln!(f)?;
        writeln!(f, "{}", Self::heavy_line())?;

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

        assert!(output.contains("[my-pod]"));
        assert!(output.contains("Error: connection refused"));
    }

    #[test]
    fn test_diagnostics_has_header() {
        let diag = Diagnostics::new("seppo-test-abc".to_string());
        let output = diag.to_string();

        // Should have a clear header
        assert!(output.contains("SEPPO TEST FAILED"));
        // Should have visual separators
        assert!(output.contains("━━━"));
    }

    #[test]
    fn test_diagnostics_has_section_headers() {
        let mut diag = Diagnostics::new("seppo-test-xyz".to_string());
        diag.pod_logs
            .insert("pod-1".to_string(), "log content".to_string());

        let output = diag.to_string();

        // Should have section headers
        assert!(output.contains("Pod Logs"));
        assert!(output.contains("Debug"));
    }

    #[test]
    fn test_diagnostics_events_formatted() {
        use k8s_openapi::api::core::v1::{Event, ObjectReference};
        use k8s_openapi::apimachinery::pkg::apis::meta::v1::Time;
        use k8s_openapi::chrono::{TimeZone, Utc};

        let mut diag = Diagnostics::new("seppo-test-xyz".to_string());

        // Create a test event
        let event = Event {
            reason: Some("BackOff".to_string()),
            message: Some("Back-off pulling image".to_string()),
            involved_object: ObjectReference {
                kind: Some("Pod".to_string()),
                name: Some("my-pod".to_string()),
                ..Default::default()
            },
            last_timestamp: Some(Time(Utc.with_ymd_and_hms(2024, 1, 15, 10, 42, 1).unwrap())),
            ..Default::default()
        };
        diag.events.push(event);

        let output = diag.to_string();

        // Events should show count
        assert!(output.contains("Events (1)"));
        // Events should be formatted with bullet points
        assert!(output.contains("•"));
        // Should include resource type and name
        assert!(output.contains("Pod/my-pod"));
    }

    #[test]
    fn test_diagnostics_cleanup_command() {
        let diag = Diagnostics::new("seppo-test-xyz".to_string());
        let output = diag.to_string();

        // Should include cleanup command
        assert!(output.contains("kubectl delete ns seppo-test-xyz"));
    }
}
