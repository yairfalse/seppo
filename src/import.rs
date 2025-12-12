//! Import helpers for Helm and Kustomize
//!
//! Migration path from YAML-based tooling to Seppo.
//! Import existing Helm charts or Kustomize overlays, then gradually
//! migrate to native Rust code.
//!
//! # Example
//!
//! ```ignore
//! use seppo::import::{helm_template, HelmOptions};
//!
//! // Render a Helm chart to K8s manifests
//! let manifests = helm_template("./charts/myapp", HelmOptions::default())?;
//!
//! // Apply them
//! for manifest in manifests {
//!     ctx.apply_yaml(&manifest).await?;
//! }
//! ```

use std::path::Path;
use std::process::Command;

/// Error type for import operations
#[derive(Debug, thiserror::Error)]
pub enum ImportError {
    #[error("Helm not found. Install with: brew install helm")]
    HelmNotFound,

    #[error("Kustomize not found. Install with: brew install kustomize")]
    KustomizeNotFound,

    #[error("Helm command failed: {0}")]
    HelmFailed(String),

    #[error("Kustomize command failed: {0}")]
    KustomizeFailed(String),

    #[error("Chart not found: {0}")]
    ChartNotFound(String),

    #[error("Invalid YAML: {0}")]
    InvalidYaml(String),

    #[error("IO error: {0}")]
    IoError(#[from] std::io::Error),
}

/// Options for Helm template rendering
#[derive(Debug, Clone, Default)]
pub struct HelmOptions {
    /// Release name (default: "release")
    pub release_name: Option<String>,
    /// Namespace for the release
    pub namespace: Option<String>,
    /// Values to override (key=value pairs)
    pub set_values: Vec<(String, String)>,
    /// Values files to use
    pub values_files: Vec<String>,
}

impl HelmOptions {
    /// Create new HelmOptions with a release name
    pub fn new(release_name: &str) -> Self {
        Self {
            release_name: Some(release_name.to_string()),
            ..Default::default()
        }
    }

    /// Set the namespace
    pub fn namespace(mut self, ns: &str) -> Self {
        self.namespace = Some(ns.to_string());
        self
    }

    /// Add a value override
    pub fn set(mut self, key: &str, value: &str) -> Self {
        self.set_values.push((key.to_string(), value.to_string()));
        self
    }

    /// Add a values file
    pub fn values_file(mut self, path: &str) -> Self {
        self.values_files.push(path.to_string());
        self
    }
}

/// Render a Helm chart to YAML manifests
///
/// Runs `helm template` on the chart and returns the rendered YAML documents.
///
/// # Example
///
/// ```ignore
/// let manifests = helm_template("./charts/myapp", HelmOptions::new("myapp"))?;
/// ```
pub fn helm_template<P: AsRef<Path>>(
    chart_path: P,
    options: HelmOptions,
) -> Result<Vec<String>, ImportError> {
    let chart_path = chart_path.as_ref();

    // Check chart exists
    if !chart_path.exists() {
        return Err(ImportError::ChartNotFound(chart_path.display().to_string()));
    }

    // Build command
    let mut cmd = Command::new("helm");
    cmd.arg("template");

    // Release name
    let release_name = options.release_name.as_deref().unwrap_or("release");
    cmd.arg(release_name);

    // Chart path
    cmd.arg(chart_path);

    // Namespace
    if let Some(ns) = &options.namespace {
        cmd.arg("--namespace").arg(ns);
    }

    // Set values
    for (key, value) in &options.set_values {
        cmd.arg("--set").arg(format!("{}={}", key, value));
    }

    // Values files
    for values_file in &options.values_files {
        cmd.arg("-f").arg(values_file);
    }

    // Run helm
    let output = cmd.output().map_err(|e| {
        if e.kind() == std::io::ErrorKind::NotFound {
            ImportError::HelmNotFound
        } else {
            ImportError::IoError(e)
        }
    })?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(ImportError::HelmFailed(stderr.to_string()));
    }

    // Parse output into separate YAML documents
    let stdout = String::from_utf8_lossy(&output.stdout);
    let documents: Vec<String> = stdout
        .split("---")
        .map(|s| s.trim())
        .filter(|s| !s.is_empty())
        .map(|s| {
            // Remove comment lines (# Source: ...) but keep the YAML content
            s.lines()
                .filter(|line| !line.trim_start().starts_with('#'))
                .collect::<Vec<_>>()
                .join("\n")
                .trim()
                .to_string()
        })
        .filter(|s| !s.is_empty())
        .collect();

    Ok(documents)
}

/// Build Kustomize overlay to YAML manifests
///
/// Runs `kustomize build` on the directory and returns the rendered YAML documents.
///
/// # Example
///
/// ```ignore
/// let manifests = kustomize_build("./k8s/overlays/prod")?;
/// ```
pub fn kustomize_build<P: AsRef<Path>>(kustomize_dir: P) -> Result<Vec<String>, ImportError> {
    let kustomize_dir = kustomize_dir.as_ref();

    // Check directory exists
    if !kustomize_dir.exists() {
        return Err(ImportError::ChartNotFound(
            kustomize_dir.display().to_string(),
        ));
    }

    // Run kustomize build
    let output = Command::new("kustomize")
        .arg("build")
        .arg(kustomize_dir)
        .output()
        .map_err(|e| {
            if e.kind() == std::io::ErrorKind::NotFound {
                ImportError::KustomizeNotFound
            } else {
                ImportError::IoError(e)
            }
        })?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(ImportError::KustomizeFailed(stderr.to_string()));
    }

    // Parse output into separate YAML documents
    let stdout = String::from_utf8_lossy(&output.stdout);
    let documents: Vec<String> = stdout
        .split("---")
        .map(|s| s.trim())
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string())
        .collect();

    Ok(documents)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    // ============================================================
    // RED: Helm import tests
    // ============================================================

    #[test]
    fn test_helm_options_builder() {
        let opts = HelmOptions::new("myapp")
            .namespace("prod")
            .set("replicas", "3")
            .set("image.tag", "v1.0")
            .values_file("values-prod.yaml");

        assert_eq!(opts.release_name, Some("myapp".to_string()));
        assert_eq!(opts.namespace, Some("prod".to_string()));
        assert_eq!(opts.set_values.len(), 2);
        assert_eq!(opts.values_files.len(), 1);
    }

    #[test]
    fn test_helm_template_chart_not_found() {
        let result = helm_template("/nonexistent/chart", HelmOptions::default());
        assert!(matches!(result, Err(ImportError::ChartNotFound(_))));
    }

    #[test]
    fn test_helm_template_renders_chart() {
        // Create a minimal Helm chart
        let temp_dir = TempDir::new().unwrap();
        let chart_dir = temp_dir.path().join("mychart");
        fs::create_dir(&chart_dir).unwrap();

        // Chart.yaml
        fs::write(
            chart_dir.join("Chart.yaml"),
            r#"
apiVersion: v2
name: mychart
version: 0.1.0
"#,
        )
        .unwrap();

        // templates/configmap.yaml
        let templates_dir = chart_dir.join("templates");
        fs::create_dir(&templates_dir).unwrap();
        fs::write(
            templates_dir.join("configmap.yaml"),
            r#"
apiVersion: v1
kind: ConfigMap
metadata:
  name: {{ .Release.Name }}-config
data:
  key: value
"#,
        )
        .unwrap();

        // Render
        let result = helm_template(&chart_dir, HelmOptions::new("myrelease"));

        // Skip if helm is not installed
        match result {
            Err(ImportError::HelmNotFound) => {
                eprintln!("Skipping test: helm not installed");
            }
            Err(e) => panic!("Unexpected error: {}", e),
            Ok(manifests) => {
                assert!(!manifests.is_empty(), "Should render at least one manifest");
                let yaml = &manifests[0];
                assert!(yaml.contains("ConfigMap"), "Should be a ConfigMap");
                assert!(
                    yaml.contains("myrelease-config"),
                    "Should have release name in resource name"
                );
            }
        }
    }

    #[test]
    fn test_helm_template_with_set_values() {
        // Create a minimal Helm chart with values
        let temp_dir = TempDir::new().unwrap();
        let chart_dir = temp_dir.path().join("mychart");
        fs::create_dir(&chart_dir).unwrap();

        // Chart.yaml
        fs::write(
            chart_dir.join("Chart.yaml"),
            r#"
apiVersion: v2
name: mychart
version: 0.1.0
"#,
        )
        .unwrap();

        // values.yaml
        fs::write(
            chart_dir.join("values.yaml"),
            r#"
replicas: 1
"#,
        )
        .unwrap();

        // templates/deployment.yaml
        let templates_dir = chart_dir.join("templates");
        fs::create_dir(&templates_dir).unwrap();
        fs::write(
            templates_dir.join("deployment.yaml"),
            r#"
apiVersion: apps/v1
kind: Deployment
metadata:
  name: {{ .Release.Name }}
spec:
  replicas: {{ .Values.replicas }}
"#,
        )
        .unwrap();

        // Render with override
        let opts = HelmOptions::new("myapp").set("replicas", "5");
        let result = helm_template(&chart_dir, opts);

        match result {
            Err(ImportError::HelmNotFound) => {
                eprintln!("Skipping test: helm not installed");
            }
            Err(e) => panic!("Unexpected error: {}", e),
            Ok(manifests) => {
                assert!(!manifests.is_empty());
                let yaml = &manifests[0];
                assert!(
                    yaml.contains("replicas: 5"),
                    "Should have overridden replicas"
                );
            }
        }
    }

    // ============================================================
    // RED: Kustomize import tests
    // ============================================================

    #[test]
    fn test_kustomize_build_dir_not_found() {
        let result = kustomize_build("/nonexistent/dir");
        assert!(matches!(result, Err(ImportError::ChartNotFound(_))));
    }

    #[test]
    fn test_kustomize_build_renders_overlay() {
        // Create a minimal Kustomize structure
        let temp_dir = TempDir::new().unwrap();
        let base_dir = temp_dir.path();

        // kustomization.yaml
        fs::write(
            base_dir.join("kustomization.yaml"),
            r#"
apiVersion: kustomize.config.k8s.io/v1beta1
kind: Kustomization
resources:
  - configmap.yaml
"#,
        )
        .unwrap();

        // configmap.yaml
        fs::write(
            base_dir.join("configmap.yaml"),
            r#"
apiVersion: v1
kind: ConfigMap
metadata:
  name: my-config
data:
  key: value
"#,
        )
        .unwrap();

        // Build
        let result = kustomize_build(base_dir);

        match result {
            Err(ImportError::KustomizeNotFound) => {
                eprintln!("Skipping test: kustomize not installed");
            }
            Err(e) => panic!("Unexpected error: {}", e),
            Ok(manifests) => {
                assert!(!manifests.is_empty(), "Should render at least one manifest");
                let yaml = &manifests[0];
                assert!(yaml.contains("ConfigMap"), "Should be a ConfigMap");
                assert!(yaml.contains("my-config"), "Should have config name");
            }
        }
    }

    #[test]
    fn test_import_error_display() {
        assert_eq!(
            ImportError::HelmNotFound.to_string(),
            "Helm not found. Install with: brew install helm"
        );
        assert_eq!(
            ImportError::KustomizeNotFound.to_string(),
            "Kustomize not found. Install with: brew install kustomize"
        );
        assert_eq!(
            ImportError::ChartNotFound("/path".to_string()).to_string(),
            "Chart not found: /path"
        );
    }
}
