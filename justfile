# SEPPO - Kubernetes Testing Framework
# Test orchestration made simple

# Show available commands (default)
default:
  @just --list

# === Quick Commands ===

# Format Rust code
fmt:
  @echo "📝 Formatting..."
  cargo fmt --all

# Run tests (fast)
test:
  @echo "🧪 Running tests..."
  cargo test --workspace

# Build seppo binary
build:
  @echo "🔨 Building seppo..."
  cargo build --release
  @echo "✅ Binary: target/release/seppo"

# === CI Checks ===

# Check formatting
fmt-check:
  @echo "🔍 Checking format..."
  cargo fmt --all --check

# Lint with clippy
lint:
  @echo "🔍 Running clippy..."
  cargo clippy --all-targets --all-features -- -D warnings

# Run all tests
test-all:
  @echo "🧪 Running all tests..."
  cargo test --workspace --all-features

# Security audit
audit:
  @echo "🔒 Running security audit..."
  cargo audit || echo "⚠️  cargo-audit not installed (run: cargo install cargo-audit)"

# Full CI (before pushing)
ci: fmt-check lint test-all audit build
  @echo "✅ Local CI passed! Safe to push."

# === Development ===

# Run tests with live reload (requires cargo-watch)
watch:
  @echo "👀 Watching for changes..."
  cargo watch -x test || echo "❌ cargo-watch not installed (run: cargo install cargo-watch)"

# Run specific test
test-one TEST:
  @echo "🧪 Running test: {{TEST}}"
  cargo test {{TEST}} -- --nocapture

# Check uncommitted changes
diff:
  @echo "📝 Uncommitted changes:"
  git diff
  @echo "\n📦 Staged changes:"
  git diff --cached

# === Seppo Usage (Native API) ===

# Run example test (native Rust API)
example:
  @echo "🎬 Running example test..."
  cargo run --example simple || echo "⚠️  No example found (check examples/ directory)"

# Run specific example
run-example NAME:
  @echo "🎬 Running example: {{NAME}}"
  cargo run --example {{NAME}}

# === Cleanup ===

# Clean build artifacts
clean:
  @echo "🧹 Cleaning..."
  cargo clean

# === Setup ===

# Install dev dependencies
setup:
  @echo "📦 Installing dev dependencies..."
  cargo install cargo-watch cargo-nextest cargo-audit
  brew install just kind kubectl
  @echo "✅ Setup complete!"

# Install git hooks (pre-commit)
install-hooks:
  @echo "🔗 Installing git hooks..."
  @if [ ! -f .git/hooks/pre-commit ]; then \
    echo "⚠️  Pre-commit hook not found (expected in .git/hooks/pre-commit)"; \
  else \
    chmod +x .git/hooks/pre-commit && echo "✅ Pre-commit hook installed"; \
  fi
  @echo "✅ Hooks ready!"

# === Git Shortcuts ===

# Commit (with pre-commit checks)
commit MESSAGE: fmt-check lint
  git add .
  git commit -m "{{MESSAGE}}"

# Commit + push (after full CI)
ship MESSAGE: ci
  git add .
  git commit -m "{{MESSAGE}}"
  git push

# === Meta ===

# Show tool versions
versions:
  @echo "Rust:  $(rustc --version)"
  @echo "Cargo: $(cargo --version)"
  @echo "Just:  $(just --version)"
  @echo "Kind:  $(kind version)"

# Project stats
stats:
  @echo "📊 Project Statistics:"
  @echo "Rust files:  $(find . -name '*.rs' -not -path './target/*' | wc -l)"
  @echo "Total lines: $(find . -name '*.rs' -not -path './target/*' | xargs wc -l | tail -1)"
  @echo "Git commits: $(git rev-list --count HEAD)"
