use super::ingress::IngressTest;
use super::Context;

impl Context {
    /// Create a pod assertion builder
    ///
    /// # Example
    ///
    /// ```ignore
    /// ctx.assert_pod("my-pod").is_running().await?;
    /// ctx.assert_pod("my-pod").has_label("app", "myapp").await?;
    /// ```
    #[must_use]
    pub fn assert_pod(&self, name: &str) -> crate::assertions::PodAssertion {
        crate::assertions::PodAssertion::new(
            self.client.clone(),
            self.namespace.clone(),
            name.to_string(),
        )
    }

    /// Create a deployment assertion builder
    ///
    /// # Example
    ///
    /// ```ignore
    /// ctx.assert_deployment("my-app").has_replicas(3).await?;
    /// ctx.assert_deployment("my-app").is_available().await?;
    /// ```
    #[must_use]
    pub fn assert_deployment(&self, name: &str) -> crate::assertions::DeploymentAssertion {
        crate::assertions::DeploymentAssertion::new(
            self.client.clone(),
            self.namespace.clone(),
            name.to_string(),
        )
    }

    /// Create a service assertion builder
    ///
    /// # Example
    ///
    /// ```ignore
    /// ctx.assert_service("my-svc").has_port(8080).await?;
    /// ctx.assert_service("my-svc").is_cluster_ip().await?;
    /// ```
    #[must_use]
    pub fn assert_service(&self, name: &str) -> crate::assertions::ServiceAssertion {
        crate::assertions::ServiceAssertion::new(
            self.client.clone(),
            self.namespace.clone(),
            name.to_string(),
        )
    }

    /// Create a PVC assertion builder
    ///
    /// # Example
    ///
    /// ```ignore
    /// ctx.assert_pvc("my-data").is_bound().await?;
    /// ctx.assert_pvc("my-data").has_storage_class("standard").await?;
    /// ctx.assert_pvc("my-data").has_capacity("10Gi").await?;
    /// ```
    #[must_use]
    pub fn assert_pvc(&self, name: &str) -> crate::assertions::PvcAssertion {
        crate::assertions::PvcAssertion::new(
            self.client.clone(),
            self.namespace.clone(),
            name.to_string(),
        )
    }

    /// Get resource metrics from the metrics-server API
    ///
    /// Currently supports pod metrics. The target format follows
    /// kubectl-style references: `pod/name` or just `name` (treated as pod).
    ///
    /// # Example
    ///
    /// ```ignore
    /// let metrics = ctx.metrics("pod/api-xyz").await?;
    ///
    /// println!("CPU: {}", metrics.cpu);           // "250m"
    /// println!("Memory: {}", metrics.memory);     // "512Mi"
    /// ```
    ///
    /// # Errors
    ///
    /// Returns `MetricsError::ServerNotAvailable` if metrics-server is not installed.
    /// Returns `MetricsError::PodNotFound` if the specified pod doesn't exist.
    pub async fn metrics(
        &self,
        target: &str,
    ) -> Result<crate::metrics::PodMetrics, crate::metrics::MetricsError> {
        let pod_name = if target.starts_with("pod/") {
            target.strip_prefix("pod/").unwrap_or(target)
        } else {
            target
        };

        crate::metrics::fetch_pod_metrics(&self.client, &self.namespace, pod_name).await
    }

    /// Start an Ingress test chain
    ///
    /// Provides fluent assertions for testing Ingress resources.
    ///
    /// # Example
    ///
    /// ```ignore
    /// ctx.test_ingress("my-ingress")
    ///     .host("api.example.com")
    ///     .path("/v1")
    ///     .expect_backend("api-svc", 8080)
    ///     .expect_tls("api-tls-secret")
    ///     .must();
    /// ```
    #[must_use]
    pub fn test_ingress(&self, name: &str) -> IngressTest {
        IngressTest::new(
            self.client.clone(),
            self.namespace.clone(),
            name.to_string(),
        )
    }
}
