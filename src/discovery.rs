//! `discovery` — mDNS auto-discovery for TeeHee senders and receivers.
//!
//! Slice 12 (Tier 3 follow-up): LAN UX-friction reduction — instead
//! of typing `teehee send --host 192.168.0.42`, senders can `--mdns`
//! and the binary resolves `_teehee._udp.local` on the LAN.
//! Receivers opt in to advertising themselves with the same flag.
//!
//! ## Wire format
//!
//! * Service type: `_teehee._udp.local.` (RFC 6763 — `_svc._proto.local.`).
//! * TXT record: `v=1` (protocol version, sparse on purpose —
//!   richer metadata such as sample rate, channels, and codec
//!   lives in the first packet's fixed header).
//! * Pairing / encryption: out of scope here. This layer only
//!   resolves an IP+port; identity checks happen above.
//!
//! ## Browse window (sender side)
//!
//! [`resolve_with_timeout`] blocks at most `timeout` and returns
//! the first `_teehee._udp.local` SRV/A record that resolves to an
//! IPv4 address. The browse is single-shot — no retry. The
//! receiver's mDNS advertisement is assumed to be running when
//! the sender reaches this function; if you race receiver startup,
//! raise `--mdns-timeout-ms`.
//!
//! ## Advertise lifetime (receiver side)
//!
//! [`Advertiser`] is RAII: registers on construction, deregisters
//! and shuts down the mDNS daemon on `Drop`. The receiver holds
//! it for `run_recv`'s lifetime; the drop at function exit happens
//! after `Receiver::bind` cleanup, so a clean shutdown path is
//! automatic.
//!
//! ## Why mdns-sd 0.7
//!
//! Stable Rust, MSRV 1.56, no nightly features. Backs onto the
//! cross-platform mDNS multicast group (224.0.0.251 on IPv4 —
//! macOS, Linux, and Windows all join the same group). The
//! `ServiceDaemon::new()` call opens a real UDP socket to that
//! multicast group, which is what makes `#[ignore]` necessary
//! for the network-touching tests below.

use std::collections::HashMap;
use std::net::{SocketAddr, SocketAddrV4};
use std::time::{Duration, Instant};

use mdns_sd::{ServiceDaemon, ServiceEvent, ServiceInfo};

/// mDNS service type for TeeHee receivers. RFC 6763 service name
/// (underscore-prefixed) + UDP transport + `.local.` domain. The
/// trailing dot is significant — mdns-sd appends it internally if
/// missing, but we include it explicitly to document the standard.
const SERVICE_TYPE: &str = "_teehee._udp.local.";

/// TXT record key for the protocol version. Sparse on purpose:
/// `v=1` is the only key we ship today; richer metadata (sample
/// rate, channels, codec) lives in the first packet's fixed header.
const TXT_VERSION_KEY: &str = "v";

/// TXT record value for the protocol version. Bumped only on
/// wire-incompatible changes; new optional fields can be added
/// without a version bump.
const TXT_VERSION_VALUE: &str = "1";

/// One-shot convenience for the sender side: open a daemon, browse
/// `SERVICE_TYPE` for `timeout`, return the first IPv4 SocketAddr
/// resolved.
///
/// On timeout (no SRV record seen within the window) returns
/// `ResolutionError::Timeout { timeout }`. All other mDNS errors
/// collapse to `ResolutionError::Daemon` so the caller can surface
/// a single, concise error string at the CLI boundary.
///
/// **Design note:** the function is synchronous and intended to be
/// called from a binary's startup path. It's NOT intended to be
/// invoked from a hot audio thread — a 3-second timeout would
/// block playback for that duration. teehee's call site is
/// `run_send` *before* any packet is shipped, so the contract
/// holds.
pub fn resolve_with_timeout(timeout: Duration) -> Result<SocketAddr, ResolutionError> {
    // RAII wrapper so the daemon is always shut down before return
    // — even on the early-error path or a panic-from-deep.
    struct DaemonGuard(ServiceDaemon);
    impl Drop for DaemonGuard {
        fn drop(&mut self) {
            let _ = self.0.shutdown();
        }
    }

    let daemon = ServiceDaemon::new()
        .map_err(|e| ResolutionError::Daemon(e.to_string()))?;
    let daemon = DaemonGuard(daemon);
    let receiver = daemon
        .0
        .browse(SERVICE_TYPE)
        .map_err(|e| ResolutionError::Daemon(e.to_string()))?;
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        let remaining = deadline.saturating_duration_since(Instant::now());
        // Cap the per-receive wait at min(100ms, remaining) so the
        // loop exits sharply when timeout elapses instead of
        // waiting the full recv_timeout window.
        let wait = remaining.min(Duration::from_millis(100));
        match receiver.recv_timeout(wait) {
            Ok(event) => {
                if let Some(addr) = first_ipv4_from_event(event) {
                    return Ok(addr);
                }
                // Other event shapes (SearchStopped, ServiceFound
                // without resolution yet) — keep looping.
            }
            Err(_e) => {
                // recv_timeout returns Err on timeout. Loop checks
                // the deadline at the top, so natural exit happens
                // on the next iteration's `Instant::now() < deadline`.
                if Instant::now() >= deadline {
                    break;
                }
            }
        }
    }
    Err(ResolutionError::Timeout { timeout })
}

fn first_ipv4_from_event(event: ServiceEvent) -> Option<SocketAddr> {
    if let ServiceEvent::ServiceResolved(info) = event {
        for addr in info.get_addresses() {
            return Some(SocketAddr::V4(SocketAddrV4::new(
                *addr,
                info.get_port(),
            )));
        }
    }
    None
}

/// RAII advertiser: registers a `_teehee._udp.local.` service when
/// constructed and unregisters when dropped.
///
/// The `instance` label (e.g. `"teehee-recv"`) and `host` label
/// (e.g. `"teehee.local."`) appear in `.local` listings on other
/// LAN hosts. The `host` label should be a `.local.`-terminated
/// form; if missing the trailing dot, mdns-sd appends it
/// automatically per RFC 6762.
pub struct Advertiser {
    daemon: ServiceDaemon,
    fullname: String,
}

impl Advertiser {
    /// Advertise `_teehee._udp.local.` on `port`. The daemon is
    /// started immediately and the service is registered before
    /// this function returns.
    ///
    /// `instance` becomes part of the SRV-record fullname
    /// (`<instance>._teehee._udp.local.`). `host` becomes the
    /// A-record label (e.g. `"myhost.local."`).
    ///
    /// Errors are typed via [`AdvertiseError`] so the caller can
    /// surface a clean CLI error rather than a raw `mdns_sd::Error`.
    pub fn advertise(
        port: u16,
        instance: &str,
        host: &str,
    ) -> Result<Self, AdvertiseError> {
        let daemon = ServiceDaemon::new()
            .map_err(|e| AdvertiseError::Daemon(e.to_string()))?;
        let mut properties = HashMap::new();
        properties.insert(
            TXT_VERSION_KEY.to_string(),
            TXT_VERSION_VALUE.to_string(),
        );
        let service_info = ServiceInfo::new(
            SERVICE_TYPE,
            instance,
            host,
            "", // address auto-populated by daemon from host interfaces
            port,
            Some(properties),
        )
        .map_err(|e| AdvertiseError::ServiceInfo(e.to_string()))?;
        let fullname = service_info.get_fullname().to_string();
        daemon
            .register(service_info)
            .map_err(|e| AdvertiseError::Register(e.to_string()))?;
        Ok(Self { daemon, fullname })
    }

    /// The SRV-record fullname of the registered service, e.g.
    /// `my-instance._teehee._udp.local.`. Useful for logging and
    /// for the unit test that pins the format.
    pub fn fullname(&self) -> &str {
        &self.fullname
    }

    /// Explicit shutdown. Not strictly necessary — Drop calls the
    /// same shutdown — but exposed so a caller can deregister
    /// mid-run without dropping the whole struct (e.g. to swap
    /// port after a successful announce).
    pub fn shutdown(self) -> Result<(), AdvertiseError> {
        self.daemon
            .shutdown()
            .map_err(|e| AdvertiseError::Shutdown(e.to_string()))
    }
}

impl Drop for Advertiser {
    fn drop(&mut self) {
        // Best-effort: log if shutdown fails but don't panic in Drop.
        // The receiver's run_recv drops this on shutdown; an extra
        // debug-level line is OK to leave in production.
        tracing::debug!(fullname = %self.fullname, "mDNS advertiser dropping");
        let _ = self.daemon.shutdown();
    }
}

/// Errors from [`resolve_with_timeout`].
#[derive(Debug, thiserror::Error)]
pub enum ResolutionError {
    #[error(
        "mDNS discovery timed out after {ms} ms for _teehee._udp.local. \
         Is the receiver running with --mdns? \
         Try raising --mdns-timeout-ms or pass --host <ip> explicitly.",
        ms = timeout.as_millis()
    )]
    Timeout { timeout: Duration },
    #[error("mDNS daemon error: {0}")]
    Daemon(String),
}

/// Errors from [`Advertiser::advertise`] / [`Advertiser::shutdown`].
#[derive(Debug, thiserror::Error)]
pub enum AdvertiseError {
    #[error("mDNS daemon error: {0}")]
    Daemon(String),
    #[error("invalid service info: {0}")]
    ServiceInfo(String),
    #[error("registration error: {0}")]
    Register(String),
    #[error("shutdown error: {0}")]
    Shutdown(String),
}

#[cfg(test)]
mod tests {
    //! The pure-std surface (constants, error Display, helper
    //! functions) is testable without touching the network. The
    //! network-touching tests (`advertiser_*`) are gated behind
    //! `#[ignore]` because they bind a real UDP multicast socket on
    //! 224.0.0.251:5353 — which on Windows can trigger a firewall
    //! prompt and on Linux/macOS could race other concurrently
    //! running mDNS daemons. Run on demand with:
    //!
    //! ```bash
    //! cargo test --lib discovery -- --ignored
    //! ```
    use super::*;

    // ----- pure-std surface -----

    #[test]
    fn service_type_constant_matches_rfc6763_form() {
        // _<svc>._<proto>.<domain>. Pin this so a future refactor
        // that drops the leading underscore or the `.local.`
        // domain surfaces immediately in unit tests, before
        // integration.
        assert!(SERVICE_TYPE.starts_with('_'));
        assert!(SERVICE_TYPE.ends_with(".local."));
        assert!(SERVICE_TYPE.contains("._udp."));
    }

    #[test]
    fn resolution_error_timeout_display_includes_ms() {
        // The Display impl names the timeout so the operator can
        // correlate the error with their --mdns-timeout-ms.
        let err = ResolutionError::Timeout {
            timeout: Duration::from_millis(3_000),
        };
        let s = format!("{err}");
        assert!(s.contains("3000"), "display must name 3000 ms; got: {s}");
        assert!(
            s.contains("timed out"),
            "display must say 'timed out'; got: {s}"
        );
        assert!(
            s.contains("_teehee._udp.local"),
            "display must name the service type; got: {s}"
        );
    }

    #[test]
    fn resolution_error_daemon_display_includes_message() {
        let err = ResolutionError::Daemon("bind failed".into());
        let s = format!("{err}");
        assert!(
            s.contains("bind failed"),
            "display must include inner msg; got: {s}"
        );
    }

    #[test]
    fn advertise_error_variants_distinct_display() {
        // Each variant Display must include the inner message —
        // otherwise users can't tell which step failed.
        let daemon = AdvertiseError::Daemon("e1".into());
        let info = AdvertiseError::ServiceInfo("e2".into());
        let register = AdvertiseError::Register("e3".into());
        let shutdown = AdvertiseError::Shutdown("e4".into());
        assert!(format!("{daemon}").contains("e1"));
        assert!(format!("{info}").contains("e2"));
        assert!(format!("{register}").contains("e3"));
        assert!(format!("{shutdown}").contains("e4"));
    }

    #[test]
    fn first_ipv4_from_event_ignores_non_resolved_events() {
        // ServiceFound resolves later — should NOT return an
        // address. We're forced to special-case non-`ServiceResolved`
        // events because the resolve event is when mdns-sd has
        // gathered enough info to populate addresses.
        let addr =
            first_ipv4_from_event(ServiceEvent::SearchStopped(SERVICE_TYPE.to_string()));
        assert!(
            addr.is_none(),
            "SearchStopped must not return an address"
        );
    }

    // ----- network-touching (ignored by default) -----

    #[test]
    #[ignore = "binds UDP multicast 224.0.0.251:5353; run with `cargo test --lib discovery -- --ignored`"]
    fn advertiser_fullname_format_is_dotted_local() {
        // The fullname must end in `._teehee._udp.local.` — the
        // service-type constant concatenated to the instance name.
        let adv = Advertiser::advertise(5000, "test-instance", "test-host.local.")
            .expect("advertiser must accept a valid port");
        let fullname = adv.fullname().to_string();
        assert!(
            fullname.ends_with("._teehee._udp.local."),
            "fullname should end with the service-type constant; got: {fullname}"
        );
        assert!(
            fullname.starts_with("test-instance."),
            "fullname should start with the instance label; got: {fullname}"
        );
        adv.shutdown().expect("shutdown must succeed");
    }

    #[test]
    #[ignore = "binds UDP multicast 224.0.0.251:5353; run with `cargo test --lib discovery -- --ignored`"]
    fn advertiser_double_register_with_distinct_instances_succeeds() {
        // Two advertisers on the same `_teehee._udp.local.`
        // service-type but with different instance labels must
        // both register cleanly. mdns-sd dedupes by fullname;
        // distinct instance labels are different SRV records.
        let adv1 = Advertiser::advertise(5000, "instance-a", "teehee.local.")
            .expect("first advertiser must register");
        let adv2 = Advertiser::advertise(5001, "instance-b", "teehee.local.")
            .expect("second advertiser with distinct instance must register");
        assert_ne!(adv1.fullname(), adv2.fullname());
        adv1.shutdown().unwrap();
        adv2.shutdown().unwrap();
    }
}
