//! Seppo - Kubernetes Testing SDK
//!
//! A native Rust library for Kubernetes integration testing.
//! No config files, just code.
//!
//! # Example
//!
//! ```no_run
//! use seppo::{ClusterConfig, EnvironmentConfig, WaitCondition, Config, setup};
//!
//! #[tokio::test]
//! async fn integration_test() -> Result<(), Box<dyn std::error::Error>> {
//!     // Define cluster
//!     let cluster = ClusterConfig::kind("my-test")
//!         .workers(2);
//!
//!     // Define environment
//!     let env = EnvironmentConfig::new()
//!         .image("myapp:test")
//!         .manifest("./k8s/deployment.yaml")
//!         .wait(WaitCondition::available("deployment/myapp"));
//!
//!     // Setup and run
//!     let config = Config::new(cluster).environment(env);
//!     setup(&config).await?;
//!
//!     // Your tests here...
//!
//!     Ok(())
//! }
//! ```
//!
//! # Quick Start
//!
//! ```no_run
//! use seppo::cluster;
//!
//! #[tokio::main]
//! async fn main() -> Result<(), Box<dyn std::error::Error>> {
//!     // Create Kind cluster
//!     cluster::create("test").await?;
//!
//!     // Load image
//!     cluster::load_image("test", "myapp:v1").await?;
//!
//!     // Run your tests...
//!
//!     // Cleanup
//!     cluster::delete("test").await?;
//!
//!     Ok(())
//! }
//! ```
//!
//! # Providers
//!
//! - **Kind** (default): Local Docker-based clusters
//! - **Minikube**: Local VM-based clusters
//! - **Existing**: Use pre-existing clusters

pub mod cluster;
pub mod config;
pub mod context;
pub mod diagnostics;
pub mod environment;
pub mod metrics;
pub mod portforward;
pub mod provider;
pub mod runner;
pub mod scenario;
pub mod stack;
pub mod telemetry;

// Re-export commonly used types
pub use cluster::{create, delete, load_image};
pub use config::{ClusterConfig, ClusterProviderType, Config, EnvironmentConfig, WaitCondition};
pub use context::{
    parse_forward_target, parse_resource_ref, ContextError, ForwardTarget, ResourceKind,
    TestContext,
};
pub use diagnostics::Diagnostics;
pub use environment::{setup, EnvironmentError, SetupResult};
pub use metrics::{metrics, SeppoMetrics};
pub use portforward::{PortForward, PortForwardError};
pub use provider::{
    get_provider, ClusterProvider, ExistingProvider, KindProvider, MinikubeProvider, ProviderError,
};
pub use runner::{run, run_with_env, RunResult, RunnerError};
pub use scenario::{Scenario, ScenarioError, Steps};
pub use stack::{ServiceBuilder, Stack, StackError};
pub use telemetry::{init_telemetry, TelemetryConfig, TelemetryError, TelemetryGuard};

// Re-export proc macros
pub use seppo_macros::test;
