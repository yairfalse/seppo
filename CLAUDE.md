# Seppo: Kubernetes Testing SDK

**Native library. No config files. Just code.**

---

## WHAT IS SEPPO

Seppo connects your tests to Kubernetes. That's it.

```rust
#[seppo::test]
async fn test_my_app(ctx: TestContext) {
    ctx.apply(my_deployment).await?;
    ctx.wait_ready("deployment/myapp").await?;

    let resp = ctx.forward("svc/myapp", 8080).get("/health").await?;
    assert_eq!(resp.status(), 200);
}
```

```bash
cargo test
```

Like `#[tokio::test]` gives you async, `#[seppo::test]` gives you Kubernetes.

---

## CORE PHILOSOPHY

1. **Native library** - Not a CLI, not a framework. A library.
2. **No config files** - No YAML, no TOML. Just Rust code.
3. **Works with cargo test** - Not a test runner. Enhances existing tests.
4. **kube-rs powered** - Native K8s client, not kubectl shelling.
5. **Failure-friendly** - Keep resources on failure, dump logs, debug easily.

---

## ARCHITECTURE

```
┌─────────────────────────────────────────────────────────────┐
│  cargo test                                                 │
│    └── #[tokio::test]  → async runtime                      │
│    └── #[sqlx::test]   → database connection                │
│    └── #[seppo::test]  → kubernetes cluster                 │
└─────────────────────────────────────────────────────────────┘

┌─────────────────────────────────────────────────────────────┐
│  Your Machine / CI                    Kubernetes Cluster    │
│  ┌─────────────────┐                 ┌─────────────────┐   │
│  │ cargo test      │ ───kube-rs───► │ Kind / EKS / etc│   │
│  │ (seppo)         │                 │                 │   │
│  └─────────────────┘                 └─────────────────┘   │
└─────────────────────────────────────────────────────────────┘
```

---

## CORE COMPONENTS

### 1. `#[seppo::test]` - Test Macro

```rust
#[seppo::test]
async fn test_my_controller(ctx: TestContext) {
    // ctx is auto-injected with:
    // - kube::Client
    // - isolated namespace
    // - cleanup on success / diagnostics on failure
}
```

### 2. `TestContext` - K8s Connection

```rust
pub struct TestContext {
    pub client: kube::Client,
    pub namespace: String,
}

impl TestContext {
    // Resource operations
    pub async fn apply<K: Resource>(&self, resource: &K) -> Result<K>;
    pub async fn get<K: Resource>(&self, name: &str) -> Result<K>;
    pub async fn list<K: Resource>(&self) -> Result<Vec<K>>;
    pub async fn delete<K: Resource>(&self, name: &str) -> Result<()>;

    // Waiting
    pub async fn wait_ready(&self, resource: &str) -> Result<()>;
    pub async fn wait_for<K: Resource>(&self, name: &str) -> WaitBuilder<K>;

    // Diagnostics
    pub async fn logs(&self, pod: &str) -> Result<String>;
    pub async fn events(&self) -> Result<Vec<Event>>;

    // Network
    pub async fn forward(&self, svc: &str, port: u16) -> PortForward;
    pub async fn exec(&self, pod: &str, cmd: &[&str]) -> Result<String>;
}
```

### 3. `Stack` - Environment Builder

```rust
let stack = Stack::new()
    .image("myapp:test")
    .service("frontend")
        .image("fe:test")
        .replicas(4)
        .port(80)
    .service("backend")
        .image("be:test")
        .replicas(2)
        .port(8080)
    .service("database")
        .image("postgres:15")
        .port(5432);

#[seppo::test]
async fn test_full_stack(ctx: TestContext) {
    ctx.up(stack).await?;
    // All services running
}
```

### 4. Failure Handling

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

Note: Namespace is always kept on failure for debugging.

---

## CURRENT STATE (Phase 1 - Done)

**Infrastructure layer:**
- `ClusterConfig` - Cluster configuration with builders
- `EnvironmentConfig` - Environment configuration with builders
- `ClusterProvider` trait - Kind, Minikube, Existing
- `setup()` - Environment setup (images, manifests, wait)
- `run()` - Command execution
- OpenTelemetry integration

**What's missing (Phase 2):**
- `#[seppo::test]` macro
- `TestContext` with kube-rs
- Failure diagnostics
- Port forwarding
- Stack builder

---

## PHASE 2 PLAN (Current Work)

### Milestone 1: TestContext

```rust
pub struct TestContext {
    client: kube::Client,
    namespace: String,
}

impl TestContext {
    pub async fn new() -> Result<Self>;
    pub async fn apply<K>(&self, resource: &K) -> Result<K>;
    pub async fn get<K>(&self, name: &str) -> Result<K>;
    pub async fn logs(&self, pod: &str) -> Result<String>;
    pub async fn cleanup(&self) -> Result<()>;
}
```

### Milestone 2: Test Macro (seppo-macros crate)

```rust
#[proc_macro_attribute]
pub fn test(attr: TokenStream, item: TokenStream) -> TokenStream {
    // 1. Create namespace
    // 2. Create TestContext
    // 3. Inject into test function
    // 4. Handle cleanup/failure
}
```

### Milestone 3: Failure Diagnostics

On panic/error:
1. Collect pod logs
2. Collect events
3. Describe resources
4. Print to test output
5. Keep namespace (configurable)

### Milestone 4: Port Forwarding & Exec

```rust
let fwd = ctx.forward("svc/myapp", 8080).await?;
let resp = fwd.get("/health").await?;

let output = ctx.exec("pod/myapp", &["cat", "/etc/config"]).await?;
```

### Milestone 5: Stack Builder

```rust
let stack = Stack::new()
    .service("frontend").image("fe:test").replicas(4)
    .service("backend").image("be:test").replicas(2);

ctx.up(stack).await?;
```

---

## LANGUAGES

| Language | Crate/Module | K8s Client |
|----------|--------------|------------|
| Rust | `seppo` | kube-rs |
| Go | `seppo-go` | client-go |

Same concepts, native implementation.

---

## FIRST USER: RAUTA

Rauta is a Kubernetes Gateway API controller in Rust.

Rauta's current test framework:
```
rauta/tests/integration/framework/
├── mod.rs        → TestContext
├── cluster.rs    → Kind management
├── fixtures.rs   → YAML templates
├── assertions.rs → Gateway assertions
```

**Seppo = Rauta's test framework, generalized.**

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
git commit -m "feat: TestContext creates isolated namespace"
```

**Small commits. Always green.**

---

## CRATE STRUCTURE

```
seppo/
├── Cargo.toml
├── src/
│   ├── lib.rs
│   ├── config.rs           # Builders
│   ├── context.rs          # TestContext (NEW)
│   ├── provider/           # Cluster providers
│   ├── stack.rs            # Stack builder (NEW)
│   ├── diagnostics.rs      # Failure diagnostics (NEW)
│   ├── telemetry.rs
│   └── metrics.rs
├── seppo-macros/           # Proc macro crate (NEW)
│   ├── Cargo.toml
│   └── src/lib.rs
└── tests/
    └── context_test.rs
```

---

## WHAT SEPPO IS NOT

- **Not a CLI** - No `seppo test` command
- **Not a test runner** - Works with `cargo test`
- **Not config-driven** - No YAML/TOML files
- **Not kubectl wrapper** - Uses kube-rs directly

---

## SUCCESS CRITERIA

```rust
#[seppo::test]
async fn test_my_app(ctx: TestContext) {
    ctx.apply(my_deployment).await?;
    ctx.wait_ready("deployment/myapp").await?;

    let resp = ctx.forward("svc/myapp", 8080)
        .get("/health").await?;
    assert_eq!(resp.status(), 200);
}
```

```bash
cargo test  # Just works
```

---

**Connect to K8s. Run your tests. That's Seppo.**
