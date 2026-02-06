use super::{improve_error_message, Context, ContextError};
use json_patch::Patch as JsonPatchOps;
use kube::api::{Api, Patch, PatchParams};
use tracing::info;

impl Context {
    /// Patch a resource in the test namespace using JSON Merge Patch
    ///
    /// Allows partial updates to a resource without replacing the entire spec.
    /// Uses JSON Merge Patch (RFC 7396) - fields set to `null` are deleted,
    /// other fields are merged.
    ///
    /// # Example
    ///
    /// ```ignore
    /// use serde_json::json;
    ///
    /// // Update deployment replicas
    /// ctx.patch::<Deployment>("myapp", &json!({
    ///     "spec": { "replicas": 5 }
    /// })).await?;
    ///
    /// // Add a label to a pod
    /// ctx.patch::<Pod>("worker", &json!({
    ///     "metadata": { "labels": { "version": "v2" } }
    /// })).await?;
    /// ```
    pub async fn patch<K>(&self, name: &str, patch: &serde_json::Value) -> Result<K, ContextError>
    where
        K: kube::Resource<Scope = kube::core::NamespaceResourceScope>
            + Clone
            + serde::de::DeserializeOwned
            + std::fmt::Debug,
        <K as kube::Resource>::DynamicType: Default,
    {
        let api: Api<K> = Api::namespaced(self.client.clone(), &self.namespace);

        let kind = K::kind(&Default::default()).to_string();
        let patched = api
            .patch(name, &PatchParams::default(), &Patch::Merge(patch))
            .await
            .map_err(|e| ContextError::PatchError(improve_error_message(&e, &kind, name)))?;

        info!(
            namespace = %self.namespace,
            name = %name,
            "Patched resource"
        );

        Ok(patched)
    }

    /// Strategic merge patch a resource
    ///
    /// Uses Kubernetes strategic merge patch, which handles arrays specially
    /// (e.g., merging container lists by name rather than index).
    ///
    /// Best for K8s-native resources where array merging semantics matter.
    ///
    /// # Example
    ///
    /// ```ignore
    /// use k8s_openapi::api::apps::v1::Deployment;
    ///
    /// // Add a label using strategic merge
    /// ctx.patch_strategic::<Deployment>("myapp", &json!({
    ///     "metadata": { "labels": { "version": "v2" } }
    /// })).await?;
    /// ```
    pub async fn patch_strategic<K>(
        &self,
        name: &str,
        patch: &serde_json::Value,
    ) -> Result<K, ContextError>
    where
        K: kube::Resource<Scope = kube::core::NamespaceResourceScope>
            + Clone
            + serde::de::DeserializeOwned
            + std::fmt::Debug,
        <K as kube::Resource>::DynamicType: Default,
    {
        let api: Api<K> = Api::namespaced(self.client.clone(), &self.namespace);

        let kind = K::kind(&Default::default()).to_string();
        let patched = api
            .patch(name, &PatchParams::default(), &Patch::Strategic(patch))
            .await
            .map_err(|e| ContextError::PatchError(improve_error_message(&e, &kind, name)))?;

        info!(
            namespace = %self.namespace,
            name = %name,
            "Strategic merge patched resource"
        );

        Ok(patched)
    }

    /// JSON patch a resource (RFC 6902)
    ///
    /// Uses JSON Patch format - an array of operations.
    /// Best for precise, surgical changes to specific fields.
    ///
    /// # Example
    ///
    /// ```ignore
    /// use k8s_openapi::api::apps::v1::Deployment;
    ///
    /// // Add a label using JSON patch operations
    /// ctx.patch_json::<Deployment>("myapp", &json!([
    ///     { "op": "add", "path": "/metadata/labels/version", "value": "v2" }
    /// ])).await?;
    /// ```
    pub async fn patch_json<K>(
        &self,
        name: &str,
        patch: &serde_json::Value,
    ) -> Result<K, ContextError>
    where
        K: kube::Resource<Scope = kube::core::NamespaceResourceScope>
            + Clone
            + serde::de::DeserializeOwned
            + std::fmt::Debug,
        <K as kube::Resource>::DynamicType: Default,
    {
        // Parse the JSON value into json_patch::Patch
        let json_patch: JsonPatchOps = serde_json::from_value(patch.clone()).map_err(|e| {
            ContextError::PatchError(format!(
                "invalid JSON patch format: {e}. Expected array of operations like \
                 [{{\"op\": \"add\", \"path\": \"/path\", \"value\": ...}}]"
            ))
        })?;

        let api: Api<K> = Api::namespaced(self.client.clone(), &self.namespace);

        let kind = K::kind(&Default::default()).to_string();
        let patched = api
            .patch(
                name,
                &PatchParams::default(),
                &Patch::Json::<()>(json_patch),
            )
            .await
            .map_err(|e| ContextError::PatchError(improve_error_message(&e, &kind, name)))?;

        info!(
            namespace = %self.namespace,
            name = %name,
            "JSON patched resource"
        );

        Ok(patched)
    }
}
