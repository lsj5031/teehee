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

# Alert on sender trouble (ring fill, drift saturation, dead sender).
# Interactive foreground watcher; the TeeHeeWatch scheduled task runs
# the same script in -Check mode every 5 min.
watch-send:
    powershell -NoProfile -ExecutionPolicy Bypass -File scripts/send_watch.ps1

# Run the sender in the foreground (same args as the TeeHeeSender
# scheduled task, which runs teehee.exe directly — no supervisor).
# For manual runs / debugging; the task self-heals via its 2-min
# periodic trigger + the sender's single-instance mutex guard.
sender:
    ./target/release/teehee.exe send --host 192.168.1.111 --port 5000 --capture-source=loopback --stats --chunk-ms 3 --log-file logs/send.jsonl --capture-buffer-ms 400

# Run the full CI suite locally (fmt + clippy + test)
ci: lint test

# Clean build artifacts
clean:
    cargo clean
