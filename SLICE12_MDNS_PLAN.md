# Slice 12 plan: mDNS auto-discovery wire-up

## Current factual state

TeeHee already has the core LAN audio slices in place: capture, decode,
jitter buffering, UDP send/receive, format reconciliation, MTU sizing,
receiver buffer sizing, and capture-source selection. The next highest-UX
slice is mDNS auto-discovery so users can run a receiver and then send audio
without manually discovering and typing the receiver IP address.

Workspace state at the time this plan was written:

- `src/discovery.rs` exists as an untracked module, but is currently inert:
  it is not declared from `src/lib.rs`, and `mdns-sd` is not yet listed in
  `Cargo.toml`.
- `src/spike_packet_ring.rs` exists as an untracked spike module and is already
  declared from `src/lib.rs` under the spike section. Its focused tests pass
  with `cargo test spike_packet_ring --lib`.
- `src/lib.rs` has local modifications for the spike module.
- `Cargo.toml`, `src/cli.rs`, and `src/main.rs` are not yet wired for mDNS.

Important correction from review: the remaining work should be done with
small contextual patches, not full-file rewrites of `cli.rs` or `main.rs`.
The failed PowerShell editing attempts were tooling problems, not a reason to
replace large files wholesale.

## Product goal

Add an opt-in mDNS path:

```bash
teehee recv --mdns --port 5000
teehee send --mdns
```

The receiver advertises `_teehee._udp.local.` after its UDP receive socket is
successfully bound. The sender browses for the first matching TeeHee receiver,
resolves it to an IPv4 socket address, and then uses the existing UDP sender
path unchanged.

Manual addressing remains supported and should stay the simplest explicit
escape hatch:

```bash
teehee send --host 192.168.0.42 --port 5000
```

## Non-goals for slice 12

- No pairing or trust model.
- No encryption.
- No receiver selection UI when multiple receivers are found.
- No automatic retry loop after discovery timeout.
- No background daemon.
- No protocol header change.
- No Opus or FEC work in this slice.

Those belong to later slices:

1. Slice 13: Opus codec and a packet codec tag.
2. Slice 14: Reed-Solomon FEC.
3. Slice 15/backlog: built-in pairing and receiver identity.

## Design decisions

### Discovery is opt-in

Both sides should require `--mdns`:

- `teehee recv --mdns` advertises.
- `teehee send --mdns` resolves.

This avoids surprising LAN broadcasts for users who prefer explicit `--host`.

### Receiver advertises only after bind succeeds

`run_recv` should bind the UDP receiver first, then create the RAII
`Advertiser`. This prevents advertising a port that failed to bind.

Desired lifetime shape:

```text
parse args
validate args
bind Receiver
if --mdns: create Advertiser for bound/listening port
enter receive/playback loop while holding Advertiser
drop Advertiser on shutdown
```

### Sender resolves before audio capture/send hot path

`run_send` should resolve mDNS before starting capture and before sending any
audio packets. Discovery may block up to `--mdns-timeout-ms`; that is acceptable
at startup but must not happen on an audio thread.

Desired sender shape:

```text
parse args
validate args -> ResolvedTarget
if target is mDNS: resolve_with_timeout(timeout)
else: use explicit host/port
start existing sender pipeline
```

### Keep the CLI contract explicit

`--mdns` and `--host` are mutually exclusive for `send`.

Valid send forms:

- `teehee send --host 192.168.0.42`
- `teehee send --host 192.168.0.42 --port 5001`
- `teehee send --mdns`
- `teehee send --mdns --port 5001` only if the CLI intentionally treats
  `--port` as a filter/default for discovery. Prefer not supporting this unless
  the current code structure needs it; mDNS SRV records already carry the port.

Invalid send forms:

- `teehee send` with neither `--host` nor `--mdns`.
- `teehee send --host 192.168.0.42 --mdns`.

Valid recv forms:

- `teehee recv`
- `teehee recv --port 5000`
- `teehee recv --mdns`
- `teehee recv --mdns --port 5000`

## Implementation plan

### 1. Add the dependency

Patch `Cargo.toml`:

```toml
mdns-sd = "0.7"
```

Place it with the normal dependencies, not under the Windows-only target
section. mDNS is cross-platform and needed for both sender and receiver on
Windows/macOS/Linux.

After adding it, run a narrow build/test to let Cargo update `Cargo.lock`.

### 2. Wire `discovery` into the library

Patch `src/lib.rs`:

- Add a rustdoc bullet for `discovery` near the other runtime modules.
- Add `pub mod discovery;` in the module list.

Keep the existing spike section intact. Do not move or rewrite unrelated module
comments.

Expected result: `src/discovery.rs` now compiles and its tests are visible to
Cargo.

### 3. Update `ResolvedTarget` in `src/cli.rs`

Current shape is a struct similar to:

```rust
pub struct ResolvedTarget {
    host: String,
    port: u16,
}
```

Change it into an enum so validation can represent either an already-known
socket target or a deferred mDNS lookup:

```rust
pub enum ResolvedTarget {
    Explicit { host: String, port: u16 },
    Mdns { timeout_ms: u64 },
}
```

Add small accessor methods that preserve existing call-site ergonomics where
possible:

```rust
impl ResolvedTarget {
    pub fn explicit(host: impl Into<String>, port: u16) -> Self;
    pub fn mdns(timeout_ms: u64) -> Self;
    pub fn host(&self) -> Option<&str>;
    pub fn port(&self) -> Option<u16>;
    pub fn to_socket_string(&self) -> Option<String>;
}
```

Prefer `Option` accessors over panics. Tests and call sites can then be updated
to handle explicit-only assumptions directly. If the existing code strongly
expects `host()` and `port()` to return bare values, use `expect_explicit()` or
pattern matching in the few places that require an explicit address rather than
making `Mdns` silently pretend to have a host.

### 4. Add mDNS send flags

Patch `SendArgs`:

- Add `pub mdns: bool`.
- Add `pub mdns_timeout_ms: u64`.

Suggested clap behavior:

```rust
#[arg(long)]
pub mdns: bool,

#[arg(long, default_value_t = 3_000, value_parser = parse_mdns_timeout_ms)]
pub mdns_timeout_ms: u64,
```

Add `parse_mdns_timeout_ms` near other parse helpers. Suggested range:

- Minimum: 1 ms, to reject meaningless zero-duration lookups.
- Maximum: 60,000 ms, to avoid accidental multi-minute startup hangs.

Error messages should name the flag and range.

### 5. Add mDNS recv flag

Patch `RecvArgs`:

```rust
#[arg(long)]
pub mdns: bool,
```

No timeout is needed on the receiver side; advertisement is held by RAII for the
duration of `run_recv`.

### 6. Rewrite send validation locally

Update `SendArgs::validate()` to short-circuit mDNS before host parsing.

Desired logic:

```text
if mdns && host is set:
    error: --mdns cannot be combined with --host

if mdns:
    validate mdns_timeout_ms via parser/clap
    return ResolvedTarget::Mdns { timeout_ms }

if host missing:
    error: pass --host <ip-or-name> or --mdns

validate/normalize explicit host and port as before
return ResolvedTarget::Explicit { host, port }
```

Preserve all existing validations unrelated to target selection:

- chunk duration
- sample rate
- channel count
- MTU
- capture source
- exact-capture-source interaction
- any current buffer/format constraints

### 7. Patch existing CLI tests

Update the existing tests that instantiate or inspect `ResolvedTarget`:

- Replace struct literals with `ResolvedTarget::explicit(...)` or enum literals.
- Replace direct field access with `host()`, `port()`, `to_socket_string()`, or
  pattern matching.
- Keep existing explicit-host behavior covered.

Then add focused tests for mDNS:

1. `send_validate_accepts_mdns_without_host`.
2. `send_validate_rejects_missing_host_without_mdns`.
3. `send_validate_rejects_mdns_and_host_together`.
4. `send_validate_returns_mdns_target_with_timeout`.
5. `parse_mdns_timeout_ms_rejects_zero`.
6. `parse_mdns_timeout_ms_rejects_too_large`.
7. `parse_mdns_timeout_ms_accepts_reasonable_value`.
8. `resolved_target_explicit_socket_string_matches_host_port`.
9. `resolved_target_mdns_has_no_socket_string_before_resolution`.
10. `recv_args_accepts_mdns_flag` if existing clap parse tests exist.
11. `send_mdns_does_not_require_port` if SRV port is authoritative.
12. `send_mdns_error_message_mentions_host_or_mdns` for the no-target case.

Keep tests close to existing CLI test style; do not introduce a new test helper
unless the file already uses one or the duplication becomes noisy.

### 8. Patch `run_send` in `src/main.rs`

Import discovery types as needed:

```rust
use teehee::discovery;
```

or use the crate's existing import style.

After `SendArgs::validate()` returns a `ResolvedTarget`, resolve it to a concrete
socket address before constructing the sender pipeline:

```rust
let target_addr = match target {
    ResolvedTarget::Explicit { host, port } => format!("{host}:{port}"),
    ResolvedTarget::Mdns { timeout_ms } => {
        let timeout = Duration::from_millis(timeout_ms);
        discovery::resolve_with_timeout(timeout)
            .map_err(|err| anyhow!(err))?
            .to_string()
    }
};
```

Adjust this to the actual sender constructor's expected type. If it currently
takes a string, keep passing a string. If it takes `SocketAddr`, prefer keeping
`SocketAddr` from discovery and resolving explicit host through the existing
path.

Important: do not alter the packet send loop, pacing, capture selection, or
format reconciliation in this slice.

### 9. Patch `run_recv` in `src/main.rs`

After the receiver bind succeeds, create and hold an advertiser when `--mdns` is
set:

```rust
let receiver = Receiver::bind(bind_addr)?;

let _advertiser = if args.mdns {
    Some(discovery::Advertiser::advertise(args.port, "teehee", "teehee.local.")?)
} else {
    None
};
```

Adjust names and error conversion to fit existing code.

If `Receiver::bind` returns or stores the actual bound local port, use that
instead of `args.port`; this matters if port `0` is ever allowed. If port `0` is
not allowed today, using `args.port` is acceptable.

Instance naming can be minimal for slice 12:

- instance: `teehee`
- host name: `teehee.local.`

If `mdns-sd` rejects duplicate full names when two receivers run on the same
LAN, handle that as a clean advertisement error and leave multi-receiver naming
for a later slice.

### 10. Error handling and messages

Sender discovery timeout should produce a user-facing message like:

```text
mDNS discovery timed out after 3000 ms for _teehee._udp.local. Is the receiver running with --mdns?
Try raising --mdns-timeout-ms or pass --host <ip> explicitly.
```

Receiver advertisement failures should say that UDP receive may be bound but
mDNS advertisement failed. If current `run_recv` startup treats all setup errors
as fatal, keep advertisement failure fatal for this slice; partial operation can
be a future enhancement.

### 11. Verification sequence

Run the narrowest useful checks first:

```bash
cargo test discovery --lib
cargo test spike_packet_ring --lib
cargo test cli --lib
cargo test --lib
cargo test
```

If time is short, minimum required verification for the wire-up is:

```bash
cargo test discovery --lib
cargo test cli --lib
cargo test --lib
```

Optional network-touching verification, run only when acceptable because it may
bind UDP multicast and trigger firewall prompts:

```bash
cargo test --lib discovery -- --ignored
```

Manual smoke test on two LAN machines:

Terminal A, receiver:

```bash
teehee recv --mdns --port 5000 --stats
```

Terminal B, sender:

```bash
teehee send --mdns --mdns-timeout-ms 5000
```

Fallback smoke test:

```bash
teehee send --host <receiver-ip> --port 5000
```

The fallback must continue to work exactly as before.

## Risks and mitigations

### `mdns-sd` API mismatch

Risk: the orphan `discovery.rs` may not compile against `mdns-sd = "0.7"` as
written.

Mitigation: wire the module first, run `cargo test discovery --lib`, and fix
only the compile errors in `discovery.rs`. Avoid changing CLI code until the
library module compiles.

### Windows firewall prompts

Risk: mDNS multicast socket creation may prompt on Windows or fail under locked
network policy.

Mitigation: keep network tests ignored by default; make runtime errors clear and
preserve `--host` as a no-mDNS fallback.

### Multiple receivers

Risk: sender returns the first receiver discovered, which may be surprising on a
LAN with multiple TeeHee receivers.

Mitigation: document this as slice-12 behavior. Receiver selection belongs with
pairing/identity work, not this slice.

### Premature advertisement

Risk: announcing before binding creates a false-positive receiver.

Mitigation: create `Advertiser` only after `Receiver::bind` succeeds.

### Large-file edit risk

Risk: full rewrites of `cli.rs` and `main.rs` can accidentally drop unrelated
behavior.

Mitigation: use small patches with local context; update tests as each section
changes; inspect `git diff` before running broad tests.

## Definition of done

Slice 12 is complete when:

- `mdns-sd = "0.7"` is in `Cargo.toml` and `Cargo.lock` is updated.
- `pub mod discovery;` is declared in `src/lib.rs` with a matching rustdoc
  entry.
- `teehee send --mdns` validates and resolves via `discovery::resolve_with_timeout`.
- `teehee recv --mdns` advertises only after the receiver socket binds.
- `teehee send --host <host>` still works without mDNS.
- `teehee send` without `--host` or `--mdns` fails with a clear error.
- `teehee send --host <host> --mdns` fails with a clear error.
- Focused discovery and CLI tests pass.
- At least `cargo test --lib` passes, or any remaining failures are documented
  as unrelated pre-existing failures.
