use super::parsing::extract_resource_name;
use super::{Context, ContextError};
use futures::Stream;
use k8s_openapi::api::core::v1::PersistentVolumeClaim;
use kube::api::Api;
use kube::runtime::watcher::{self, Event as WatchEvent};
use tracing::debug;

impl Context {
    /// Wait for a resource to satisfy a condition
    ///
    /// Polls the resource until the condition returns true or timeout is reached.
    /// Default timeout is 60 seconds, polling interval is 1 second.
    ///
    /// Accepts both formats for consistency with other methods:
    /// - `"my-config"` - just the resource name
    /// - `"configmap/my-config"` - kind/name format (kind is ignored, type comes from generic)
    ///
    /// # Example
    ///
    /// ```ignore
    /// // Wait for a ConfigMap to have a specific key
    /// ctx.wait_for::<ConfigMap>("my-config", |cm| {
    ///     cm.data.as_ref().map_or(false, |d| d.contains_key("ready"))
    /// }).await?;
    ///
    /// // Also works with kind/name format
    /// ctx.wait_for::<Gateway>("gateway/test-gw", |gw| {
    ///     gw.status.is_some()
    /// }).await?;
    /// ```
    pub async fn wait_for<K, F>(&self, name: &str, condition: F) -> Result<K, ContextError>
    where
        K: kube::Resource<Scope = kube::core::NamespaceResourceScope>
            + Clone
            + serde::de::DeserializeOwned
            + std::fmt::Debug,
        <K as kube::Resource>::DynamicType: Default,
        F: Fn(&K) -> bool,
    {
        self.wait_for_with_timeout(name, condition, std::time::Duration::from_secs(60))
            .await
    }

    /// Wait for a resource with custom timeout
    ///
    /// Accepts both `"name"` and `"kind/name"` formats for consistency.
    pub async fn wait_for_with_timeout<K, F>(
        &self,
        name: &str,
        condition: F,
        timeout: std::time::Duration,
    ) -> Result<K, ContextError>
    where
        K: kube::Resource<Scope = kube::core::NamespaceResourceScope>
            + Clone
            + serde::de::DeserializeOwned
            + std::fmt::Debug,
        <K as kube::Resource>::DynamicType: Default,
        F: Fn(&K) -> bool,
    {
        // Extract just the name if "kind/name" format was provided
        let name = extract_resource_name(name);
        let start = std::time::Instant::now();
        let poll_interval = std::time::Duration::from_secs(1);

        debug!(
            namespace = %self.namespace,
            resource = %name,
            timeout = ?timeout,
            "Starting wait_for"
        );

        loop {
            match self.get::<K>(name).await {
                Ok(resource) => {
                    if condition(&resource) {
                        debug!(
                            namespace = %self.namespace,
                            resource = %name,
                            elapsed = ?start.elapsed(),
                            "Condition met"
                        );
                        return Ok(resource);
                    }
                    debug!(
                        namespace = %self.namespace,
                        resource = %name,
                        elapsed = ?start.elapsed(),
                        "Resource exists but condition not met, waiting..."
                    );
                }
                Err(ContextError::GetError(_)) => {
                    debug!(
                        namespace = %self.namespace,
                        resource = %name,
                        elapsed = ?start.elapsed(),
                        "Resource doesn't exist yet, waiting..."
                    );
                }
                Err(e) => return Err(e),
            }

            if start.elapsed() >= timeout {
                return Err(crate::wait::WaitError::new(name, timeout, start.elapsed())
                    .with_state("condition not met")
                    .into());
            }

            tokio::time::sleep(poll_interval).await;
        }
    }

    /// Wait for a resource to be deleted
    ///
    /// Polls until the resource no longer exists in the namespace.
    /// Default timeout is 60 seconds, polling interval is 1 second.
    ///
    /// Accepts both `"name"` and `"kind/name"` formats for consistency.
    ///
    /// # Example
    ///
    /// ```ignore
    /// ctx.delete::<Pod>("worker").await?;
    /// ctx.wait_deleted::<Pod>("worker").await?;
    /// // Pod is now fully gone
    ///
    /// // Also works with kind/name format
    /// ctx.wait_deleted::<Pod>("pod/worker").await?;
    /// ```
    pub async fn wait_deleted<K>(&self, name: &str) -> Result<(), ContextError>
    where
        K: kube::Resource<Scope = kube::core::NamespaceResourceScope>
            + Clone
            + serde::de::DeserializeOwned
            + std::fmt::Debug,
        <K as kube::Resource>::DynamicType: Default,
    {
        self.wait_deleted_with_timeout::<K>(name, std::time::Duration::from_secs(60))
            .await
    }

    /// Wait for a resource to be deleted with custom timeout
    ///
    /// Accepts both `"name"` and `"kind/name"` formats for consistency.
    pub async fn wait_deleted_with_timeout<K>(
        &self,
        name: &str,
        timeout: std::time::Duration,
    ) -> Result<(), ContextError>
    where
        K: kube::Resource<Scope = kube::core::NamespaceResourceScope>
            + Clone
            + serde::de::DeserializeOwned
            + std::fmt::Debug,
        <K as kube::Resource>::DynamicType: Default,
    {
        // Extract just the name if "kind/name" format was provided
        let name = extract_resource_name(name);
        let api: Api<K> = Api::namespaced(self.client.clone(), &self.namespace);
        let start = std::time::Instant::now();
        let poll_interval = std::time::Duration::from_secs(1);

        debug!(
            namespace = %self.namespace,
            resource = %name,
            timeout = ?timeout,
            "Starting wait_deleted"
        );

        loop {
            match api.get(name).await {
                Ok(_) => {
                    // Resource still exists, keep waiting
                    debug!(
                        namespace = %self.namespace,
                        resource = %name,
                        elapsed = ?start.elapsed(),
                        "Resource still exists, waiting for deletion..."
                    );
                }
                Err(kube::Error::Api(err)) if err.code == 404 => {
                    // Resource is gone (HTTP 404 Not Found)
                    debug!(
                        namespace = %self.namespace,
                        resource = %name,
                        elapsed = ?start.elapsed(),
                        "Resource deleted"
                    );
                    return Ok(());
                }
                Err(e) => {
                    return Err(ContextError::GetError(e.to_string()));
                }
            }

            if start.elapsed() >= timeout {
                return Err(crate::wait::WaitError::new(name, timeout, start.elapsed())
                    .with_state("still exists")
                    .into());
            }

            tokio::time::sleep(poll_interval).await;
        }
    }

    /// Wait for a `PersistentVolumeClaim` to be bound
    ///
    /// Polls until the PVC's status.phase is "Bound".
    /// Default timeout is 60 seconds, polling interval is 1 second.
    ///
    /// Accepts both `"name"` and `"pvc/name"` formats for consistency.
    ///
    /// # Example
    ///
    /// ```ignore
    /// ctx.apply(&my_pvc).await?;
    /// ctx.wait_pvc_bound("my-pvc").await?;
    /// // PVC is now bound and ready
    /// ```
    pub async fn wait_pvc_bound(&self, name: &str) -> Result<PersistentVolumeClaim, ContextError> {
        self.wait_pvc_bound_with_timeout(name, std::time::Duration::from_secs(60))
            .await
    }

    /// Wait for a PVC to be bound with a custom timeout
    pub async fn wait_pvc_bound_with_timeout(
        &self,
        name: &str,
        timeout: std::time::Duration,
    ) -> Result<PersistentVolumeClaim, ContextError> {
        // Extract just the name if "pvc/name" format was provided
        let name = extract_resource_name(name);
        let start = std::time::Instant::now();
        let poll_interval = std::time::Duration::from_secs(1);

        debug!(
            namespace = %self.namespace,
            pvc = %name,
            timeout = ?timeout,
            "Starting wait_pvc_bound"
        );

        loop {
            match self.get::<PersistentVolumeClaim>(name).await {
                Ok(pvc) => {
                    let phase = pvc
                        .status
                        .as_ref()
                        .and_then(|s| s.phase.as_ref())
                        .map_or("Unknown", std::string::String::as_str);

                    if phase == "Bound" {
                        debug!(
                            namespace = %self.namespace,
                            pvc = %name,
                            elapsed = ?start.elapsed(),
                            "PVC is bound"
                        );
                        return Ok(pvc);
                    }

                    debug!(
                        namespace = %self.namespace,
                        pvc = %name,
                        phase = %phase,
                        elapsed = ?start.elapsed(),
                        "PVC not bound yet, waiting..."
                    );
                }
                Err(ContextError::GetError(_)) => {
                    debug!(
                        namespace = %self.namespace,
                        pvc = %name,
                        elapsed = ?start.elapsed(),
                        "PVC doesn't exist yet, waiting..."
                    );
                }
                Err(e) => return Err(e),
            }

            if start.elapsed() >= timeout {
                return Err(crate::wait::WaitError::new(
                    format!("pvc/{name}"),
                    timeout,
                    start.elapsed(),
                )
                .with_state("not bound")
                .into());
            }

            tokio::time::sleep(poll_interval).await;
        }
    }

    /// Watch for changes to resources of a given type
    ///
    /// Returns a stream of watch events for all resources of type K in the namespace.
    /// Events include Applied (create/update) and Deleted.
    ///
    /// # Example
    ///
    /// ```ignore
    /// use futures::StreamExt;
    ///
    /// let mut stream = ctx.watch::<Pod>();
    /// while let Some(event) = stream.next().await {
    ///     match event {
    ///         Ok(WatchEvent::Applied(pod)) => println!("Pod changed: {:?}", pod.metadata.name),
    ///         Ok(WatchEvent::Deleted(pod)) => println!("Pod deleted: {:?}", pod.metadata.name),
    ///         Err(e) => eprintln!("Watch error: {}", e),
    ///         _ => {}
    ///     }
    /// }
    /// ```
    pub fn watch<K>(&self) -> impl Stream<Item = Result<WatchEvent<K>, watcher::Error>> + Send + '_
    where
        K: kube::Resource<Scope = kube::core::NamespaceResourceScope>
            + Clone
            + serde::de::DeserializeOwned
            + std::fmt::Debug
            + Send
            + 'static,
        <K as kube::Resource>::DynamicType: Default + Clone,
    {
        let api: Api<K> = Api::namespaced(self.client.clone(), &self.namespace);
        let config = watcher::Config::default();

        debug!(
            namespace = %self.namespace,
            "Starting watch"
        );

        watcher::watcher(api, config)
    }

    /// Watch for changes to a specific resource
    ///
    /// Returns a stream of watch events for a single named resource.
    ///
    /// # Example
    ///
    /// ```ignore
    /// use futures::StreamExt;
    ///
    /// let mut stream = ctx.watch_one::<Deployment>("myapp");
    /// while let Some(event) = stream.next().await {
    ///     if let Ok(WatchEvent::Applied(dep)) = event {
    ///         let ready = dep.status.as_ref().and_then(|s| s.ready_replicas).unwrap_or(0);
    ///         println!("Deployment myapp: {} ready replicas", ready);
    ///     }
    /// }
    /// ```
    pub fn watch_one<K>(
        &self,
        name: &str,
    ) -> impl Stream<Item = Result<WatchEvent<K>, watcher::Error>> + Send + '_
    where
        K: kube::Resource<Scope = kube::core::NamespaceResourceScope>
            + Clone
            + serde::de::DeserializeOwned
            + std::fmt::Debug
            + Send
            + 'static,
        <K as kube::Resource>::DynamicType: Default + Clone,
    {
        let api: Api<K> = Api::namespaced(self.client.clone(), &self.namespace);
        let field_selector = format!("metadata.name={name}");
        let config = watcher::Config::default().fields(&field_selector);

        debug!(
            namespace = %self.namespace,
            resource = %name,
            "Starting watch for single resource"
        );

        watcher::watcher(api, config)
    }
}
