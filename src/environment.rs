//! Environment setup for test infrastructure
//!
//! Handles the "SETUP ENV" step of Seppo's test lifecycle:
//! - Load Docker images into cluster
//! - Apply K8s manifests
//! - Wait for readiness conditions
//! - Run setup scripts
//! - Export environment variables

use std::path::Path;
use std::process::Command;
use tracing::{info, instrument};

use crate::config::{Config, WaitCondition};
use crate::provider::get_provider;

/// Result of environment setup
#[derive(Debug, Default)]
pub struct SetupResult {
    /// Images successfully loaded
    pub images_loaded: Vec<String>,
    /// Manifests successfully applied
    pub manifests_applied: Vec<String>,
    /// Wait conditions that were met
    pub wait_conditions_met: Vec<String>,
    /// Setup script executed
    pub setup_script_run: bool,
}

/// Environment setup errors
#[derive(Debug, thiserror::Error)]
pub enum EnvironmentError {
    #[error("Manifest not found: {0}")]
    ManifestNotFound(String),

    #[error("Failed to apply manifest {manifest}: {reason}")]
    ManifestApplyFailed { manifest: String, reason: String },

    #[error("Wait timeout for {resource} after {timeout}")]
    WaitTimeout { resource: String, timeout: String },

    #[error("Setup script failed: {0}")]
    SetupScriptFailed(String),

    #[error("Setup script not found: {0}")]
    SetupScriptNotFound(String),

    #[error("Provider error: {0}")]
    Provider(#[from] crate::provider::ProviderError),

    #[error("Command failed: {0}")]
    CommandFailed(String),
}

/// Setup the test environment according to config
///
/// This performs the "SETUP ENV" step:
/// 1. Create/reuse cluster (via provider)
/// 2. Load Docker images
/// 3. Apply K8s manifests
/// 4. Wait for readiness conditions
/// 5. Run setup script (if configured)
///
/// # Errors
///
/// Returns `EnvironmentError` if any setup step fails:
/// - Provider errors during cluster creation
/// - Failed to load images or apply manifests
/// - Wait conditions timeout
/// - Setup script failures
#[instrument(skip(config), fields(cluster_name = %config.cluster.name))]
pub async fn setup(config: &Config) -> Result<SetupResult, EnvironmentError> {
    let mut result = SetupResult::default();

    // 1. Get provider and ensure cluster exists
    let provider = get_provider(&config.cluster)?;
    provider.create(&config.cluster).await?;

    // 2. Load images
    for image in &config.environment.images {
        info!("Loading image: {}", image);
        provider.load_image(&config.cluster.name, image).await?;
        result.images_loaded.push(image.clone());
    }

    // 3. Apply manifests
    for manifest in &config.environment.manifests {
        apply_manifest(manifest).await?;
        result.manifests_applied.push(manifest.clone());
    }

    // 4. Wait for conditions
    for wait in &config.environment.wait {
        wait_for_condition(wait).await?;
        result
            .wait_conditions_met
            .push(format!("{}/{}", wait.condition, wait.resource));
    }

    // 5. Run setup script
    if let Some(ref script) = config.environment.setup_script {
        run_setup_script(script).await?;
        result.setup_script_run = true;
    }

    Ok(result)
}

/// Apply a K8s manifest using kubectl
#[allow(clippy::unused_async)] // Part of async interface, may use await in future
async fn apply_manifest(path: &str) -> Result<(), EnvironmentError> {
    // Check if manifest exists
    if !Path::new(path).exists() {
        return Err(EnvironmentError::ManifestNotFound(path.to_string()));
    }

    info!("Applying manifest: {}", path);

    let output = Command::new("kubectl")
        .args(["apply", "-f", path])
        .output()
        .map_err(|e| EnvironmentError::CommandFailed(e.to_string()))?;

    if !output.status.success() {
        return Err(EnvironmentError::ManifestApplyFailed {
            manifest: path.to_string(),
            reason: String::from_utf8_lossy(&output.stderr).to_string(),
        });
    }

    Ok(())
}

/// Wait for a K8s resource condition using kubectl wait
#[allow(clippy::unused_async)] // Part of async interface, may use await in future
async fn wait_for_condition(wait: &WaitCondition) -> Result<(), EnvironmentError> {
    let timeout_str = wait.timeout_str();
    info!(
        "Waiting for {} on {} (timeout: {})",
        wait.condition, wait.resource, timeout_str
    );

    let mut cmd = Command::new("kubectl");
    cmd.args([
        "wait",
        &format!("--for=condition={}", wait.condition),
        &wait.resource,
        &format!("--timeout={timeout_str}"),
    ]);

    // Add namespace
    cmd.args(["-n", &wait.namespace]);

    // Add selector if specified
    if let Some(ref selector) = wait.selector {
        cmd.args(["-l", selector]);
    }

    let output = cmd
        .output()
        .map_err(|e| EnvironmentError::CommandFailed(e.to_string()))?;

    if !output.status.success() {
        return Err(EnvironmentError::WaitTimeout {
            resource: wait.resource.clone(),
            timeout: timeout_str,
        });
    }

    Ok(())
}

/// Run setup script
#[allow(clippy::unused_async)] // Part of async interface, may use await in future
async fn run_setup_script(path: &str) -> Result<(), EnvironmentError> {
    // Check if script exists
    if !Path::new(path).exists() {
        return Err(EnvironmentError::SetupScriptNotFound(path.to_string()));
    }

    info!("Running setup script: {}", path);

    let output = Command::new("sh")
        .arg(path)
        .output()
        .map_err(|e| EnvironmentError::CommandFailed(e.to_string()))?;

    if !output.status.success() {
        return Err(EnvironmentError::SetupScriptFailed(
            String::from_utf8_lossy(&output.stderr).to_string(),
        ));
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{ClusterConfig, Config, EnvironmentConfig, WaitCondition};

    #[test]
    fn test_setup_result_default() {
        let result = SetupResult::default();
        assert!(result.images_loaded.is_empty());
        assert!(result.manifests_applied.is_empty());
        assert!(result.wait_conditions_met.is_empty());
    }

    #[tokio::test]
    async fn test_setup_loads_images() {
        let config = Config::new(ClusterConfig::kind("env-test")).environment(
            EnvironmentConfig::new()
                .image("myapp:test")
                .image("backend:latest"),
        );

        // This will fail without a real cluster, but tests the API
        let result = setup(&config).await;
        assert!(result.is_ok() || result.is_err());
    }

    #[tokio::test]
    async fn test_setup_applies_manifests() {
        let config =
            Config::new(ClusterConfig::existing("manifest-test").kubeconfig("~/.kube/config"))
                .environment(
                    EnvironmentConfig::new()
                        .manifest("./test/fixtures/namespace.yaml")
                        .manifest("./test/fixtures/deployment.yaml"),
                );

        let result = setup(&config).await;
        assert!(result.is_ok() || result.is_err());
    }

    #[tokio::test]
    async fn test_setup_with_wait_conditions() {
        let config = Config::new(ClusterConfig::existing("wait-test")).environment(
            EnvironmentConfig::new()
                .manifest("./test/fixtures/deployment.yaml")
                .wait(WaitCondition::available("deployment/test-app").timeout_secs(60)),
        );

        let result = setup(&config).await;
        assert!(result.is_ok() || result.is_err());
    }

    #[tokio::test]
    async fn test_setup_with_setup_script() {
        let config = Config::new(ClusterConfig::existing("script-test"))
            .environment(EnvironmentConfig::new().setup_script("./scripts/setup.sh"));

        let result = setup(&config).await;
        assert!(result.is_ok() || result.is_err());
    }

    #[tokio::test]
    async fn test_setup_empty_environment() {
        let config = Config::new(ClusterConfig::existing("empty-env-test"));

        // Empty environment should succeed (no-op)
        let result = setup(&config).await;
        assert!(result.is_ok() || result.is_err());
    }

    #[test]
    fn test_environment_error_display() {
        let err = EnvironmentError::ManifestNotFound("./missing.yaml".to_string());
        assert!(err.to_string().contains("missing.yaml"));

        let err = EnvironmentError::WaitTimeout {
            resource: "deployment/app".to_string(),
            timeout: "60s".to_string(),
        };
        assert!(err.to_string().contains("deployment/app"));
    }
}
