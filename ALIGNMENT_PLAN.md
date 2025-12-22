# Seppo Alignment Plan

**Goal:** Align seppo (Rust) with ilmari (Go) APIs for consistent "K8s as a library" experience.

---

## 1. API Renames

### PortForward (not Forward)

```rust
// BEFORE
ctx.forward("svc/api", 8080).await?;
ctx.forward_to("svc/api", 8080).await?;

// AFTER
ctx.port_forward("svc/api", 8080).await?;
```

**Files to change:**
- `src/context.rs` - rename methods
- `src/portforward.rs` - update if needed
- `src/lib.rs` - update exports
- All tests

---

## 2. Typed Assertions

### Replace unified Assert with typed methods

```rust
// BEFORE (if exists)
ctx.assert("deployment/api").has_replicas(3);

// AFTER
ctx.assert_pod("api-xyz").has_no_restarts().must().await?;
ctx.assert_deployment("api").has_replicas(3).is_ready().must().await?;
ctx.assert_service("api").exists().must().await?;
ctx.assert_pvc("data").is_bound().has_capacity("10Gi").must().await?;
```

**Methods per type:**

| Type | Methods |
|------|---------|
| `assert_pod()` | `exists`, `is_ready`, `has_no_restarts`, `no_oom_kills`, `logs_contain`, `has_label` |
| `assert_deployment()` | `exists`, `has_replicas`, `is_ready`, `is_progressing`, `has_label` |
| `assert_service()` | `exists`, `has_label` |
| `assert_pvc()` | `exists`, `is_bound`, `has_storage_class`, `has_capacity` |

**Files to change:**
- `src/assertions.rs` - add typed assertion structs
- `src/context.rs` - add `assert_pod()`, `assert_deployment()`, etc.
- `src/lib.rs` - export new types

---

## 3. Builder Syntax

### Use function style, not Type::new()

```rust
// BEFORE
let deploy = DeploymentFixture::new("api").image("v1").build();

// AFTER
let deploy = deployment("api").image("v1").build();

// Also for others:
let stack = stack().service("api").image("v1").service("db").image("postgres").build();
let sa = service_account("my-sa").can_get("pods").can_list("deployments").build();
```

**Implementation:**
```rust
// src/fixtures.rs or src/builders.rs

pub fn deployment(name: &str) -> DeploymentBuilder {
    DeploymentBuilder::new(name)
}

pub fn stack() -> StackBuilder {
    StackBuilder::new()
}

pub fn service_account(name: &str) -> ServiceAccountBuilder {
    ServiceAccountBuilder::new(name)
}
```

**Note:** `ServiceAccountBuilder` and the `service_account()` helper are planned for future implementation and are **not** part of this PR.
**Files to change:**
- `src/fixtures.rs` - add lowercase functions
- `src/stack.rs` - add `stack()` function
- `src/lib.rs` - export functions

---

## 4. Add Metrics()

### Simple CPU/memory access

```rust
let metrics = ctx.metrics("pod/api-xyz").await?;

println!("CPU: {}", metrics.cpu);           // "250m"
println!("Memory: {}", metrics.memory);     // "512Mi"
println!("CPU %: {}", metrics.cpu_percent); // 25.0
println!("Mem %: {}", metrics.memory_percent); // 50.0
```

**Implementation:**
- Uses metrics-server API (`/apis/metrics.k8s.io/v1beta1`)
- Returns `PodMetrics` struct
- Calculates percentages from pod resource limits

**Files to create/change:**
- `src/metrics.rs` - new module (or extend existing)
- `src/context.rs` - add `metrics()` method

---

## 5. Add Debug()

### Combined diagnostics dump

```rust
ctx.debug("deployment/api").await?;

// Prints:
// === deployment/api ===
// Status: 3/3 ready
//
// === Pods ===
// api-xyz-123: Running (0 restarts)
// api-xyz-456: Running (0 restarts)
//
// === Events ===
// 10:42:01 Scheduled: Assigned to node-1
// 10:42:02 Pulled: Image pulled
//
// === Logs (last 20 lines) ===
// [api-xyz-123] Server started on :8080
// ...
```

**Files to change:**
- `src/context.rs` - add `debug()` method
- `src/diagnostics.rs` - helper functions

---

## 6. Better Error Messages

### Human-readable, not K8s gibberish

```rust
// BEFORE
"the server could not find the requested resource"

// AFTER
"Deployment 'api' failed: image 'api:v99' not found in registry gcr.io/myproject"
```

**Implementation:**
- Wrap kube-rs errors with context
- Parse common error patterns
- Add resource name to all errors

**Files to change:**
- `src/context.rs` - wrap errors with context
- Create error helper module if needed

---

## 7. Examples

Create `examples/` directory:

```
examples/
├── basic/
│   ├── Cargo.toml
│   └── main.rs          # Deploy nginx, port-forward, check health
├── infra-package/
│   ├── Cargo.toml
│   └── src/
│       ├── main.rs      # CLI entrypoint
│       ├── api.rs       # API deployment
│       └── database.rs  # DB deployment
├── embedded/
│   ├── Cargo.toml
│   └── main.rs          # Self-deploying app
├── integration-test/
│   ├── Cargo.toml
│   └── tests/
│       └── api_test.rs  # #[seppo::test] examples
└── multi-service/
    ├── Cargo.toml
    └── main.rs          # Stack with API + DB + Redis
```

---

## 8. README Updates

### Positioning: "Kubernetes as a library"

Key messages:
- No YAML, just code
- Import and use
- Same API as ilmari (Go)
- Type-safe, compile-time checks

---

## Verification Checklist

Before each change:
- [ ] `cargo build` passes
- [ ] `cargo test` passes
- [ ] `cargo clippy` clean
- [ ] `cargo fmt` applied

---

## Order of Implementation

1. PortForward rename (simple)
2. Builder functions (simple)
3. Typed assertions (medium)
4. Metrics() (medium)
5. Debug() (medium)
6. Error messages (medium)
7. Examples (after APIs stable)
8. README updates (last)

---

## Git Workflow

**NEVER push to main. Always use feature branches + PRs.**

```bash
git checkout -b feat/rename-portforward
# make changes
git push -u origin feat/rename-portforward
gh pr create --title "Rename Forward to PortForward"
```
