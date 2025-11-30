# Seppo

Kubernetes test orchestrator. Create clusters, setup environments, run tests.

## What it does

```
seppo.yaml -> Cluster -> Environment -> Tests -> Results
```

1. **Cluster**: Create Kind/Minikube or use existing
2. **Environment**: Load images, apply manifests, wait for readiness
3. **Tests**: Run your test commands
4. **Results**: Pass/fail with timing

## Quick Start

### Rust Library

```toml
[dev-dependencies]
seppo = "0.1"
tokio = { version = "1", features = ["full"] }
```

```rust
use seppo::{Config, setup, run};

#[tokio::test]
async fn integration_test() -> Result<(), Box<dyn std::error::Error>> {
    // Load config
    let config: Config = std::fs::read_to_string("seppo.yaml")?.parse()?;

    // Setup cluster + environment
    setup(&config).await?;

    // Run tests
    let result = run("cargo", &["test", "--", "--ignored"]).await?;

    assert!(result.passed());
    Ok(())
}
```

### Configuration (seppo.yaml)

```yaml
cluster:
  name: my-test
  provider: kind        # kind | minikube | existing
  workers: 2

environment:
  images:
    - myapp:test
    - sidecar:latest
  manifests:
    - ./k8s/namespace.yaml
    - ./k8s/deployment.yaml
  wait:
    - condition: available
      resource: deployment/myapp
      namespace: default
      timeout: 120s
  setup_script: ./scripts/setup.sh
```

## Providers

| Provider | Use Case |
|----------|----------|
| `kind` | Local dev, CI (Docker-based) |
| `minikube` | Local dev with VM drivers |
| `existing` | Use your own cluster |

### Kind (default)

```yaml
cluster:
  name: test
  provider: kind
  workers: 2
  k8s_version: "1.31.0"  # optional
```

### Minikube

```yaml
cluster:
  name: test
  provider: minikube
  driver: docker  # or hyperkit, virtualbox
```

### Existing Cluster

```yaml
cluster:
  name: my-cluster
  provider: existing
  kubeconfig: ~/.kube/config
  context: my-context
```

## API

### Cluster Operations

```rust
use seppo::cluster;

// Simple API (Kind only)
cluster::create("test").await?;
cluster::load_image("test", "myapp:v1").await?;
cluster::delete("test").await?;
```

### Multi-Provider

```rust
use seppo::{Config, get_provider};

let config: Config = yaml.parse()?;
let provider = get_provider(&config.cluster)?;

provider.create(&config.cluster).await?;
provider.load_image("test", "myapp:v1").await?;
provider.delete("test").await?;
```

### Environment Setup

```rust
use seppo::{Config, setup};

let config: Config = yaml.parse()?;
let result = setup(&config).await?;

println!("Images loaded: {:?}", result.images_loaded);
println!("Manifests applied: {:?}", result.manifests_applied);
```

### Test Runner

```rust
use seppo::{run, run_with_env};

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
