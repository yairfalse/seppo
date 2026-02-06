use super::parsing::parse_resource_ref;
use super::types::ResourceKind;
use super::{Context, ContextError};
use k8s_openapi::api::apps::v1::{DaemonSet, Deployment, StatefulSet};
use k8s_openapi::api::core::v1::{Pod, Service};
use kube::api::{Api, ListParams};

impl Context {
    /// Print combined diagnostic information for a resource
    ///
    /// Prints to stdout: resource status, related pods, events, and logs.
    /// Useful for debugging during test development.
    ///
    /// Supports kubectl-style references: `deployment/name`, `pod/name`, `svc/name`.
    ///
    /// # Example
    ///
    /// ```ignore
    /// // Print diagnostics for a deployment
    /// ctx.debug("deployment/api").await?;
    ///
    /// // Output:
    /// // === deployment/api ===
    /// // Status: 3/3 ready
    /// //
    /// // === Pods ===
    /// // api-xyz-123: Running (0 restarts)
    /// //
    /// // === Events ===
    /// // 10:42:01 Scheduled: Assigned to node-1
    /// //
    /// // === Logs (last 20 lines) ===
    /// // [api-xyz-123] Server started on :8080
    /// ```
    pub async fn debug(&self, target: &str) -> Result<(), ContextError> {
        let (kind, name) = parse_resource_ref(target)?;

        let kind_name = match &kind {
            ResourceKind::Deployment => "deployment",
            ResourceKind::Pod => "pod",
            ResourceKind::Service => "service",
            ResourceKind::DaemonSet => "daemonset",
            ResourceKind::StatefulSet => "statefulset",
        };

        println!();
        println!("=== {kind_name}/{name} ===");

        match kind {
            ResourceKind::Deployment => self.debug_deployment(name).await?,
            ResourceKind::Pod => self.debug_pod(name).await?,
            ResourceKind::Service => self.debug_service(name).await?,
            ResourceKind::DaemonSet => self.debug_daemonset(name).await?,
            ResourceKind::StatefulSet => self.debug_statefulset(name).await?,
        }

        Ok(())
    }

    async fn debug_deployment(&self, name: &str) -> Result<(), ContextError> {
        let deploys: Api<Deployment> = Api::namespaced(self.client.clone(), &self.namespace);

        match deploys.get(name).await {
            Ok(d) => {
                let status = d.status.as_ref();
                let ready = status.and_then(|s| s.ready_replicas).unwrap_or(0);
                let desired = d.spec.as_ref().and_then(|s| s.replicas).unwrap_or(0);
                println!("Status: {ready}/{desired} ready");
            }
            Err(e) => {
                println!("Error fetching deployment: {e}");
            }
        }

        // Get pods with matching labels
        self.debug_pods_for_app(name).await?;
        self.debug_events_for_resource("Deployment", name).await?;
        self.debug_logs_for_app(name).await?;

        Ok(())
    }

    async fn debug_pod(&self, name: &str) -> Result<(), ContextError> {
        let pods: Api<Pod> = Api::namespaced(self.client.clone(), &self.namespace);

        match pods.get(name).await {
            Ok(p) => {
                let phase = p
                    .status
                    .as_ref()
                    .and_then(|s| s.phase.as_ref())
                    .map_or("Unknown", std::string::String::as_str);
                let restarts: i32 = p
                    .status
                    .as_ref()
                    .and_then(|s| s.container_statuses.as_ref())
                    .map_or(0, |cs| cs.iter().map(|c| c.restart_count).sum());
                println!("Phase: {phase} ({restarts} restarts)");

                // Print container statuses
                if let Some(statuses) = p
                    .status
                    .as_ref()
                    .and_then(|s| s.container_statuses.as_ref())
                {
                    println!();
                    println!("=== Containers ===");
                    for cs in statuses {
                        let ready = if cs.ready { "ready" } else { "not ready" };
                        println!("  {}: {} ({})", cs.name, ready, cs.restart_count);
                    }
                }
            }
            Err(e) => {
                println!("Error fetching pod: {e}");
            }
        }

        self.debug_events_for_resource("Pod", name).await?;

        // Print logs
        match self.logs(name).await {
            Ok(logs) => {
                println!();
                println!("=== Logs (last 20 lines) ===");
                for line in logs
                    .lines()
                    .rev()
                    .take(20)
                    .collect::<Vec<_>>()
                    .into_iter()
                    .rev()
                {
                    println!("  {line}");
                }
            }
            Err(e) => {
                println!("Error fetching logs: {e}");
            }
        }

        Ok(())
    }

    async fn debug_service(&self, name: &str) -> Result<(), ContextError> {
        let svcs: Api<Service> = Api::namespaced(self.client.clone(), &self.namespace);

        match svcs.get(name).await {
            Ok(s) => {
                let svc_type = s
                    .spec
                    .as_ref()
                    .and_then(|s| s.type_.as_ref())
                    .map_or("ClusterIP", std::string::String::as_str);
                let ports: Vec<String> = s
                    .spec
                    .as_ref()
                    .and_then(|s| s.ports.as_ref())
                    .map(|ps| ps.iter().map(|p| format!("{}", p.port)).collect())
                    .unwrap_or_default();
                println!("Type: {svc_type}");
                println!("Ports: {}", ports.join(", "));
            }
            Err(e) => {
                println!("Error fetching service: {e}");
            }
        }

        self.debug_events_for_resource("Service", name).await?;

        Ok(())
    }

    async fn debug_daemonset(&self, name: &str) -> Result<(), ContextError> {
        let ds: Api<DaemonSet> = Api::namespaced(self.client.clone(), &self.namespace);

        match ds.get(name).await {
            Ok(d) => {
                let status = d.status.as_ref();
                let ready = status.map_or(0, |s| s.number_ready);
                let desired = status.map_or(0, |s| s.desired_number_scheduled);
                println!("Status: {ready}/{desired} ready");
            }
            Err(e) => {
                println!("Error fetching daemonset: {e}");
            }
        }

        self.debug_pods_for_app(name).await?;
        self.debug_events_for_resource("DaemonSet", name).await?;

        Ok(())
    }

    async fn debug_statefulset(&self, name: &str) -> Result<(), ContextError> {
        let sts: Api<StatefulSet> = Api::namespaced(self.client.clone(), &self.namespace);

        match sts.get(name).await {
            Ok(s) => {
                let status = s.status.as_ref();
                let ready = status.and_then(|s| s.ready_replicas).unwrap_or(0);
                let desired = s.spec.as_ref().and_then(|s| s.replicas).unwrap_or(0);
                println!("Status: {ready}/{desired} ready");
            }
            Err(e) => {
                println!("Error fetching statefulset: {e}");
            }
        }

        self.debug_pods_for_app(name).await?;
        self.debug_events_for_resource("StatefulSet", name).await?;

        Ok(())
    }

    async fn debug_pods_for_app(&self, app_name: &str) -> Result<(), ContextError> {
        let pods: Api<Pod> = Api::namespaced(self.client.clone(), &self.namespace);
        let lp = ListParams::default().labels(&format!("app={app_name}"));

        match pods.list(&lp).await {
            Ok(pod_list) => {
                if !pod_list.items.is_empty() {
                    println!();
                    println!("=== Pods ===");
                    for p in &pod_list.items {
                        let name = p.metadata.name.as_deref().unwrap_or("unknown");
                        let phase = p
                            .status
                            .as_ref()
                            .and_then(|s| s.phase.as_ref())
                            .map_or("Unknown", std::string::String::as_str);
                        let restarts: i32 = p
                            .status
                            .as_ref()
                            .and_then(|s| s.container_statuses.as_ref())
                            .map_or(0, |cs| cs.iter().map(|c| c.restart_count).sum());
                        println!("  {name}: {phase} ({restarts} restarts)");
                    }
                }
            }
            Err(e) => {
                println!("Error listing pods: {e}");
            }
        }

        Ok(())
    }

    async fn debug_events_for_resource(&self, kind: &str, name: &str) -> Result<(), ContextError> {
        match self.events().await {
            Ok(events) => {
                let filtered: Vec<_> = events
                    .iter()
                    .filter(|e| {
                        e.involved_object.kind.as_deref() == Some(kind)
                            && e.involved_object.name.as_deref() == Some(name)
                    })
                    .take(10)
                    .collect();

                if !filtered.is_empty() {
                    println!();
                    println!("=== Events ===");
                    for e in filtered {
                        let reason = e.reason.as_deref().unwrap_or("?");
                        let msg = e.message.as_deref().unwrap_or("");
                        // Truncate long messages
                        let msg = if msg.len() > 60 {
                            format!("{}...", &msg[..57])
                        } else {
                            msg.to_string()
                        };
                        println!("  {reason}: {msg}");
                    }
                }
            }
            Err(e) => {
                println!("Error fetching events: {e}");
            }
        }

        Ok(())
    }

    async fn debug_logs_for_app(&self, app_name: &str) -> Result<(), ContextError> {
        let pods: Api<Pod> = Api::namespaced(self.client.clone(), &self.namespace);
        let lp = ListParams::default().labels(&format!("app={app_name}"));

        if let Ok(pod_list) = pods.list(&lp).await {
            for p in pod_list.items.iter().take(2) {
                // Limit to first 2 pods
                let name = p.metadata.name.as_deref().unwrap_or("unknown");
                if let Ok(logs) = self.logs(name).await {
                    println!();
                    println!("=== Logs [{name}] (last 20 lines) ===");
                    for line in logs
                        .lines()
                        .rev()
                        .take(20)
                        .collect::<Vec<_>>()
                        .into_iter()
                        .rev()
                    {
                        println!("  {line}");
                    }
                } else {
                    // Skip pods without logs
                }
            }
        } else {
            // Silently skip if we can't list pods
        }

        Ok(())
    }
}
