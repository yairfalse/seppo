//! Seppo - Kubernetes Testing Framework
//!
//! Seppo provides cluster management and testing utilities for Kubernetes controllers
//! and applications. Supports multiple cluster types (Kind, EKS, AKS, GKE) with a
//! unified interface.
//!
//! # Example (Rust)
//!
//! ```no_run
//! use seppo::cluster;
//!
//! #[tokio::main]
//! async fn main() -> Result<(), Box<dyn std::error::Error>> {
//!     // Create Kind cluster
//!     cluster::create("test-cluster").await?;
//!
//!     println!("Cluster created!");
//!
//!     // Your tests here...
//!
//!     // Cleanup
//!     cluster::delete("test-cluster").await?;
//!
//!     Ok(())
//! }
//! ```
//!
//! # CLI Usage
//!
//! ```bash
//! # Create cluster
//! seppo cluster create --type kind --name test --nodes 2
//!
//! # Delete cluster
//! seppo cluster delete --name test
//! ```

pub mod cluster;
pub mod config;
pub mod environment;
pub mod metrics;
pub mod provider;
pub mod runner;
pub mod telemetry;

// Re-export commonly used types
pub use cluster::{create, delete, load_image};
pub use config::{Config, ClusterConfig, ClusterProviderType, EnvironmentConfig, WaitCondition, ConfigError};
pub use environment::{setup, SetupResult, EnvironmentError};
pub use provider::{ClusterProvider, ProviderError, get_provider, KindProvider, MinikubeProvider, ExistingProvider};
pub use runner::{run, run_with_env, RunResult, RunnerError};
pub use telemetry::{init_telemetry, TelemetryConfig, TelemetryError, TelemetryGuard};
pub use metrics::{metrics, SeppoMetrics};
