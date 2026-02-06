use super::Context;
use crate::stack::{Stack, StackError};
use k8s_openapi::api::apps::v1::Deployment;
use k8s_openapi::api::core::v1::Service;
use kube::api::{Api, PostParams};
use tracing::info;

impl Context {
    /// Deploy a stack of services to the test namespace
    ///
    /// Creates Deployments and Services for all services defined in the stack.
    ///
    /// # Example
    ///
    /// ```ignore
    /// let stack = Stack::new()
    ///     .service("frontend").image("fe:test").replicas(4).port(80)
    ///     .service("backend").image("be:test").replicas(2).port(8080)
    ///     .build();  // <-- This is required
    ///
    /// ctx.up(&stack).await?;
    /// ```
    pub async fn up(&self, stack: &Stack) -> Result<(), StackError> {
        use futures::future::try_join_all;

        if stack.services().is_empty() {
            return Err(StackError::EmptyStack);
        }

        // Validate all services have required fields
        for svc_def in stack.services() {
            if svc_def.image.is_empty() {
                return Err(StackError::DeployError(
                    svc_def.name.clone(),
                    "image is required".to_string(),
                ));
            }
        }

        let deployments: Api<Deployment> = Api::namespaced(self.client.clone(), &self.namespace);
        let services: Api<Service> = Api::namespaced(self.client.clone(), &self.namespace);
        let namespace = self.namespace.clone();

        // Create all deployments concurrently
        let deployment_futures = stack.services().iter().map(|svc_def| {
            let deployments = deployments.clone();
            let deployment = stack.deployment_for(svc_def, &namespace);
            let name = svc_def.name.clone();
            async move {
                deployments
                    .create(&PostParams::default(), &deployment)
                    .await
                    .map_err(|e| StackError::DeployError(name, e.to_string()))
            }
        });

        try_join_all(deployment_futures).await?;

        for svc_def in stack.services() {
            info!(
                namespace = %self.namespace,
                service = %svc_def.name,
                replicas = %svc_def.replicas,
                "Deployed service"
            );
        }

        // Create all services concurrently (for those with ports)
        let service_futures = stack
            .services()
            .iter()
            .filter_map(|svc_def| {
                stack.service_for(svc_def, &namespace).map(|k8s_svc| {
                    let services = services.clone();
                    let name = svc_def.name.clone();
                    async move {
                        services
                            .create(&PostParams::default(), &k8s_svc)
                            .await
                            .map_err(|e| StackError::DeployError(name, e.to_string()))
                    }
                })
            })
            .collect::<Vec<_>>();

        try_join_all(service_futures).await?;

        for svc_def in stack.services().iter().filter(|s| s.port.is_some()) {
            info!(
                namespace = %self.namespace,
                service = %svc_def.name,
                port = ?svc_def.port,
                "Created service"
            );
        }

        Ok(())
    }
}
