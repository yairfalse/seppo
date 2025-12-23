// Seppo CI Pipeline
// Run with: sykli (or cargo run --bin sykli --features sykli -- --emit)
#![allow(unused_must_use)]

use sykli_sdk::Pipeline;

fn main() {
    let mut p = Pipeline::new();

    // Level 0: These run in parallel
    p.task("fmt-check")
        .run("cargo fmt --all --check")
        .inputs(&["**/*.rs"]);

    p.task("lint")
        .run("cargo clippy --all-targets --all-features -- -D warnings")
        .inputs(&["**/*.rs", "Cargo.toml", "Cargo.lock"]);

    p.task("test")
        .run("cargo test --workspace --all-features")
        .inputs(&["**/*.rs", "Cargo.toml", "Cargo.lock"]);

    p.task("audit")
        .run("cargo audit")
        .inputs(&["Cargo.lock"])
        .retry(1); // May fail if not installed

    // Level 1: Build after all checks pass
    // Build only the library (skip sykli binary which requires --features sykli)
    p.task("build")
        .run("cargo build --release --lib")
        .after(&["fmt-check", "lint", "test", "audit"])
        .inputs(&["**/*.rs", "Cargo.toml", "Cargo.lock"]);

    p.emit();
}
