# Seppo

Kubernetes testing SDK. Native library, no config files.

## What it does

```
Code -> Cluster -> Environment -> Tests -> Results
```

1. **Cluster**: Create Kind/Minikube or use existing
2. **Environment**: Load images, apply manifests, wait for readiness
3. **Tests**: Run your test commands
4. **Results**: Pass/fail with timing

## Quick Start

```toml
[dev-dependencies]
seppo = "0.1"
tokio = { version = "1", features = ["full"] }
```

```rust
use seppo::{ClusterConfig, EnvironmentConfig, WaitCondition, Config, setup};

#[tokio::test]
async fn integration_test() -> Result<(), Box<dyn std::error::Error>> {
    // Define cluster
    let cluster = ClusterConfig::kind("my-test")
        .workers(2);

    // Define environment
    let env = EnvironmentConfig::new()
        .image("myapp:test")
        .manifest("./k8s/deployment.yaml")
        .wait(WaitCondition::available("deployment/myapp"));

    // Setup
    let config = Config::new(cluster).environment(env);
    setup(&config).await?;

    // Run tests
    let result = seppo::run("cargo", &["test", "--", "--ignored"]).await?;
    assert!(result.passed());

    Ok(())
}
```

## Providers

| Provider | Use Case |
|----------|----------|
| `kind` | Local dev, CI (Docker-based) |
| `minikube` | Local dev with VM drivers |
| `existing` | Use your own cluster |

### Kind (default)

```rust
let cluster = ClusterConfig::kind("test")
    .workers(2)
    .k8s_version("1.31.0");
```

### Minikube

```rust
let cluster = ClusterConfig::minikube("test")
    .driver("docker");  // or hyperkit, virtualbox
```

### Existing Cluster

```rust
let cluster = ClusterConfig::existing("my-cluster")
    .kubeconfig("~/.kube/config")
    .context("my-context");
```

## API

### Simple API (Kind only)

```rust
use seppo::cluster;

cluster::create("test").await?;
cluster::load_image("test", "myapp:v1").await?;
cluster::delete("test").await?;
```

### Multi-Provider

```rust
use seppo::{ClusterConfig, get_provider};

let config = ClusterConfig::kind("test").workers(2);
let provider = get_provider(&config)?;

provider.create(&config).await?;
provider.load_image("test", "myapp:v1").await?;
provider.delete("test").await?;
```

### Environment Setup

```rust
use seppo::{ClusterConfig, EnvironmentConfig, WaitCondition, Config, setup};

let config = Config::new(ClusterConfig::kind("test"))
    .environment(
        EnvironmentConfig::new()
            .image("myapp:test")
            .manifest("./k8s/namespace.yaml")
            .manifest("./k8s/deployment.yaml")
            .wait(
                WaitCondition::available("deployment/myapp")
                    .namespace("test")
                    .timeout_secs(120)
            )
            .setup_script("./scripts/setup.sh")
    );

let result = setup(&config).await?;
println!("Images loaded: {:?}", result.images_loaded);
println!("Manifests applied: {:?}", result.manifests_applied);
```

### Test Runner

```rust
use seppo::{run, run_with_env};
use std::collections::HashMap;

// Run command
let result = run("pytest", &["-v", "tests/"]).await?;
println!("Exit code: {}", result.exit_code);
println!("Stdout: {}", result.stdout);

// With environment variables
let mut env = HashMap::new();
env.insert("KUBECONFIG".into(), "/tmp/kubeconfig".into());
let result = run_with_env("go", &["test", "./..."], &env).await?;
```

## Telemetry

Seppo exports OpenTelemetry traces and metrics.

```rust
use seppo::{TelemetryConfig, init_telemetry};

let config = TelemetryConfig::from_env();
let _guard = init_telemetry(&config)?;

// Spans and metrics exported to OTLP endpoint
```

### Environment Variables

```bash
OTEL_EXPORTER_OTLP_ENDPOINT=http://localhost:4317
SEPPO_SERVICE_NAME=seppo
SEPPO_TELEMETRY_ENABLED=true
```

### Metrics

- `seppo_clusters_created_total`
- `seppo_tests_executed_total`
- `seppo_tests_passed_total`
- `seppo_tests_failed_total`
- `seppo_test_duration_ms`

## Requirements

- Rust 1.70+
- Docker (for Kind)
- `kind` CLI: https://kind.sigs.k8s.io/
- `minikube` CLI (optional): https://minikube.sigs.k8s.io/
- `kubectl`: https://kubernetes.io/docs/tasks/tools/

## Development

```bash
# Run tests
cargo test

# Run with real cluster (slow)
cargo test -- --ignored

# Lint
cargo clippy --all-targets
```

## License

Apache-2.0
