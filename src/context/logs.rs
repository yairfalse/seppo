use super::{improve_error_message, Context, ContextError};
use futures::AsyncBufReadExt;
use futures::Stream;
use k8s_openapi::api::core::v1::Pod;
use kube::api::{Api, ListParams, LogParams};
use std::collections::HashMap;
use tracing::{debug, info};

impl Context {
    /// Get logs from a pod in the test namespace
    pub async fn logs(&self, pod_name: &str) -> Result<String, ContextError> {
        let pods: Api<Pod> = Api::namespaced(self.client.clone(), &self.namespace);

        let logs = pods
            .logs(pod_name, &LogParams::default())
            .await
            .map_err(|e| ContextError::LogsError(improve_error_message(&e, "Pod", pod_name)))?;

        Ok(logs)
    }

    /// Stream logs from a pod in real-time
    ///
    /// Returns a stream of log lines that can be iterated with `try_next().await`.
    /// Uses `follow: true` for continuous streaming like `kubectl logs -f`.
    ///
    /// # Lifetime
    ///
    /// The returned stream holds an internal reference to the Kubernetes API
    /// connection. The stream must be dropped before the `Context` can be dropped.
    /// This is enforced at compile-time via the `'_` lifetime on the return type.
    ///
    /// # Example
    ///
    /// ```ignore
    /// use futures::TryStreamExt;
    ///
    /// let mut stream = ctx.logs_stream("my-pod").await?;
    /// while let Some(line) = stream.try_next().await? {
    ///     println!("{}", line);
    /// }
    /// ```
    pub async fn logs_stream(
        &self,
        pod_name: &str,
    ) -> Result<impl Stream<Item = Result<String, std::io::Error>> + '_, ContextError> {
        let pods: Api<Pod> = Api::namespaced(self.client.clone(), &self.namespace);

        let params = LogParams {
            follow: true,
            ..Default::default()
        };

        debug!(
            namespace = %self.namespace,
            pod = %pod_name,
            "Starting log stream"
        );

        let log_stream = pods
            .log_stream(pod_name, &params)
            .await
            .map_err(|e| ContextError::LogsError(improve_error_message(&e, "Pod", pod_name)))?;

        Ok(log_stream.lines())
    }

    /// Get logs from all pods matching a label selector
    ///
    /// Returns a `HashMap` of pod name to logs. Useful for debugging deployments
    /// with multiple replicas or getting logs from all pods in a service.
    ///
    /// # Arguments
    ///
    /// * `selector` - Label selector string, e.g., "app=myapp" or "app=myapp,tier=backend"
    ///
    /// # Example
    ///
    /// ```ignore
    /// // Get logs from all pods with label app=myapp
    /// let all_logs = ctx.logs_all("app=myapp").await?;
    /// for (pod_name, logs) in &all_logs {
    ///     println!("=== {} ===\n{}", pod_name, logs);
    /// }
    /// ```
    pub async fn logs_all(&self, selector: &str) -> Result<HashMap<String, String>, ContextError> {
        let pods: Api<Pod> = Api::namespaced(self.client.clone(), &self.namespace);

        // List pods matching the selector
        let list_params = ListParams::default().labels(selector);
        let pod_list = pods
            .list(&list_params)
            .await
            .map_err(|e| ContextError::ListError(e.to_string()))?;

        let mut all_logs = HashMap::new();

        for pod in pod_list.items {
            let pod_name = pod.metadata.name.unwrap_or_default();
            if pod_name.is_empty() {
                continue;
            }

            // Try to get logs, but don't fail if a pod doesn't have logs yet
            match pods.logs(&pod_name, &LogParams::default()).await {
                Ok(logs) => {
                    all_logs.insert(pod_name, logs);
                }
                Err(e) => {
                    debug!(
                        pod = %pod_name,
                        error = %e,
                        "Could not get logs from pod (may not be ready yet)"
                    );
                    // Insert empty string to indicate pod exists but no logs
                    all_logs.insert(pod_name, String::new());
                }
            }
        }

        info!(
            namespace = %self.namespace,
            selector = %selector,
            pod_count = %all_logs.len(),
            "Collected logs from pods"
        );

        Ok(all_logs)
    }
}
