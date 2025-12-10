# Seppo: Kubernetes SDK

**Native library. No config files. Just code.**

---

## WHAT IS SEPPO

Seppo connects your code to Kubernetes. That's it.

```rust
// Standalone usage
use seppo::Context;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let ctx = Context::new().await?;

    ctx.apply(&my_deployment).await?;
    ctx.wait_ready("deployment/myapp").await?;

    let pf = ctx.forward_to("svc/myapp", 8080).await?;
    let resp = pf.get("/health").await?;

    ctx.cleanup().await?;
    Ok(())
}
```

```rust
// With test macro
#[seppo::test]
async fn test_my_app(ctx: Context) {
    ctx.apply(&my_deployment).await?;
    ctx.wait_ready("deployment/myapp").await?;

    let resp = ctx.forward_to("svc/myapp", 8080).await?.get("/health").await?;
    assert!(resp.contains("ok"));
}
```

Like `#[tokio::test]` gives you async, `#[seppo::test]` gives you Kubernetes.

---

## CORE PHILOSOPHY

1. **Native library** - Not a CLI, not a framework. A library.
2. **No config files** - No YAML, no TOML. Just Rust code.
3. **Works standalone or with tests** - Use however you want.
4. **kube-rs powered** - Native K8s client, not kubectl shelling.
5. **Failure-friendly** - Keep resources on failure, dump logs, debug easily.

---

## ARCHITECTURE

```
┌─────────────────────────────────────────────────────────────┐
│  Your Code                                                  │
│    └── Context::new()  → standalone usage                   │
│    └── #[seppo::test]  → test macro (optional)              │
└─────────────────────────────────────────────────────────────┘

┌─────────────────────────────────────────────────────────────┐
│  Your Machine / CI                    Kubernetes Cluster    │
│  ┌─────────────────┐                 ┌─────────────────┐   │
│  │ your app        │ ───kube-rs───► │ Kind / EKS / etc│   │
│  │ (seppo)         │                 │                 │   │
│  └─────────────────┘                 └─────────────────┘   │
└─────────────────────────────────────────────────────────────┘
```

---

## CORE COMPONENTS

### 1. `Context` - K8s Connection

```rust
pub struct Context {
    pub client: kube::Client,
    pub namespace: String,
}

impl Context {
    // Creation
    pub async fn new() -> Result<Self>;  // Creates isolated namespace
    pub async fn cleanup(&self) -> Result<()>;

    // Resource operations
    pub async fn apply<K: Resource>(&self, resource: &K) -> Result<K>;
    pub async fn get<K: Resource>(&self, name: &str) -> Result<K>;
    pub async fn list<K: Resource>(&self) -> Result<Vec<K>>;
    pub async fn delete<K: Resource>(&self, name: &str) -> Result<()>;

    // Waiting
    pub async fn wait_ready(&self, resource: &str) -> Result<()>;
    pub async fn wait_for<K: Resource>(&self, name: &str, condition: F) -> Result<K>;

    // Diagnostics
    pub async fn logs(&self, pod: &str) -> Result<String>;
    pub async fn events(&self) -> Result<Vec<Event>>;

    // Network
    pub async fn forward(&self, pod: &str, port: u16) -> Result<PortForward, PortForwardError>;
    pub async fn forward_to(&self, target: &str, port: u16) -> Result<PortForward, PortForwardError>;
    pub async fn exec(&self, pod: &str, cmd: &[&str]) -> Result<String>;

    // Stack deployment
    pub async fn up(&self, stack: &Stack) -> Result<()>;
}
```

### 2. `#[seppo::test]` - Test Macro (Optional)

```rust
#[seppo::test]
async fn test_my_controller(ctx: Context) {
    // ctx is auto-injected with:
    // - kube::Client
    // - isolated namespace
    // - cleanup on success / diagnostics on failure
}
```

### 3. `Stack` - Environment Builder

```rust
let stack = Stack::new()
    .service("frontend")
        .image("fe:test")
        .replicas(4)
        .port(80)
    .service("backend")
        .image("be:test")
        .replicas(2)
        .port(8080)
    .build();

ctx.up(&stack).await?;
```

### 4. Fixtures - Resource Builders

```rust
let deployment = DeploymentFixture::new("myapp")
    .image("nginx:latest")
    .replicas(3)
    .port(80)
    .env("LOG_LEVEL", "debug")
    .build();

ctx.apply(&deployment).await?;
```

### 5. Failure Handling (with test macro)

On test **success**:
- Cleanup namespace
- Delete resources

On test **failure**:
- Keep namespace alive
- Dump pod logs to test output
- Dump events to test output
- Print resource state
- User can debug

```
FAILED: test_my_controller

Namespace: seppo-test-a1b2c3 (kept for debugging)

--- Pod logs (myapp-xyz) ---
Error: connection refused to database:5432

--- Events ---
0:01 Pod scheduled
0:02 Pulling image myapp:test
0:05 BackOff pulling image

--- Resources ---
deployment/myapp  0/1 ready
pod/myapp-xyz     ImagePullBackOff
```

Configurable:
```bash
SEPPO_KEEP_ALL=true          # never cleanup, even on success (debug mode)
```

---

## CURRENT STATE

**Core SDK:**
- `Context` - K8s connection with namespace management
- `#[seppo::test]` - Optional test macro
- `Stack` - Multi-service deployment builder
- `DeploymentFixture`, `PodFixture`, `ServiceFixture` - Resource builders
- `eventually`, `consistently` - Async condition helpers
- Port forwarding and exec
- Diagnostics collection
- OpenTelemetry integration

---

## LANGUAGES

| Language | Crate/Module | K8s Client |
|----------|--------------|------------|
| Rust | `seppo` | kube-rs |
| Go | `ilmari` | client-go |

Same concepts, native implementation.

---

## TDD WORKFLOW

```bash
# RED: Write failing test
cargo test test_context_creates_namespace  # FAILS

# GREEN: Minimal implementation
cargo test test_context_creates_namespace  # PASSES

# REFACTOR: Clean up
cargo test  # ALL PASS

# COMMIT
git commit -m "feat: Context creates isolated namespace"
```

**Small commits. Always green.**

---

## CRATE STRUCTURE

```
seppo/
├── Cargo.toml
├── src/
│   ├── lib.rs
│   ├── context.rs          # Context - main entry point
│   ├── config.rs           # Cluster/Environment config
│   ├── stack.rs            # Stack builder
│   ├── fixtures.rs         # Resource builders
│   ├── assertions.rs       # Semantic assertions
│   ├── eventually.rs       # Async condition helpers
│   ├── diagnostics.rs      # Failure diagnostics
│   ├── portforward.rs      # Port forwarding
│   ├── wait.rs             # Rich wait errors
│   ├── provider/           # Cluster providers
│   ├── telemetry.rs
│   └── metrics.rs
├── seppo-macros/           # Proc macro crate
│   ├── Cargo.toml
│   └── src/lib.rs
└── tests/
    └── macro_test.rs
```

---

## WHAT SEPPO IS NOT

- **Not a CLI** - No `seppo` command
- **Not a test runner** - Works with `cargo test` or standalone
- **Not config-driven** - No YAML/TOML files
- **Not kubectl wrapper** - Uses kube-rs directly

---

## SUCCESS CRITERIA

```rust
// Standalone
let ctx = Context::new().await?;
ctx.apply(&my_deployment).await?;
ctx.wait_ready("deployment/myapp").await?;
ctx.cleanup().await?;

// Or with test macro
#[seppo::test]
async fn test_my_app(ctx: Context) {
    ctx.apply(&my_deployment).await?;
    ctx.wait_ready("deployment/myapp").await?;
}
```

```bash
cargo test  # Just works
```

---

**Connect to K8s. Just code. That's Seppo.**
