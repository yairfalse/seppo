# Seppo: Kubernetes Testing Framework

**SEPPO = Blacksmith - Forge reliable Kubernetes tests**

---

## CRITICAL: Project Nature

**THIS IS A TEST ORCHESTRATOR, NOT JUST A CLUSTER PROVISIONER**
- **Goal**: Full test lifecycle management for Kubernetes integration tests
- **Language**: 100% Rust (library + CLI)
- **Status**: v0.1.0 - Basic cluster management | v0.2.0 NEXT - Test orchestration
- **Approach**: Real value, not a `kind` wrapper

---

## PROJECT MISSION

**Mission**: Orchestrate Kubernetes integration tests end-to-end

**Core Value Proposition:**

**"Define your test environment, Seppo handles the rest"**

**The 5-Step Test Lifecycle:**

```
┌─────────────────────────────────────────────────────────────────┐
│                    SEPPO TEST ORCHESTRATOR                       │
├─────────────────────────────────────────────────────────────────┤
│                                                                 │
│  1. SETUP CLUSTER                                               │
│     └── Create new Kind cluster OR reuse existing               │
│                                                                 │
│  2. SETUP ENVIRONMENT  ← THIS IS WHERE SEPPO EARNS ITS KEEP    │
│     ├── Load Docker images into cluster                         │
│     ├── Apply K8s manifests (pods, services, gateways)         │
│     ├── Run setup scripts                                       │
│     ├── Wait for readiness (deployments, pods, gateways)       │
│     └── Export KUBECONFIG for tests                            │
│                                                                 │
│  3. RUN TESTS                                                   │
│     └── Execute user's test command (cargo test, go test, etc) │
│                                                                 │
│  4. RESULTS                                                     │
│     ├── Capture exit code                                       │
│     ├── Capture stdout/stderr                                   │
│     └── Report pass/fail                                        │
│                                                                 │
│  5. CLEANUP (WHEN ASKED)                                        │
│     └── Delete cluster on explicit request (not automatic)      │
│     └── Keep cluster for debugging by default                   │
│                                                                 │
└─────────────────────────────────────────────────────────────────┘
```

**Why Seppo Exists:**

Without environment setup, Seppo would be:
```bash
kind create cluster    # Step 1
???                    # Step 2 - USER DOES THIS MANUALLY (tedious, error-prone)
cargo test             # Step 3
kind delete cluster    # Step 5
```

With environment setup, Seppo is:
```bash
seppo test -- cargo test   # All 5 steps handled
```

**The environment setup is the REAL VALUE:**
- Deploy 10 backend pods
- Setup Gateway with specific config
- Create test clients with IPv6
- Configure networking policies
- Wait for everything to be ready
- THEN run tests

Everyone reinvents this. Seppo does it once, correctly.

---

## ARCHITECTURE

### The Design

```
┌─────────────────────────────────────────────────────────────────┐
│                    SEPPO ARCHITECTURE                            │
├─────────────────────────────────────────────────────────────────┤
│                                                                 │
│  User Interface                                                 │
│  ┌─────────────────────┐     ┌─────────────────────────────┐   │
│  │ Rust Library        │     │ CLI (seppo)                 │   │
│  │                     │     │                             │   │
│  │ seppo::test(...)    │     │ seppo test -- cargo test    │   │
│  │ seppo::cleanup(...) │     │ seppo cleanup my-cluster    │   │
│  └─────────────────────┘     └─────────────────────────────┘   │
│            │                           │                        │
│            └───────────┬───────────────┘                        │
│                        ▼                                        │
│  ┌─────────────────────────────────────────────────────────┐   │
│  │  Test Orchestrator Core                                  │   │
│  │                                                         │   │
│  │  1. ClusterManager                                      │   │
│  │     - Create/reuse Kind clusters                        │   │
│  │     - Cluster health checks                             │   │
│  │                                                         │   │
│  │  2. EnvironmentManager  ← THE REAL VALUE               │   │
│  │     - Load Docker images                                │   │
│  │     - Apply manifests (kubectl apply)                   │   │
│  │     - Run setup scripts                                 │   │
│  │     - Wait for readiness                                │   │
│  │     - Export KUBECONFIG                                 │   │
│  │                                                         │   │
│  │  3. TestRunner                                          │   │
│  │     - Execute test commands                             │   │
│  │     - Capture output                                    │   │
│  │     - Report results                                    │   │
│  │                                                         │   │
│  │  4. CleanupManager                                      │   │
│  │     - Delete on explicit request                        │   │
│  │     - Cleanup all seppo clusters                        │   │
│  └─────────────────────────────────────────────────────────┘   │
│                        │                                        │
│                        ▼                                        │
│  ┌─────────────────────────────────────────────────────────┐   │
│  │  External Tools                                          │   │
│  │  - kind (cluster lifecycle)                             │   │
│  │  - kubectl (manifests, wait conditions)                 │   │
│  │  - docker (image management)                            │   │
│  └─────────────────────────────────────────────────────────┘   │
│                                                                 │
└─────────────────────────────────────────────────────────────────┘
```

---

## CONFIGURATION (seppo.yaml)

**Test environments are defined declaratively:**

```yaml
# seppo.yaml - Test environment definition
cluster:
  name: rauta-test
  workers: 3
  k8s_version: "1.31.0"  # optional

environment:
  # Docker images to load into cluster
  images:
    - rauta:test
    - test-backend:latest
    - test-client:latest

  # K8s manifests to apply (in order)
  manifests:
    - ./test/fixtures/namespace.yaml
    - ./test/fixtures/backend-deployment.yaml
    - ./test/fixtures/gateway.yaml
    - ./test/fixtures/httproute.yaml

  # Wait for these conditions before running tests
  wait:
    - condition: available
      resource: deployment/test-backend
      namespace: test
      timeout: 120s
    - condition: programmed
      resource: gateway/test-gateway
      namespace: test
      timeout: 60s

  # Optional setup script (runs after manifests, before tests)
  setup_script: ./scripts/setup-test-env.sh

  # Environment variables to export for tests
  env:
    TEST_GATEWAY_URL: "http://localhost:8080"
    TEST_NAMESPACE: "test"
```

**Example: RAUTA Gateway API Controller Test**

```yaml
# rauta/seppo.yaml
cluster:
  name: rauta-integration
  workers: 2

environment:
  images:
    - rauta:test

  manifests:
    - ./deploy/crds/gateway-api.yaml
    - ./deploy/rauta-daemonset.yaml
    - ./test/fixtures/test-backend.yaml
    - ./test/fixtures/gateway.yaml
    - ./test/fixtures/httproute.yaml

  wait:
    - condition: available
      resource: deployment/test-backend
      timeout: 60s
    - condition: ready
      resource: pod
      selector: app=rauta
      timeout: 120s
```

**Example: Complex Test Environment**

```yaml
# tapio/seppo.yaml - eBPF observability testing
cluster:
  name: tapio-test
  workers: 5  # Need multiple nodes for network tests

environment:
  images:
    - tapio-agent:test
    - test-workload:latest

  manifests:
    - ./test/fixtures/namespaces.yaml
    - ./test/fixtures/network-policies.yaml
    - ./test/fixtures/workloads/10-pod-deployment.yaml
    - ./test/fixtures/workloads/client-with-ipv6.yaml
    - ./test/fixtures/tapio-daemonset.yaml

  wait:
    - condition: available
      resource: deployment/test-workload
      namespace: workloads
      replicas: 10  # Wait for all 10 pods
      timeout: 180s
    - condition: ready
      resource: daemonset/tapio-agent
      namespace: kube-system
      timeout: 120s

  setup_script: ./scripts/verify-ebpf-loaded.sh
```

---

## CLI USAGE

```bash
# Run tests (cluster + environment persist after)
$ seppo test -- cargo test integration
$ seppo test -- go test ./integration/...
$ seppo test -- pytest tests/

# Run tests with specific config
$ seppo test --config ./seppo.yaml -- cargo test

# Run tests AND cleanup after
$ seppo test --cleanup -- cargo test

# Just setup environment (no tests)
$ seppo setup
$ seppo setup --config ./custom-seppo.yaml

# Manual cleanup
$ seppo cleanup                    # Cleanup cluster from seppo.yaml
$ seppo cleanup my-cluster         # Cleanup specific cluster
$ seppo cleanup --all              # Cleanup ALL seppo-created clusters

# Status
$ seppo status                     # Show cluster and environment status
```

---

## RUST LIBRARY API

```rust
use seppo::{Config, TestResult};

// Load config from seppo.yaml
let config = Config::from_file("seppo.yaml")?;

// Run tests with full orchestration
let result: TestResult = seppo::test(&config, || async {
    // Your test code here
    // Environment is already set up
    // KUBECONFIG is exported

    let client = kube::Client::try_default().await?;
    // ... run assertions

    Ok(())
}).await?;

// Check results
if result.passed {
    println!("Tests passed!");
} else {
    println!("Tests failed: {}", result.output);
}

// Cleanup when ready (explicit, not automatic)
seppo::cleanup(&config).await?;
```

**Or run external command:**

```rust
use seppo::{Config, TestResult};

let config = Config::from_file("seppo.yaml")?;

// Setup environment, run command, capture results
let result = seppo::test_command(&config, "cargo", &["test", "integration"]).await?;

println!("Exit code: {}", result.exit_code);
println!("Stdout: {}", result.stdout);
println!("Stderr: {}", result.stderr);

// Cluster persists - cleanup when ready
// seppo::cleanup(&config).await?;
```

---

## CORE API (Current v0.1.0)

**Note:** Current API is basic cluster management. Full orchestration coming in v0.2.0.

```rust
// src/cluster.rs (current)

/// Create a Kind cluster (idempotent - reuses if exists)
pub async fn create(name: &str) -> Result<(), Box<dyn std::error::Error>>

/// Delete a Kind cluster
pub async fn delete(name: &str) -> Result<(), Box<dyn std::error::Error>>

/// Load a Docker image into the cluster
pub async fn load_image(cluster_name: &str, image: &str) -> Result<(), Box<dyn std::error::Error>>
```

---

## DEPENDENCIES

### External Tools (Required)

| Tool | Purpose | Install |
|------|---------|---------|
| `kind` | Cluster creation | https://kind.sigs.k8s.io/ |
| `kubectl` | Manifests, wait conditions | https://kubernetes.io/docs/tasks/tools/ |
| `docker` | Container runtime, images | https://docs.docker.com/get-docker/ |

### Rust Dependencies

```toml
# Core
tokio = "1.41"          # Async runtime
kube = "1.0"            # K8s client
k8s-openapi = "0.25"    # K8s API types

# Config
serde = "1"
serde_yaml = "0.9"

# Error handling
thiserror = "2"
anyhow = "1"

# CLI (for seppo binary)
clap = "4"
```

---

## TDD WORKFLOW (RED -> GREEN -> REFACTOR)

**MANDATORY**: All code follows strict Test-Driven Development in small batches.

### RED Phase: Write Failing Test First

```rust
// Step 1: Write test that FAILS (RED)
#[tokio::test]
#[ignore]
async fn test_environment_setup_applies_manifests() {
    let config = Config::from_str(r#"
        cluster:
          name: manifest-test
        environment:
          manifests:
            - ./test/fixtures/simple-pod.yaml
          wait:
            - condition: ready
              resource: pod/test-pod
              timeout: 60s
    "#).unwrap();

    // Setup environment (not yet implemented)
    seppo::setup(&config).await.expect("Should setup environment");

    // Verify pod exists and is ready
    let client = kube::Client::try_default().await.unwrap();
    let pods: Api<Pod> = Api::namespaced(client, "default");
    let pod = pods.get("test-pod").await.expect("Pod should exist");

    assert_eq!(
        pod.status.unwrap().phase.unwrap(),
        "Running"
    );

    seppo::cleanup(&config).await.unwrap();
}

// Step 2: Verify test FAILS
// $ cargo test -- --ignored test_environment_setup_applies_manifests
// # FAILED - setup() doesn't exist (RED phase confirmed)
```

### GREEN Phase: Minimal Implementation

```rust
// Step 3: Write MINIMAL code to pass test
pub async fn setup(config: &Config) -> Result<(), SeppoError> {
    // 1. Create/reuse cluster
    cluster::create(&config.cluster.name).await?;

    // 2. Apply manifests
    for manifest in &config.environment.manifests {
        kubectl_apply(manifest).await?;
    }

    // 3. Wait for conditions
    for wait in &config.environment.wait {
        kubectl_wait(wait).await?;
    }

    Ok(())
}

// Step 4: Verify test PASSES
// $ cargo test -- --ignored test_environment_setup_applies_manifests
// # ok (GREEN phase confirmed)
```

### TDD Checklist

- [ ] **RED**: Write failing test first
- [ ] **RED**: Verify test fails (or doesn't compile)
- [ ] **GREEN**: Write minimal implementation to pass
- [ ] **GREEN**: Verify test passes
- [ ] **REFACTOR**: Clean up, add edge cases
- [ ] **REFACTOR**: Verify tests still pass
- [ ] **COMMIT**: Small, incremental commit

### Example Session (Small Batches)

```bash
# Feature: Environment manifest application (TDD)

# Commit 1: RED
git commit -m "test: add test for manifest application (failing)"

# Commit 2: GREEN
git commit -m "feat: add setup() with manifest application"

# Commit 3: RED
git commit -m "test: add test for wait conditions (failing)"

# Commit 4: GREEN
git commit -m "feat: add wait condition support to setup()"

# Commit 5: REFACTOR
git commit -m "refactor: extract kubectl helpers to separate module"
```

**NO BIG BANG COMMITS. SMALL STEPS ONLY.**

---

## ROADMAP

### v0.1.0 (Current)

- [x] Kind cluster create/delete
- [x] Docker image loading
- [x] Gateway API CRD installation
- [x] Cluster reuse (idempotent create)
- [x] Basic Rust library API

### v0.2.0 (Next - Test Orchestrator)

- [ ] `seppo.yaml` configuration format
- [ ] Environment setup (manifests, images, wait conditions)
- [ ] `seppo test` command (full orchestration)
- [ ] `seppo setup` command (environment only)
- [ ] `seppo cleanup` command (explicit cleanup)
- [ ] `seppo status` command
- [ ] Test result capture and reporting
- [ ] Setup script support

### v0.3.0 (Future)

- [ ] Parallel test environments
- [ ] Test isolation (namespaced environments)
- [ ] Environment snapshots/restore
- [ ] CI/CD mode (optimized for GitHub Actions)
- [ ] Multi-cluster support (EKS, AKS, GKE)
- [ ] ClusterProvider trait abstraction

---

## CODEBASE STRUCTURE

```
seppo/
├── Cargo.toml              # Package manifest
├── README.md               # User documentation
├── CLAUDE.md               # This file - AI assistant context
├── src/
│   ├── lib.rs              # Crate root, public exports
│   ├── cluster.rs          # Kind cluster management (v0.1.0)
│   ├── config.rs           # seppo.yaml parsing (v0.2.0)
│   ├── environment.rs      # Environment setup (v0.2.0)
│   ├── runner.rs           # Test execution (v0.2.0)
│   └── cleanup.rs          # Cleanup management (v0.2.0)
├── src/bin/
│   └── seppo.rs            # CLI binary (v0.2.0)
└── examples/
    └── rauta_integration_test.rs
```

---

## ERROR HANDLING

```rust
#[derive(Debug, thiserror::Error)]
pub enum SeppoError {
    // Cluster errors
    #[error("Kind not installed. Install from: https://kind.sigs.k8s.io/")]
    KindNotFound,

    #[error("Docker not running. Start Docker and try again.")]
    DockerNotRunning,

    #[error("Cluster '{name}' creation failed: {reason}")]
    ClusterCreationFailed { name: String, reason: String },

    // Environment errors
    #[error("Manifest not found: {path}")]
    ManifestNotFound { path: String },

    #[error("kubectl apply failed for {manifest}: {reason}")]
    ManifestApplyFailed { manifest: String, reason: String },

    #[error("Wait condition timed out: {condition} on {resource}")]
    WaitTimeout { condition: String, resource: String },

    #[error("Image load failed: {image}: {reason}")]
    ImageLoadFailed { image: String, reason: String },

    // Config errors
    #[error("Config file not found: {path}")]
    ConfigNotFound { path: String },

    #[error("Invalid config: {reason}")]
    InvalidConfig { reason: String },

    // Test errors
    #[error("Test command failed with exit code {code}")]
    TestFailed { code: i32 },
}
```

---

## RELATED PROJECTS

| Project | Description | Seppo Usage |
|---------|-------------|-------------|
| RAUTA | K8s Gateway API controller | Primary test consumer (Rust library) |
| TAPIO | K8s runtime + eBPF observability | CLI integration |
| AHTI | Correlation graph database | CLI integration |
| KULTA | gRPC golden path framework | Library integration |

---

## DEFINITION OF DONE

A feature is complete when:

- [ ] Function implemented with proper error handling
- [ ] Integration test added (with `#[ignore]`)
- [ ] seppo.yaml schema updated (if config change)
- [ ] README.md updated
- [ ] CLAUDE.md updated
- [ ] `cargo fmt && cargo clippy` passes
- [ ] Works on Linux and macOS

---

## WHAT SEPPO IS vs IS NOT

### Seppo IS

- **Test orchestrator** - Full lifecycle management
- **Environment provisioner** - Deploy test infrastructure
- **Cleanup manager** - Explicit, controlled cleanup
- **Polyglot** - Works with any test framework (cargo, go, pytest)

### Seppo IS NOT

- **Just a Kind wrapper** - That would be a joke
- **A test framework** - It orchestrates YOUR tests, doesn't replace cargo test/go test
- **Automatic cleanup** - Cleanup is explicit (for debugging)
- **A CI system** - It runs in CI, doesn't replace GitHub Actions

---

**Forge your tests with confidence.**
