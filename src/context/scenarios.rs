use super::parsing::parse_resource_ref;
use super::traffic_types::{TrafficConfig, TrafficHandle};
use super::types::ResourceKind;
use super::{Context, ContextError};
use crate::portforward::PortForwardError;
use k8s_openapi::api::core::v1::Pod;
use kube::api::{Api, ListParams};
use tracing::info;

impl Context {
    /// Test self-healing behavior of a deployment
    ///
    /// Kills a pod and verifies the deployment recovers within the timeout.
    ///
    /// # Example
    ///
    /// ```ignore
    /// ctx.test_self_healing("deployment/myapp", Duration::from_secs(60)).await?;
    /// ```
    pub async fn test_self_healing(
        &self,
        resource: &str,
        recovery_timeout: std::time::Duration,
    ) -> Result<(), ContextError> {
        // Parse deployment/name format
        let (kind, name) = parse_resource_ref(resource)?;
        if kind != ResourceKind::Deployment {
            return Err(ContextError::InvalidResourceRef(
                "test_self_healing requires deployment/name format".to_string(),
            ));
        }

        // Find a pod from this deployment
        let pods: Api<Pod> = Api::namespaced(self.client.clone(), &self.namespace);
        let pod_list = pods
            .list(&ListParams::default().labels(&format!("app={name}")))
            .await
            .map_err(|e| ContextError::ListError(format!("failed to list pods: {e}")))?;

        if pod_list.items.is_empty() {
            return Err(ContextError::ListError(format!(
                "no pods found for deployment {name}"
            )));
        }

        // Kill the first pod
        let pod_name = pod_list.items[0]
            .metadata
            .name
            .as_ref()
            .ok_or_else(|| ContextError::ListError("pod has no name".to_string()))?;

        info!(
            namespace = %self.namespace,
            deployment = %name,
            pod = %pod_name,
            "Killing pod for self-healing test"
        );

        self.kill(&format!("pod/{pod_name}")).await?;

        // Wait for recovery
        self.wait_ready_with_timeout(resource, recovery_timeout)
            .await
    }

    /// Test scaling behavior of a deployment
    ///
    /// Scales to the target replica count and waits for stability.
    ///
    /// # Example
    ///
    /// ```ignore
    /// ctx.test_scaling("deployment/myapp", 5, Duration::from_secs(60)).await?;
    /// ```
    pub async fn test_scaling(
        &self,
        resource: &str,
        target_replicas: i32,
        timeout: std::time::Duration,
    ) -> Result<(), ContextError> {
        // Parse deployment/name format
        let (kind, name) = parse_resource_ref(resource)?;
        if kind != ResourceKind::Deployment {
            return Err(ContextError::InvalidResourceRef(
                "test_scaling requires deployment/name format".to_string(),
            ));
        }

        info!(
            namespace = %self.namespace,
            deployment = %name,
            target_replicas = target_replicas,
            "Scaling deployment for scaling test"
        );

        // Scale the deployment
        self.scale(resource, target_replicas).await?;

        // Wait for stability
        self.wait_ready_with_timeout(resource, timeout).await
    }

    /// Start generating HTTP traffic to a service
    ///
    /// Returns a handle that can be used to wait for completion and get statistics.
    ///
    /// # Example
    ///
    /// ```ignore
    /// let config = TrafficConfig::default()
    ///     .with_rps(100)
    ///     .with_duration(Duration::from_secs(30));
    ///
    /// let traffic = ctx.start_traffic("svc/myapp", 8080, config).await?;
    /// traffic.wait().await;
    /// let stats = traffic.stats();
    /// println!("Error rate: {:.2}%", stats.error_rate() * 100.0);
    /// ```
    pub async fn start_traffic(
        &self,
        resource: &str,
        port: u16,
        config: TrafficConfig,
    ) -> Result<TrafficHandle, PortForwardError> {
        use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
        use std::sync::Arc;

        // Set up port forward
        let pf = self.port_forward(resource, port).await?;
        let local_port = pf.local_addr().port();

        // Create shared state
        let stop_flag = Arc::new(AtomicBool::new(false));
        let total = Arc::new(AtomicU64::new(0));
        let errors = Arc::new(AtomicU64::new(0));
        let latencies = Arc::new(std::sync::Mutex::new(Vec::new()));

        // Clone for the spawned task
        let stop_flag_clone = stop_flag.clone();
        let total_clone = total.clone();
        let errors_clone = errors.clone();
        let latencies_clone = latencies.clone();
        let endpoint = config.endpoint.clone();
        let rps = config.rps;
        let duration = config.duration;

        // Start traffic generation in background
        let handle = tokio::spawn(async move {
            let client = reqwest::Client::builder()
                .timeout(std::time::Duration::from_secs(5))
                .build()
                .unwrap();

            let interval = std::time::Duration::from_secs_f64(1.0 / f64::from(rps));
            let deadline = std::time::Instant::now() + duration;

            while std::time::Instant::now() < deadline && !stop_flag_clone.load(Ordering::Relaxed) {
                let start = std::time::Instant::now();
                let url = format!("http://127.0.0.1:{local_port}{endpoint}");

                let result = client.get(&url).send().await;
                let latency = start.elapsed();

                total_clone.fetch_add(1, Ordering::Relaxed);
                if let Ok(ref resp) = result {
                    if !resp.status().is_success() {
                        errors_clone.fetch_add(1, Ordering::Relaxed);
                    }
                } else {
                    errors_clone.fetch_add(1, Ordering::Relaxed);
                }

                latencies_clone.lock().unwrap().push(latency);

                tokio::time::sleep(interval).await;
            }
        });

        Ok(TrafficHandle {
            _pf: pf,
            handle,
            stop_flag,
            total,
            errors,
            latencies,
        })
    }
}
