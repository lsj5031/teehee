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

# Rotate oversized legacy logs into logs/archive and prune old archives.
# The GUI rotates its own session log; this is for old CLI/task logs.
rotate-logs:
    powershell -NoProfile -ExecutionPolicy Bypass -File scripts/rotate_logs.ps1

# Run the CLI sender in the foreground for manual debugging. The GUI in
# gui/ is the supported sender; this path does not start at login or
# reconnect after an error.
sender:
    ./target/release/teehee.exe send --host 192.168.1.111 --port 5000 --capture-source=loopback --stats --chunk-ms 3 --log-file logs/send.jsonl --capture-buffer-ms 1200

# Run the full CI suite locally (fmt + clippy + test)
ci: lint test

# Clean build artifacts
clean:
    cargo clean
