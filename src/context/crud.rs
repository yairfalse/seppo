use super::{improve_error_message, Context, ContextError};
use kube::api::{Api, DeleteParams, ListParams, Patch, PatchParams};
use tracing::info;

impl Context {
    /// Apply a resource to the test namespace
    ///
    /// Creates or updates the resource in the test namespace using server-side apply.
    /// This works like `kubectl apply` - it will create if not exists, or update if exists.
    /// Overrides any namespace specified in the resource metadata with the test namespace.
    ///
    /// # Example
    ///
    /// ```ignore
    /// // First apply creates
    /// ctx.apply(&deployment).await?;
    ///
    /// // Second apply updates
    /// // Assuming spec is set
    /// deployment.spec.as_mut().unwrap().replicas = Some(5);
    /// ctx.apply(&deployment).await?;
    /// ```
    pub async fn apply<K>(&self, resource: &K) -> Result<K, ContextError>
    where
        K: kube::Resource<Scope = kube::core::NamespaceResourceScope>
            + Clone
            + serde::de::DeserializeOwned
            + serde::Serialize
            + std::fmt::Debug,
        <K as kube::Resource>::DynamicType: Default,
    {
        let api: Api<K> = Api::namespaced(self.client.clone(), &self.namespace);

        // Clone and set namespace to our test namespace
        let mut resource = resource.clone();
        resource.meta_mut().namespace = Some(self.namespace.clone());

        // Get resource name for the patch call
        let name = resource
            .meta()
            .name
            .as_ref()
            .ok_or_else(|| ContextError::ApplyError("resource must have a name".to_string()))?;

        // Use server-side apply (like kubectl apply)
        // Field manager identifies who owns these fields for conflict detection
        let patch_params = PatchParams::apply("seppo-sdk").force();
        let kind = K::kind(&Default::default()).to_string();
        let applied = api
            .patch(name, &patch_params, &Patch::Apply(&resource))
            .await
            .map_err(|e| ContextError::ApplyError(improve_error_message(&e, &kind, name)))?;

        info!(
            namespace = %self.namespace,
            name = ?applied.meta().name,
            "Applied resource"
        );

        Ok(applied)
    }

    /// Apply a cluster-scoped resource (create or update)
    ///
    /// Use this for resources that are not namespaced, such as:
    /// - `ClusterRole`, `ClusterRoleBinding`
    /// - `GatewayClass`
    /// - `CustomResourceDefinition`
    /// - Namespace
    /// - `PersistentVolume`
    /// - `StorageClass`
    ///
    /// # Example
    ///
    /// ```ignore
    /// use k8s_openapi::api::rbac::v1::ClusterRole;
    ///
    /// let role = ClusterRole {
    ///     metadata: ObjectMeta {
    ///         name: Some("my-cluster-role".to_string()),
    ///         ..Default::default()
    ///     },
    ///     rules: Some(vec![]),
    ///     ..Default::default()
    /// };
    ///
    /// ctx.apply_cluster(&role).await?;
    /// ```
    pub async fn apply_cluster<K>(&self, resource: &K) -> Result<K, ContextError>
    where
        K: kube::Resource<Scope = kube::core::ClusterResourceScope>
            + Clone
            + serde::de::DeserializeOwned
            + serde::Serialize
            + std::fmt::Debug,
        <K as kube::Resource>::DynamicType: Default,
    {
        let api: Api<K> = Api::all(self.client.clone());

        let name = resource
            .meta()
            .name
            .as_ref()
            .ok_or_else(|| ContextError::ApplyError("resource must have a name".to_string()))?;

        // Use server-side apply (like kubectl apply)
        let patch_params = PatchParams::apply("seppo-sdk").force();
        let kind = K::kind(&Default::default()).to_string();
        let applied = api
            .patch(name, &patch_params, &Patch::Apply(resource))
            .await
            .map_err(|e| ContextError::ApplyError(improve_error_message(&e, &kind, name)))?;

        info!(
            name = ?applied.meta().name,
            "Applied cluster-scoped resource"
        );

        Ok(applied)
    }

    /// Get a resource from the test namespace
    pub async fn get<K>(&self, name: &str) -> Result<K, ContextError>
    where
        K: kube::Resource<Scope = kube::core::NamespaceResourceScope>
            + Clone
            + serde::de::DeserializeOwned
            + std::fmt::Debug,
        <K as kube::Resource>::DynamicType: Default,
    {
        let api: Api<K> = Api::namespaced(self.client.clone(), &self.namespace);
        let kind = K::kind(&Default::default()).to_string();

        let resource = api
            .get(name)
            .await
            .map_err(|e| ContextError::GetError(improve_error_message(&e, &kind, name)))?;

        Ok(resource)
    }

    /// Get a cluster-scoped resource
    ///
    /// # Example
    ///
    /// ```ignore
    /// use k8s_openapi::api::rbac::v1::ClusterRole;
    ///
    /// let role: ClusterRole = ctx.get_cluster("my-cluster-role").await?;
    /// ```
    pub async fn get_cluster<K>(&self, name: &str) -> Result<K, ContextError>
    where
        K: kube::Resource<Scope = kube::core::ClusterResourceScope>
            + Clone
            + serde::de::DeserializeOwned
            + std::fmt::Debug,
        <K as kube::Resource>::DynamicType: Default,
    {
        let api: Api<K> = Api::all(self.client.clone());
        let kind = K::kind(&Default::default()).to_string();

        let resource = api
            .get(name)
            .await
            .map_err(|e| ContextError::GetError(improve_error_message(&e, &kind, name)))?;

        Ok(resource)
    }

    /// Delete a resource from the test namespace
    pub async fn delete<K>(&self, name: &str) -> Result<(), ContextError>
    where
        K: kube::Resource<Scope = kube::core::NamespaceResourceScope>
            + Clone
            + serde::de::DeserializeOwned
            + std::fmt::Debug,
        <K as kube::Resource>::DynamicType: Default,
    {
        let api: Api<K> = Api::namespaced(self.client.clone(), &self.namespace);
        let kind = K::kind(&Default::default()).to_string();

        api.delete(name, &DeleteParams::default())
            .await
            .map_err(|e| ContextError::DeleteError(improve_error_message(&e, &kind, name)))?;

        info!(
            namespace = %self.namespace,
            name = %name,
            "Deleted resource"
        );

        Ok(())
    }

    /// Delete a cluster-scoped resource
    ///
    /// # Example
    ///
    /// ```ignore
    /// use k8s_openapi::api::rbac::v1::ClusterRole;
    ///
    /// ctx.delete_cluster::<ClusterRole>("my-cluster-role").await?;
    /// ```
    pub async fn delete_cluster<K>(&self, name: &str) -> Result<(), ContextError>
    where
        K: kube::Resource<Scope = kube::core::ClusterResourceScope>
            + Clone
            + serde::de::DeserializeOwned
            + std::fmt::Debug,
        <K as kube::Resource>::DynamicType: Default,
    {
        let api: Api<K> = Api::all(self.client.clone());
        let kind = K::kind(&Default::default()).to_string();

        api.delete(name, &DeleteParams::default())
            .await
            .map_err(|e| ContextError::DeleteError(improve_error_message(&e, &kind, name)))?;

        info!(
            name = %name,
            "Deleted cluster-scoped resource"
        );

        Ok(())
    }

    /// List resources of a given type in the test namespace
    pub async fn list<K>(&self) -> Result<Vec<K>, ContextError>
    where
        K: kube::Resource<Scope = kube::core::NamespaceResourceScope>
            + Clone
            + serde::de::DeserializeOwned
            + std::fmt::Debug,
        <K as kube::Resource>::DynamicType: Default,
    {
        let api: Api<K> = Api::namespaced(self.client.clone(), &self.namespace);
        let kind = K::kind(&Default::default()).to_string();

        let list = api
            .list(&ListParams::default())
            .await
            .map_err(|e| ContextError::ListError(format!("failed to list {kind}: {e}")))?;

        Ok(list.items)
    }
}
