# teehee development commands
# Run `just --list` to see all available recipes.

# Build release binary
build:
    cargo build --release

# Build debug binary (fast compile)
debug:
    cargo build

# Run all tests
test:
    cargo test --workspace

# Run clippy (deny warnings) and rustfmt check
lint: fmt-check clippy

# Run rustfmt formatting check
fmt-check:
    cargo fmt --all --check

# Auto-format all source files
fmt:
    cargo fmt --all

# Run clippy with warnings as errors
clippy:
    cargo clippy --all-targets -- -D warnings

# Quick type-check without full compilation
check:
    cargo check --workspace

# Alert on sender trouble by tailing logs/send.jsonl (ring fill,
# drift saturation, dead sender)
watch-send:
    bash scripts/send_watch.sh

# Launch/keep-alive the sender in the foreground (logs/supervisor.log);
# designed to run at logon via the TeeHeeSender scheduled task
supervisor:
    bash scripts/send_supervisor.sh

# Run the full CI suite locally (fmt + clippy + test)
ci: lint test

# Clean build artifacts
clean:
    cargo clean
