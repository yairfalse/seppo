//! Seppo - Kubernetes SDK
//!
//! A native Rust library for Kubernetes operations.
//! Connects via your kubeconfig — no config files, just code.
//!
//! # Standalone Usage
//!
//! ```ignore
//! use seppo::Context;
//!
//! #[tokio::main]
//! async fn main() -> Result<(), Box<dyn std::error::Error>> {
//!     let ctx = Context::new().await?;
//!
//!     ctx.apply(&my_deployment).await?;
//!     ctx.wait_ready("deployment/myapp").await?;
//!     ctx.cleanup().await?;
//!     Ok(())
//! }
//! ```
//!
//! # With Test Macro
//!
//! ```ignore
//! #[seppo::test]
//! async fn test_my_app(ctx: seppo::Context) {
//!     ctx.apply(&my_deployment).await?;
//!     ctx.wait_ready("deployment/myapp").await?;
//! }
//! ```

pub mod assertions;
pub mod context;
pub mod diagnostics;
pub mod eventually;
pub mod fixtures;
pub mod metrics;
pub mod portforward;
pub mod runner;
pub mod scenario;
pub mod stack;
pub mod telemetry;
pub mod traffic;
pub mod wait;

// Re-export commonly used types
pub use assertions::{
    AssertionError, DeploymentAssertion, PodAssertion, PvcAssertion, ServiceAssertion,
};
pub use context::{
    parse_forward_target, parse_resource_ref, Context, ContextError, ForwardTarget, ResourceKind,
};

// Backward compatibility alias
#[allow(deprecated)]
pub use context::TestContext;
pub use diagnostics::Diagnostics;
pub use eventually::{consistently, eventually, ConditionError, Consistently, Eventually};
pub use fixtures::{deployment, pod, service, DeploymentFixture, PodFixture, ServiceFixture};
pub use metrics::{MetricsError, PodMetrics};
pub use portforward::{PortForward, PortForwardError};
pub use runner::{run, run_with_env, RunResult, RunnerError};
pub use scenario::{Scenario, ScenarioError, Steps};
pub use stack::{stack, ServiceBuilder, Stack, StackError};
pub use telemetry::init_logging;
pub use traffic::{HttpAssert, RequestRecord, TrafficError, TrafficRecorder};
pub use wait::{ResourceState, WaitError, WaitEvent};

// Re-export watcher types for watch() methods
pub use kube::runtime::watcher::Error as WatcherError;
pub use kube::runtime::watcher::Event as WatchEvent;

// Re-export proc macros
pub use seppo_macros::test;
