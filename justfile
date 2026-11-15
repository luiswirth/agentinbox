# Warnings are errors here, so that a local build stays warning-tolerant.
export RUSTFLAGS := "-D warnings"
# Doc comments carry the mathematics, so a broken intra-doc link is an error.
export RUSTDOCFLAGS := "-D warnings"

# Everything CI runs.
ci:
    nix fmt -- --ci
    nix flake check
    cargo clippy --workspace --all-targets
    cargo test --workspace
    cargo doc --workspace --no-deps
