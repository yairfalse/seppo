# seppo

**Kubernetes SDK for Rust. No YAML. No CLI. Just code.**

```rust
use seppo::Context;

// Standalone
let ctx = Context::new().await?;
ctx.apply(&deployment).await?;
ctx.wait_ready("deployment/myapp").await?;

// Or with test macro
#[seppo::test]
async fn test_my_app(ctx: Context) {
    ctx.apply(&deployment).await?;
    ctx.wait_ready("deployment/myapp").await?;

    let pf = ctx.forward_to("svc/myapp", 8080).await?;
    assert!(pf.get("/health").await?.contains("ok"));
}
```

---

## Why

Kubernetes tooling today means YAML, bash scripts, and complex frameworks that don't integrate with your language. You end up with a parallel infrastructure that's harder to maintain than the code it operates.

Seppo is a native Rust library. Your K8s operations are Rust code. No external tools, no configuration languages, no impedance mismatch.

- **No YAML** — define resources in Rust
- **No CLI** — library, not a tool
- **Native kube-rs** — not kubectl shelling
- **Works standalone or with tests**

---

## How It Works

```
┌──────────────────┐     ┌─────────────────┐     ┌──────────────┐
│  Context::new()  │────▶│     Context     │────▶│  Kubernetes  │
│  or #[seppo::test]│     │   (namespace)   │     │   (Kind/EKS) │
└──────────────────┘     └─────────────────┘     └──────────────┘
         │                       │
         │                       ├── apply()      → create resources
         │                       ├── wait_ready() → poll until ready
         │                       ├── forward_to() → port forward
         │                       ├── exec()       → run commands
         │                       └── up()         → deploy stacks
         │
         └── cleanup() when done
             (test macro: auto-cleanup on success, keep on failure)
```

Each `Context` gets an isolated namespace. Resources are created with `kube-rs`. The `#[seppo::test]` macro handles cleanup automatically.

---

## Install

```toml
[dependencies]
seppo = "0.1"
```

Requires: Docker + [Kind](https://kind.sigs.k8s.io/) (or existing cluster)

---

## Usage

### Standalone

```rust
use seppo::Context;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let ctx = Context::new().await?;

    ctx.apply(&my_deployment).await?;
    ctx.wait_ready("deployment/myapp").await?;

    // Do stuff...

    ctx.cleanup().await?;
    Ok(())
}
```

### With Test Macro

```rust
use seppo::Context;
use k8s_openapi::api::core::v1::ConfigMap;

#[seppo::test]
async fn test_configmap(ctx: Context) {
    let cm = ConfigMap {
        metadata: kube::api::ObjectMeta {
            name: Some("myconfig".into()),
            ..Default::default()
        },
        data: Some([("key".into(), "value".into())].into()),
        ..Default::default()
    };

    ctx.apply(&cm).await.unwrap();

    let fetched: ConfigMap = ctx.get("myconfig").await.unwrap();
    assert_eq!(fetched.data.unwrap()["key"], "value");
}
```

### Wait for Resources

```rust
#[seppo::test]
async fn test_deployment(ctx: Context) {
    ctx.apply(&deployment).await?;

    // Wait until ready (understands K8s semantics)
    ctx.wait_ready("deployment/myapp").await?;
    ctx.wait_ready("pod/worker-0").await?;
    ctx.wait_ready("svc/backend").await?;
}
```

### Port Forward & HTTP

```rust
#[seppo::test]
async fn test_api(ctx: Context) {
    ctx.apply(&deployment).await?;
    ctx.wait_ready("deployment/myapp").await?;

    // Forward to service (finds backing pod automatically)
    let pf = ctx.forward_to("svc/myapp", 8080).await?;

    let health = pf.get("/health").await?;
    assert!(health.contains("ok"));
}
```

### Resource Builders (No YAML)

```rust
use seppo::DeploymentFixture;

let deployment = DeploymentFixture::new("myapp")
    .image("nginx:latest")
    .replicas(3)
    .port(80)
    .env("LOG_LEVEL", "debug")
    .build();

ctx.apply(&deployment).await?;
```

### Deploy Stacks

```rust
use seppo::Stack;

#[seppo::test]
async fn test_full_stack(ctx: Context) {
    let stack = Stack::new()
        .service("frontend")
            .image("fe:test")
            .replicas(2)
            .port(80)
        .service("backend")
            .image("be:test")
            .replicas(1)
            .port(8080)
        .service("worker")
            .image("worker:test")
            .replicas(3)
        .build();

    ctx.up(&stack).await?;

    // All services deployed concurrently
}
```

### Exec Commands

```rust
#[seppo::test]
async fn test_exec(ctx: Context) {
    ctx.apply(&pod).await?;
    ctx.wait_ready("pod/myapp").await?;

    let output = ctx.exec("myapp", &["cat", "/etc/config"]).await?;
    assert!(output.contains("expected"));
}
```

---

## Failure Diagnostics

When tests fail, Seppo keeps the namespace and dumps diagnostics:

```
━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
  SEPPO TEST FAILED
━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━

  Namespace: seppo-test-a1b2c3d4 (kept for debugging)

─── Pod Logs ───────────────────────────────────────────────────
[myapp-xyz123]
  ERROR: Connection refused to postgres:5432

─── Events (2) ─────────────────────────────────────────────────
  • 10:42:00  Pod/myapp  Scheduled  Assigned to node-1
  • 10:42:01  Pod/myapp  BackOff    Image pull error

─── Debug ──────────────────────────────────────────────────────
  kubectl -n seppo-test-a1b2c3d4 get all
  kubectl delete ns seppo-test-a1b2c3d4  # cleanup
━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
```

Set `SEPPO_KEEP_ALL=true` to keep namespaces even on success.

---

## Features

| Feature | Status |
|---------|--------|
| `Context` (standalone) | Done |
| `#[seppo::test]` macro | Done |
| Isolated namespaces | Done |
| apply/get/delete/list | Done |
| wait_ready() | Done |
| forward_to() (svc/deploy/pod) | Done |
| exec() | Done |
| Stack builder | Done |
| DeploymentFixture/PodFixture/ServiceFixture | Done |
| eventually/consistently helpers | Done |
| Failure diagnostics | Done |
| OpenTelemetry integration | Done |
| Kind provider | Done |
| Minikube provider | Done |
| Existing cluster provider | Done |

---

## Name

*Seppo* — Finnish name. The everyman. Like your code: reliable, unpretentious, gets the job done.

---

## License

Apache-2.0
