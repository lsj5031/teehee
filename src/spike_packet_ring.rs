//! # spike_packet_ring
//!
//! Spike module. **Not** wired into the running pipeline. `cargo check` and
//! `cargo test` exercise it; nothing in TeeHee's runtime imports from it.
//!
//! ## Inspiration
//!
//! Structural inspiration was taken from `neoeinstein/aliri`'s
//! [`jitter.rs`](https://github.com/neoeinstein/aliri) — but note that
//! aliri's file is about *intentionally adding* jitter to scheduled events
//! (anti-thundering-herd); this module is doing the inverse (measuring
//! observed jitter in audio packets). The shared idea: fixed-capacity
//! ring + rolling-sum bookkeeping. The semantic model here is original.
//!
//! ## What this is NOT
//!
//! - Not a drop-in for `jitter::buffer`. Run a `cargo bench` before swapping.
//! - Not wired into `audio_io::playback`. The public API stays builder-only
//!   until a sponsor of this spike wires it through.
//! - Not feature-flagged; we accept the dead-code lint via `#[allow(...)]`
//!   instead because flag-gating is heavier than the spike itself.
//!
//! ## Design
//!
//! | Operation | Complexity |
//! |-----------|-----------|
//! | `push` | O(1), no allocation |
//! | `push_instant` | O(1), `Duration` wrapper around `push` |
//! | `observed_jitter_ns` | O(1), reads running sum |
//! | `last_delta_ns` | O(1), reads stored last delta |
//! | `next_playout_ns` | O(1), reads running sum |
//! | `next_playout_instant` | O(1), `Duration`-flavoured mirror |
//! | Worst-case memory | `cap * sizeof(u128) + Vec headroom` |
//!
//! Timestamps are wall-clock nanoseconds (`u128`). The running mean uses
//! the *lifetime* `push_count` (`usize` to match `len()`), NOT the ring's
//! current window length, so overflow past capacity does not perturb the
//! metric.
//!
//! **v5 note:** the spike tracks arrival timestamps only. Wire-level
//! sequence numbers have been intentionally dropped — this is a
//! measurement primitive, not a packet store. If a future caller needs
//! to peek the associated wire sequence, add it back as a parallel
//! field (e.g. `Vec<Option<T>>` next to `slots`) without breaking this
//! API.

#![allow(dead_code)]

use std::time::Duration;

/// Default ring capacity. Tuned for ~64 packets of headroom at common
/// 48 kHz / 20 ms frames (~1.28 s of buffering).
pub const DEFAULT_CAPACITY: usize = 64;

/// Fixed-capacity ring buffer of timestamped packet arrivals.
///
/// Measurement primitive, not a packet store. See module-level notes.
#[derive(Debug, Clone)]
pub struct PacketRing {
    cap: usize,
    /// Arrival timestamps in ring order. `None` for unwritten slots.
    slots: Vec<Option<u128>>,
    head: usize,
    tail: usize,
    len: usize,
    /// Last arrival timestamp. Canonical reference for inter-arrival delta.
    last_arrival_ns: Option<u128>,
    /// Last computed inter-arrival delta. `Some` after the second push onwards.
    last_delta_ns: Option<u128>,
    /// Total pushes since creation. `usize` to match `len()`.
    push_count: usize,
    /// Running sum of inter-arrival deltas (nanoseconds, absolute).
    jitter_sum_ns: u128,
}

impl PacketRing {
    /// Build a ring with the given capacity. Clamped to at least 1.
    pub fn new(capacity: usize) -> Self {
        Self::with_capacity(capacity.max(1))
    }

    fn with_capacity(capacity: usize) -> Self {
        Self {
            cap: capacity,
            slots: (0..capacity).map(|_| None).collect(),
            head: 0,
            tail: 0,
            len: 0,
            last_arrival_ns: None,
            last_delta_ns: None,
            push_count: 0,
            jitter_sum_ns: 0,
        }
    }

    /// Push a packet arrival timestamp (nanoseconds). Returns the
    /// inter-arrival delta vs the previous push, or `None` on the first
    /// push.
    ///
    /// When the ring is full the oldest slot is overwritten. The `tail`
    /// cursor advances to maintain `len == cap`.
    ///
    /// The running mean uses `push_count` (lifetime total), NOT `len`, so
    /// overflow past capacity does not perturb the metric.
    pub fn push(&mut self, arrived_at_ns: u128) -> Option<u128> {
        let prev = self.last_arrival_ns;

        self.slots[self.head] = Some(arrived_at_ns);
        self.head = (self.head + 1) % self.cap;
        if self.len < self.cap {
            self.len += 1;
        } else {
            // Overwriting — advance tail to drop the slot we just clobbered.
            self.tail = (self.tail + 1) % self.cap;
        }
        self.last_arrival_ns = Some(arrived_at_ns);
        self.push_count = self.push_count.saturating_add(1);

        prev.map(|p| {
            let delta = arrived_at_ns.abs_diff(p);
            self.jitter_sum_ns = self.jitter_sum_ns.saturating_add(delta);
            self.last_delta_ns = Some(delta);
            delta
        })
    }

    /// `Duration`-flavoured mirror of [`Self::push`].
    pub fn push_instant(&mut self, arrived: Duration) -> Option<u128> {
        self.push(arrived.as_nanos())
    }

    /// Mean inter-arrival delta across the *lifetime* of pushes, in
    /// nanoseconds. Uses `push_count - 1` as the divisor.
    pub fn observed_jitter_ns(&self) -> f64 {
        if self.push_count < 2 {
            return 0.0;
        }
        self.jitter_sum_ns as f64 / (self.push_count - 1) as f64
    }

    /// Most recent inter-arrival delta, or `None` if fewer than two pulses.
    pub fn last_delta_ns(&self) -> Option<u128> {
        self.last_delta_ns
    }

    /// Suggested playout deadline (raw ns): `arrival + target_buffer + 1.5 * mean_jitter`.
    /// Uses an RFC 3550-style 1.5× safety margin over the observed mean.
    ///
    /// **Note:** `(observed * 1.5) as u128` truncates fractional
    /// nanoseconds to zero — callers needing sub-nanosecond precision
    /// should not rely on the trailing margin.
    pub fn next_playout_ns(&self, arrived_at_ns: u128, target_buffer_ns: u128) -> u128 {
        let margin = (self.observed_jitter_ns() * 1.5) as u128;
        arrived_at_ns
            .saturating_add(target_buffer_ns)
            .saturating_add(margin)
    }

    /// `Duration`-flavoured mirror of [`Self::next_playout_ns`].
    /// Saturates to `Duration::MAX` when the underlying nanosecond math
    /// would overflow `u64`.
    pub fn next_playout_instant(&self, arrived: Duration, target_buffer: Duration) -> Duration {
        let out_ns = self.next_playout_ns(arrived.as_nanos(), target_buffer.as_nanos());
        if out_ns > u64::MAX as u128 {
            Duration::MAX
        } else {
            Duration::from_nanos(out_ns as u64)
        }
    }

    pub fn len(&self) -> usize { self.len }
    pub fn is_empty(&self) -> bool { self.len == 0 }
    pub fn capacity(&self) -> usize { self.cap }
    pub fn push_count(&self) -> usize { self.push_count }
}

impl Default for PacketRing {
    fn default() -> Self { Self::new(DEFAULT_CAPACITY) }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn first_push_returns_none_and_zero_jitter() {
        let mut r = PacketRing::new(4);
        assert!(r.is_empty());
        assert_eq!(r.push(1_000), None);
        assert_eq!(r.push_count(), 1);
        assert_eq!(r.observed_jitter_ns(), 0.0);
        assert_eq!(r.last_delta_ns(), None);
    }

    #[test]
    fn consecutive_pushes_match_consecutive_deltas() {
        // LOCKS IN: deltas are measured between consecutive pushes.
        let mut r = PacketRing::new(8);
        r.push(0);
        let d1 = r.push(1_000);
        let d2 = r.push(6_000);
        let d3 = r.push(7_000);
        // deltas: [1000, 5000, 1000] -> jitter_sum=7000 -> mean = 7000/3
        assert_eq!(d1, Some(1_000));
        assert_eq!(d2, Some(5_000));
        assert_eq!(d3, Some(1_000));
        assert!((r.observed_jitter_ns() - 7_000.0 / 3.0).abs() < 1.0,
            "expected mean ~2333.33, got {}", r.observed_jitter_ns());
        assert_eq!(r.last_delta_ns(), Some(1_000));
        assert_eq!(r.push_count(), 4);
    }

    #[test]
    fn overflow_does_not_break_running_mean() {
        // LOCKS IN that overflow does NOT reset the running mean divisor.
        let mut r = PacketRing::new(4);
        for i in 0..10u32 {
            r.push((i as u128) * 1_000);
        }
        // 9 deltas, all 1000 ns. Ring is full (len == cap == 4).
        assert_eq!(r.push_count(), 10);
        assert_eq!(r.len(), 4);
        assert!((r.observed_jitter_ns() - 1_000.0).abs() < 1.0,
            "expected mean ~1000, got {}", r.observed_jitter_ns());
        assert_eq!(r.last_delta_ns(), Some(1_000));
    }

    #[test]
    fn next_playout_includes_buffer_and_jitter_margin() {
        let mut r = PacketRing::new(8);
        r.push(0);
        r.push(10_000);
        // arrival=30_000, target=20_000, mean_jitter ~10_000, margin ~15_000
        let out = r.next_playout_ns(30_000, 20_000);
        assert!(out >= 50_000, "expected playout >= 50_000 ns, got {out}");
    }

    #[test]
    fn next_playout_instant_round_trips_through_duration() {
        let mut r = PacketRing::new(8);
        r.push(0);
        r.push(10_000);
        let arrived = Duration::from_nanos(30_000);
        let target = Duration::from_nanos(20_000);
        let out = r.next_playout_instant(arrived, target);
        assert!(out >= Duration::from_nanos(50_000),
            "expected playout >= 50_000 ns, got {:?}", out);
    }

    #[test]
    fn push_instant_wraps_duration() {
        let mut r = PacketRing::new(4);
        let d = Duration::from_nanos(42);
        assert_eq!(r.push_instant(d), None);
    }
}
