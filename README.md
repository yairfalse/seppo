# seppo

**Kubernetes SDK for Rust. No YAML. No CLI. Just code.**

```rust
use seppo::Context;

let ctx = Context::new().await?;
ctx.apply(&deployment).await?;
ctx.wait_ready("deployment/myapp").await?;
```

---

## Why

**The problem:** Kubernetes tooling is fragmented. You write YAML, shell out to kubectl, maintain bash scripts, and wrestle with templating engines. Your "infrastructure as code" isn't really code—it's configuration files with code-like aspirations.

**The vision:** What if Kubernetes operations were just... code? Real code. In your language. With types, IDE support, and compile-time checks.

Seppo makes Kubernetes a library call:

```rust
// This is your deployment. No YAML.
let deployment = DeploymentFixture::new("myapp")
    .image("myapp:v1")
    .replicas(3)
    .port(8080)
    .build();

ctx.apply(&deployment).await?;
ctx.wait_ready("deployment/myapp").await?;
```

No YAML. No kubectl. No templating. Just Rust.

---

## What We Want to Achieve

1. **Escape YAML** — Define resources in code, not configuration files
2. **Native SDK** — First-class Rust library, not a CLI wrapper
3. **Full kubectl coverage** — Everything kubectl does, but programmatic
4. **Ecosystem integration** — Import from Helm/Kustomize as migration path, then stay in code
5. **Multi-language** — Same concepts in Rust (Seppo) and Go (Ilmari)

The goal: you should never need to write YAML again.

---

## How It Works

```
┌──────────────────┐     ┌─────────────────┐     ┌──────────────┐
│  Context::new()  │────▶│     Context     │────▶│  Kubernetes  │
│  (your code)     │     │   (namespace)   │     │ (any cluster)│
└──────────────────┘     └─────────────────┘     └──────────────┘
         │                       │
         │                       ├── apply()      → create resources
         │                       ├── get/list()   → read resources
         │                       ├── delete()     → remove resources
         │                       ├── wait_ready() → poll until ready
         │                       ├── port_forward() → port forward
         │                       ├── exec()       → run commands
         │                       └── logs()       → stream logs
```

Each `Context` manages a namespace. Resources are created via `kube-rs`. The optional `#[seppo::test]` macro adds automatic cleanup and failure diagnostics.

---

## Install

```toml
[dependencies]
seppo = "0.1"
```

Requires a Kubernetes cluster (Kind, Minikube, EKS, GKE, etc.)

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

    // Your operations here...

    ctx.cleanup().await?;
    Ok(())
}
```

### With Test Macro

```rust
use seppo::Context;

#[seppo::test]
async fn test_my_app(ctx: Context) {
    ctx.apply(&deployment).await?;
    ctx.wait_ready("deployment/myapp").await?;

    let pf = ctx.port_forward("svc/myapp", 8080).await?;
    assert!(pf.get("/health").await?.contains("ok"));
}
// Cleanup automatic on success, diagnostics on failure
```

### Resource Builders

```rust
use seppo::deployment;

let deploy = deployment("myapp")
    .image("nginx:latest")
    .replicas(3)
    .port(80)
    .env("LOG_LEVEL", "debug")
    .label("tier", "frontend")
    .build();

ctx.apply(&deploy).await?;
```

### Deploy Stacks

```rust
use seppo::stack;

let s = stack()
    .service("frontend")
        .image("fe:v1")
        .replicas(2)
        .port(80)
    .service("backend")
        .image("be:v1")
        .replicas(3)
        .port(8080)
    .service("db")
        .image("postgres:15")
        .port(5432)
    .build();

ctx.up(&s).await?;
```

### Async Conditions

```rust
use seppo::eventually;

eventually(|| async {
    let pods: Vec<Pod> = ctx.list().await?;
    Ok(pods.len() >= 3)
})
.timeout(Duration::from_secs(60))
.await?;
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

─── Events ─────────────────────────────────────────────────────
  • 10:42:00  Pod/myapp  Scheduled  Assigned to node-1
  • 10:42:01  Pod/myapp  BackOff    Image pull error

─── Debug ──────────────────────────────────────────────────────
  kubectl -n seppo-test-a1b2c3d4 get all
  kubectl delete ns seppo-test-a1b2c3d4
━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
```

---

## Features

| Category | Features |
|----------|----------|
| **Core** | `Context`, apply/get/list/delete, wait_ready |
| **Network** | Port forwarding, exec, logs |
| **Builders** | DeploymentFixture, PodFixture, ServiceFixture, Stack |
| **Testing** | `#[seppo::test]`, eventually/consistently, assertions |
| **Diagnostics** | Pod logs, events, failure reports |
| **Providers** | Kind, Minikube, existing clusters |
| **Telemetry** | OpenTelemetry integration |

---

## Roadmap

| Phase | Goal |
|-------|------|
| **0** | Rebrand as SDK, expose `Context::new()` standalone |
| **1** | Complete primitives: patch, watch, copy, diff |
| **2** | Helm/Kustomize import, migration CLI |
| **3** | Operational: scale, rollback, restart, drain |
| **4** | Multi-cluster, impersonation, audit |

---

## Name

*Seppo Ilmarinen* — the eternal hammerer, legendary smith of the Kalevala. He forged the Sampo, the mythical artifact of prosperity. Like Ilmarinen at his forge, Seppo shapes Kubernetes from raw code into running infrastructure.

The Rust SDK is **Seppo**. The Go SDK is **Ilmari**. Same forge, different metals.

---

## License

Apache-2.0
