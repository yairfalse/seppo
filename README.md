# seppo

**Kubernetes testing in Rust. No YAML. No CLI. Just code.**

```rust
#[seppo::test]
async fn test_my_app(ctx: TestContext) {
    // Deploy
    ctx.apply(&my_deployment).await?;
    ctx.wait_ready("deployment/myapp").await?;

    // Test
    let pf = ctx.forward_to("svc/myapp", 8080).await?;
    let resp = pf.get("/health").await?;
    assert!(resp.contains("ok"));
}
```

Run `cargo test`. Done.

---

## Why

Kubernetes testing tooling today means YAML fixtures, bash scripts, and complex test frameworks that don't integrate with your language. You end up with a parallel testing infrastructure that's harder to maintain than the code it tests.

Seppo is a native Rust library. Your tests are Rust code. They run with `cargo test`. No external tools, no configuration languages, no impedance mismatch.

- **No YAML** — define resources in Rust
- **No CLI** — library, not a tool
- **No framework** — works with `cargo test`
- **Native kube-rs** — not kubectl shelling

---

## How It Works

```
┌──────────────────┐     ┌─────────────────┐     ┌──────────────┐
│  #[seppo::test]  │────▶│   TestContext   │────▶│  Kubernetes  │
│   your test fn   │     │   (namespace)   │     │   (Kind/EKS) │
└──────────────────┘     └─────────────────┘     └──────────────┘
         │                       │
         │                       ├── apply()      → create resources
         │                       ├── wait_ready() → poll until ready
         │                       ├── forward_to() → port forward
         │                       ├── exec()       → run commands
         │                       └── up()         → deploy stacks
         │
         └── on success: cleanup namespace
             on failure: keep namespace + dump diagnostics
```

Each test gets an isolated namespace. Resources are created with `kube-rs`. On failure, Seppo dumps pod logs and events, then keeps the namespace for debugging.

---

## Install

```toml
[dev-dependencies]
seppo = "0.1"
```

Requires: Docker + [Kind](https://kind.sigs.k8s.io/) (or existing cluster)

---

## Usage

### Basic Test

```rust
use seppo::TestContext;
use k8s_openapi::api::core::v1::ConfigMap;

#[seppo::test]
async fn test_configmap(ctx: TestContext) {
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
async fn test_deployment(ctx: TestContext) {
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
async fn test_api(ctx: TestContext) {
    ctx.apply(&deployment).await?;
    ctx.wait_ready("deployment/myapp").await?;

    // Forward to service (finds backing pod automatically)
    let pf = ctx.forward_to("svc/myapp", 8080).await?;

    let health = pf.get("/health").await?;
    assert!(health.contains("ok"));
}
```

### Deploy Stacks

```rust
use seppo::Stack;

#[seppo::test]
async fn test_full_stack(ctx: TestContext) {
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
async fn test_exec(ctx: TestContext) {
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
| `#[seppo::test]` macro | Done |
| Isolated namespaces | Done |
| apply/get/delete/list | Done |
| wait_ready() | Done |
| forward_to() (svc/deploy/pod) | Done |
| exec() | Done |
| Stack builder | Done |
| Failure diagnostics | Done |
| OpenTelemetry integration | Done |
| Kind provider | Done |
| Minikube provider | Done |
| Existing cluster provider | Done |

---

## Architecture

```
┌─────────────────────────────────────────────────────────────────┐
│  cargo test                                                     │
│    └── #[tokio::test]  → async runtime                          │
│    └── #[sqlx::test]   → database connection                    │
│    └── #[seppo::test]  → kubernetes cluster                     │
└─────────────────────────────────────────────────────────────────┘

┌─────────────────────────────────────────────────────────────────┐
│  Your Machine / CI                      Kubernetes Cluster      │
│  ┌──────────────────┐                  ┌──────────────────┐    │
│  │ cargo test       │ ───kube-rs────▶  │ Kind / EKS / GKE │    │
│  │ (seppo)          │                  │                  │    │
│  └──────────────────┘                  └──────────────────┘    │
└─────────────────────────────────────────────────────────────────┘
```

---

## Name

*Seppo* — Finnish name. The everyman. Like your tests: reliable, unpretentious, gets the job done.

---

## License

Apache-2.0
