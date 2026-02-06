use super::{Context, ContextError};
use crate::diagnostics::Diagnostics;
use k8s_openapi::api::core::v1::{Event, Pod};
use kube::api::{Api, ListParams, LogParams};
use std::collections::HashMap;
use tracing::warn;

impl Context {
    /// Get events from the test namespace
    ///
    /// Returns all events in the namespace, useful for debugging test failures.
    pub async fn events(&self) -> Result<Vec<Event>, ContextError> {
        let events: Api<Event> = Api::namespaced(self.client.clone(), &self.namespace);

        let list = events
            .list(&ListParams::default())
            .await
            .map_err(|e| ContextError::EventsError(e.to_string()))?;

        Ok(list.items)
    }

    /// Collect logs from all pods in the test namespace
    ///
    /// Returns a map of pod name to logs. Pods that don't have logs yet
    /// (e.g., pending pods) will have an empty string or error message.
    pub async fn collect_pod_logs(&self) -> Result<HashMap<String, String>, ContextError> {
        let pods: Api<Pod> = Api::namespaced(self.client.clone(), &self.namespace);

        let pod_list = pods
            .list(&ListParams::default())
            .await
            .map_err(|e| ContextError::ListError(e.to_string()))?;

        let mut logs_map = HashMap::new();

        for pod in pod_list.items {
            let pod_name = pod.metadata.name.unwrap_or_default();
            if pod_name.is_empty() {
                continue;
            }

            // Try to get logs, but don't fail if pod isn't ready
            let logs = match pods.logs(&pod_name, &LogParams::default()).await {
                Ok(logs) => logs,
                Err(e) => format!("[error getting logs: {e}]"),
            };

            logs_map.insert(pod_name, logs);
        }

        Ok(logs_map)
    }

    /// Collect all diagnostic information for debugging
    ///
    /// Gathers pod logs and events from the namespace. This is called
    /// automatically on test failure by the `#[seppo::test]` macro.
    pub async fn collect_diagnostics(&self) -> Diagnostics {
        let mut diag = Diagnostics::new(self.namespace.clone());

        // Collect pod logs (best effort)
        match self.collect_pod_logs().await {
            Ok(logs) => diag.pod_logs = logs,
            Err(e) => {
                warn!(error = %e, "Failed to collect pod logs for diagnostics");
            }
        }

        // Collect events (best effort)
        match self.events().await {
            Ok(events) => diag.events = events,
            Err(e) => {
                warn!(error = %e, "Failed to collect events for diagnostics");
            }
        }

        diag
    }
}
