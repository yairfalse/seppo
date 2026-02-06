use super::types::Gvr;
use super::{Context, ContextError};
use kube::api::{Api, DeleteParams, ListParams, PostParams};
use tracing::info;

impl Context {
    /// Apply an unstructured resource using the dynamic client
    ///
    /// Useful for CRDs or when you don't have typed structs.
    /// Creates the resource if it doesn't exist, updates if it does.
    ///
    /// # Example
    ///
    /// ```ignore
    /// let gvr = Gvr::http_route();
    /// ctx.apply_dynamic(&gvr, &json!({
    ///     "apiVersion": "gateway.networking.k8s.io/v1",
    ///     "kind": "HTTPRoute",
    ///     "metadata": { "name": "my-route" },
    ///     "spec": {
    ///         "parentRefs": [{ "name": "my-gateway" }],
    ///         "rules": [{ "backendRefs": [{ "name": "my-service", "port": 8080 }] }]
    ///     }
    /// })).await?;
    /// ```
    pub async fn apply_dynamic(
        &self,
        gvr: &Gvr,
        obj: &serde_json::Value,
    ) -> Result<serde_json::Value, ContextError> {
        use kube::api::DynamicObject;

        let ar = gvr.to_api_resource();
        let api: Api<DynamicObject> =
            Api::namespaced_with(self.client.clone(), &self.namespace, &ar);

        // Parse the JSON into a DynamicObject
        let mut dyn_obj: DynamicObject = serde_json::from_value(obj.clone())
            .map_err(|e| ContextError::ApplyError(format!("invalid object format: {e}")))?;

        // Ensure namespace is set
        if dyn_obj.metadata.namespace.is_none() {
            dyn_obj.metadata.namespace = Some(self.namespace.clone());
        }

        let name = dyn_obj.metadata.name.clone().ok_or_else(|| {
            ContextError::ApplyError("object must have metadata.name".to_string())
        })?;

        // Try to get existing, create or update
        let result = match api.get(&name).await {
            Ok(existing) => {
                // Update - preserve resourceVersion
                dyn_obj.metadata.resource_version = existing.metadata.resource_version;
                api.replace(&name, &PostParams::default(), &dyn_obj)
                    .await
                    .map_err(|e| {
                        ContextError::ApplyError(format!("failed to update {name}: {e}"))
                    })?
            }
            Err(kube::Error::Api(ref ae)) if ae.code == 404 => {
                // Create
                api.create(&PostParams::default(), &dyn_obj)
                    .await
                    .map_err(|e| {
                        ContextError::ApplyError(format!("failed to create {name}: {e}"))
                    })?
            }
            Err(e) => {
                return Err(ContextError::ApplyError(format!(
                    "failed to check {name}: {e}"
                )));
            }
        };

        info!(
            namespace = %self.namespace,
            name = %name,
            gvr = ?gvr,
            "Applied dynamic resource"
        );

        serde_json::to_value(result)
            .map_err(|e| ContextError::ApplyError(format!("failed to serialize result: {e}")))
    }

    /// Get an unstructured resource using the dynamic client
    ///
    /// Returns the resource as a JSON value.
    ///
    /// # Example
    ///
    /// ```ignore
    /// let gvr = Gvr::http_route();
    /// let route = ctx.get_dynamic(&gvr, "my-route").await?;
    /// println!("Route spec: {:?}", route["spec"]);
    /// ```
    pub async fn get_dynamic(
        &self,
        gvr: &Gvr,
        name: &str,
    ) -> Result<serde_json::Value, ContextError> {
        use kube::api::DynamicObject;

        let ar = gvr.to_api_resource();
        let api: Api<DynamicObject> =
            Api::namespaced_with(self.client.clone(), &self.namespace, &ar);

        let obj = api.get(name).await.map_err(|e| {
            ContextError::GetError(format!("{} '{}' not found: {}", gvr.kind, name, e))
        })?;

        serde_json::to_value(obj)
            .map_err(|e| ContextError::GetError(format!("failed to serialize: {e}")))
    }

    /// Delete an unstructured resource using the dynamic client
    ///
    /// # Example
    ///
    /// ```ignore
    /// let gvr = Gvr::http_route();
    /// ctx.delete_dynamic(&gvr, "my-route").await?;
    /// ```
    pub async fn delete_dynamic(&self, gvr: &Gvr, name: &str) -> Result<(), ContextError> {
        use kube::api::DynamicObject;

        let ar = gvr.to_api_resource();
        let api: Api<DynamicObject> =
            Api::namespaced_with(self.client.clone(), &self.namespace, &ar);

        api.delete(name, &DeleteParams::default())
            .await
            .map_err(|e| {
                ContextError::DeleteError(format!("{} '{}' delete failed: {}", gvr.kind, name, e))
            })?;

        info!(
            namespace = %self.namespace,
            name = %name,
            gvr = ?gvr,
            "Deleted dynamic resource"
        );

        Ok(())
    }

    /// List unstructured resources using the dynamic client
    ///
    /// Returns resources as a vector of JSON values.
    ///
    /// # Example
    ///
    /// ```ignore
    /// let gvr = Gvr::http_route();
    /// let routes = ctx.list_dynamic(&gvr).await?;
    /// for route in routes {
    ///     println!("Route: {}", route["metadata"]["name"]);
    /// }
    /// ```
    pub async fn list_dynamic(&self, gvr: &Gvr) -> Result<Vec<serde_json::Value>, ContextError> {
        use kube::api::DynamicObject;

        let ar = gvr.to_api_resource();
        let api: Api<DynamicObject> =
            Api::namespaced_with(self.client.clone(), &self.namespace, &ar);

        let list = api.list(&ListParams::default()).await.map_err(|e| {
            ContextError::ListError(format!("failed to list {}: {}", gvr.resource, e))
        })?;

        list.items
            .into_iter()
            .map(|obj| {
                serde_json::to_value(obj)
                    .map_err(|e| ContextError::ListError(format!("failed to serialize: {e}")))
            })
            .collect()
    }
}
