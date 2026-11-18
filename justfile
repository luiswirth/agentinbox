# Warnings are errors here, so that a local build stays warning-tolerant.
export RUSTFLAGS := "-D warnings"
export RUSTDOCFLAGS := "-D warnings"

# Everything CI runs.
ci:
    nix fmt -- --ci
    cargo clippy --all-targets
    cargo test
    cargo doc --no-deps
