# Seppo - Kubernetes Testing Framework

**Seppo** (Finnish: "blacksmith") - Forge your Kubernetes tests with confidence.

A lightweight Kubernetes testing framework for integration tests, designed for both Rust and Go projects.

---

## Features

- **Kind Cluster Management**: Create and destroy Kind clusters programmatically
- **Docker Image Loading**: Load local Docker images into Kind clusters
- **Gateway API Support**: Automatic installation of Gateway API CRDs
- **Rust Native**: Written in Rust, ergonomic Rust library API
- **Go Compatible**: CLI tool for Go projects (future)
- **Fast**: Minimal dependencies, quick cluster creation

---

## Installation

### For Rust Projects (Library)

Add to your `Cargo.toml`:

```toml
[dev-dependencies]
seppo = { path = "../seppo" }  # Or from crates.io when published
tokio = { version = "1", features = ["full"] }
```

### For Go Projects (CLI - Coming Soon)

```bash
# Install CLI
cargo install --path seppo/cli

# Or download binary from releases
wget https://github.com/you/seppo/releases/download/v0.1.0/seppo-cli
```

---

## Usage

### Rust Projects (as Library)

**Example: Basic Cluster Management**

```rust
use seppo::cluster;

#[tokio::test]
async fn test_my_kubernetes_app() -> Result<(), Box<dyn std::error::Error>> {
    // Create Kind cluster
    cluster::create("my-test-cluster").await?;

    // Your tests here...
    // - Deploy your app
    // - Run integration tests
    // - Verify behavior

    // Cleanup
    cluster::delete("my-test-cluster").await?;

    Ok(())
}
```

**Example: With Docker Image Loading**

```rust
use seppo::cluster;

#[tokio::test]
async fn test_with_docker_image() -> Result<(), Box<dyn std::error::Error>> {
    let cluster_name = "test-cluster";

    // Create cluster
    cluster::create(cluster_name).await?;

    // Build your app image
    std::process::Command::new("docker")
        .args(&["build", "-t", "my-app:test", "."])
        .status()?;

    // Load image into Kind cluster
    cluster::load_image(cluster_name, "my-app:test").await?;

    // Now deploy and test your app...

    // Cleanup
    cluster::delete(cluster_name).await?;

    Ok(())
}
```

**Example: Integration Test for RAUTA Gateway API Controller**

```rust
use seppo::cluster;
use kube::Client;

#[tokio::test]
async fn test_rauta_gateway() -> Result<(), Box<dyn std::error::Error>> {
    // Setup cluster
    cluster::create("rauta-test").await?;

    // Build and load RAUTA
    std::process::Command::new("cargo")
        .args(&["build", "--release"])
        .current_dir("../rauta")
        .status()?;

    std::process::Command::new("docker")
        .args(&["build", "-t", "rauta:test", "-f", "docker/Dockerfile.control-local", "."])
        .current_dir("../rauta")
        .status()?;

    cluster::load_image("rauta-test", "rauta:test").await?;

    // Deploy RAUTA (using kubectl or kube-rs)
    // ...

    // Run tests
    let client = Client::try_default().await?;
    // Test Gateway API resources...

    // Cleanup
    cluster::delete("rauta-test").await?;

    Ok(())
}
```

### Go Projects (CLI - Coming Soon)

For Go projects like TAPIO and AHTI, Seppo will provide a proper CLI binary (written in Rust) that integrates with Go testing frameworks.

**Planned Go Integration:**

```go
// integration_test.go
package integration

import (
    "testing"
    "os/exec"
)

func TestTAPIOwithSeppo(t *testing.T) {
    // Seppo CLI handles cluster lifecycle
    // Your Go tests use standard k8s client-go

    // Integration with testing frameworks like:
    // - testify
    // - ginkgo/gomega
    // - godog (Cucumber for Go)
}
```

**Note:** Proper Go integration coming in v0.2.0. Framework-based, not scripts.

---

## Architecture

Seppo is designed as:

1. **Rust Library** (`seppo` crate): For Rust projects like RAUTA, KULTA
2. **CLI Tool** (`seppo-cli`): For Go projects like TAPIO, AHTI

Both share the same core cluster management logic, ensuring consistency across your polyglot stack.

### Why This Design?

- **Rust projects** get native ergonomic API with strong typing and async/await
- **Go projects** use simple CLI (no CGO complexity, no Rust embedding)
- **Same behavior** across all projects in your organization
- **Easy to test** Seppo itself (library API is directly testable)

---

## Supported Cluster Types

### Current: Kind (Kubernetes in Docker)

- Create/delete clusters
- Custom node configuration
- Load Docker images
- Gateway API CRD installation

### Future: Multi-Cloud (Planned)

- EKS (AWS Elastic Kubernetes Service)
- AKS (Azure Kubernetes Service)
- GKE (Google Kubernetes Engine)
- K3s (Lightweight Kubernetes)
- Existing clusters (use your own kubeconfig)

---

## Comparison with Alternatives

### vs Testkube

- **Testkube**: Run app tests *inside* Kubernetes
- **Seppo**: Manage Kubernetes clusters *for* integration tests
- **Use case**: Different! Use Seppo to create clusters, Testkube to orchestrate tests.

### vs Kind CLI

- **Kind CLI**: Manual cluster management via shell
- **Seppo**: Programmatic API (Rust) + CLI (Go-friendly)
- **Benefit**: Type-safe Rust API, consistent across projects

### vs Custom Scripts

- **Custom scripts**: Each project has own cluster management
- **Seppo**: Shared library, consistent behavior
- **Benefit**: DRY (Don't Repeat Yourself), easier to maintain

---

## Development

### Building from Source

```bash
# Clone repository
git clone https://github.com/you/seppo
cd seppo

# Build library
cargo build --release

# Run tests
cargo test

# Run integration tests (requires kind installed)
cargo test -- --ignored
```

### Testing Seppo Itself

```bash
# Unit tests
cargo test

# Integration tests (creates real Kind cluster)
cargo test -- --ignored test_create_and_delete_cluster
```

---

## Roadmap

### v0.1.0 (Current)

- [x] Basic Kind cluster management
- [x] Docker image loading
- [x] Gateway API CRD installation
- [x] Rust library API

### v0.2.0 (Next)

- [ ] CLI tool for Go projects
- [ ] Cluster reuse/cleanup config
- [ ] Better error messages
- [ ] Progress indicators

### v0.3.0 (Future)

- [ ] Multi-cloud support (EKS, AKS, GKE)
- [ ] ClusterProvider trait abstraction
- [ ] Parallel cluster creation
- [ ] Cluster health checks

---

## Contributing

Contributions welcome! Please:

1. Fork the repository
2. Create a feature branch
3. Write tests for your changes
4. Run `cargo fmt` and `cargo clippy`
5. Submit a pull request

---

## License

Apache-2.0

---

## Authors

- Yair (@yairfalse)

---

## FAQ

**Q: Why "Seppo"?**

A: Finnish for "blacksmith" - forging reliable Kubernetes tests!

**Q: Do I need to install Kind?**

A: Yes, Seppo uses the `kind` CLI under the hood. Install from https://kind.sigs.k8s.io/

**Q: Can I use Seppo with Minikube?**

A: Not yet, but planned for future versions (ClusterProvider abstraction).

**Q: Does Seppo work on macOS/Linux/Windows?**

A: Yes, anywhere Kind works. Windows support via WSL2.

**Q: How does Seppo compare to controller-runtime's envtest?**

A: Different approach:
- **envtest**: Runs minimal K8s API server (no containers, no nodes)
- **Seppo**: Uses full Kind clusters (real containers, real networking)
- **Use case**: Seppo for full integration tests, envtest for unit tests

---

**Built for the Kubernetes community**
