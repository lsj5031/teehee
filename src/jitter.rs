//! `jitter` — receive-side reorder buffer for teehee audio packets.
//!
//! The buffer holds a fixed number of packet-sized sample slots in a ring
//! indexed by `seq % capacity_packets`. Until the receiver has seen at
//! least one packet, the buffer is in **prebuffer mode**: `pop_frames`
//! emits silence and the buffer does not advance a play head. Once a
//! packet has been pushed, `pop_frames` anchors the play head at the
//! smallest sequence number seen and starts emitting samples in cyclic
//! `seq` order.
//!
//! ## Partial-packet playout (v2 semantics)
//!
//! After slice 2, `pop_frames` consumes any number of f32 samples —
//! including `out.len() < samples_per_packet`. State tracks
//! `(head, head_offset)`: `head` is the next seq to play; `head_offset`
//! counts samples already written from that packet so a single packet
//! can drain across multiple cpal callbacks (which are commonly 256 /
//! 512 / 1024 f32 frames while packets in v1 are 1920 f32 frames at
//! default 48 kHz stereo / chunk_ms=20). When `head_offset` reaches
//! `samples_per_packet` the slot is cleared and `head` advances.
//! Missing slots emit silence and the silence is counted once per
//! full packet's worth (not per frame).
//!
//! ## Slice 6 — prebuffer gate
//!
//! `pop_frames` checks `queued_frames()` against an optional
//! `prebuffer_target_frames` before anchoring `head`. If the gate is
//! set (e.g. main.rs computes `target_frames = (prebuffer_ms *
//! sample_rate * channels) / 1000` from `--prebuffer-ms` after the
//! first packet reveals the format), `pop_frames` stays in silence
//! prebuffer until queued fill crosses the target — even after
//! packets have been pushed. This expresses the operator's intent
//! "wait this long before playing" in output sample-rate frames so
//! it matches cpal callback semantics. With a target set,
//! `prebuffer_holds` ticks once per `pop_frames` call that hits the
//! gate; it drops to zero the moment playback starts.
//!
//! ## Push-side invariants
//!
//! * `seq <= head` (modular) → `Late` (already played or playing).
//! * `seq > head` and `slot[idx].seq == seq` → `Duplicate`.
//! * `seq > head` and the slot's stored seq is mid-read
//!   (`target_idx == head_idx` AND `head_offset > 0`) →
//!   `MidReadCollision` (refused; the active slot is not overwritable
//!   because cpal is currently draining it).
//! * `seq > head` and slot is stale or empty → store.
//!
//! Sequence-number comparison is fully modular — a packet with
//! `seq = u32::MAX` is treated as one step "ahead" of `seq = u32::MAX - 1`
//! across the wrap boundary.
//!
//! ## Thread safety
//!
//! `JitterBuffer` is `!Send` / `!Sync` (no `unsafe impl`). Cross-thread
//! access must be guarded by a `Mutex` (the receiver pipeline in
//! `main.rs` does this with `Arc<Mutex<Option<JitterBuffer>>>`). The
//! cpal data callback and the recv thread are mutually exclusive in
//! time, so `push` (recv thread) cannot race with `pop_frames`
//! (cpal audio thread).

/// Outcome of a [`JitterBuffer::push`] call.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PushOutcome {
    /// Packet stored in the ring buffer.
    Stored,
    /// Same sequence number was already buffered (and not yet played).
    Duplicate,
    /// Sequence number is at or before the current play head — too late.
    Late,
    /// Sequence number landed in the slot the cpal callback is currently
    /// draining (modular collision with `head_idx`, while
    /// `head_offset > 0`). Refused to avoid mid-playback corruption.
    /// Real LAN packets landing here are extremely rare — typically
    /// only a long-burred late-out-of-order packet with
    /// `push.seq - head >= capacity_packets`.
    MidReadCollision,
    /// Sender restarted: the incoming seq is far behind the play head
    /// (gap > capacity_packets), so the jitter buffer was reset and
    /// the packet was stored as the new anchor.
    StreamReset,
}
/// Cumulative diagnostics from the jitter buffer.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct Stats {
    pub late_drops: u64,
    pub duplicates: u64,
    pub silence_insertions: u64,
    /// Number of times `push` refused to overwrite a slot currently
    /// being read by an in-flight cpal callback. Non-zero means the
    /// sender's `seq - receiver_head` exceeded `capacity_packets` —
    /// raise `--prebuffer-ms` to grow the ring's capacity multiplier
    /// or shrink the sender's `--chunk-ms`.
    pub mid_read_collisions: u64,
    /// Slice 6 (prebuffer gate): number of times `pop_frames` emitted
    /// silence because `queued_frames()` was below the operator's
    /// `--prebuffer-ms` translated to `prebuffer_target_frames`.
    /// Each `pop_frames` call that hits the gate increments this by
    /// one; with `--stats` enabled, this counter drops to zero the
    /// moment `queued_frames` crosses the target and playback starts.
    pub prebuffer_holds: u64,
    /// Slice 10 (receiver back-pressure): number of times `push`
    /// silently overwrote a slot whose contents were an unplayed
    /// future sequence (`seq > head` AND
    /// `slot.seq != push.seq`). This is the canonical "ring was too
    /// small for the sender's burst rate" signature: the sender's
    /// `seq - receiver_head` reached `capacity_packets` and the
    /// previous occupant of the modular slot had not yet been
    /// played. Distinct from [`Stats::mid_read_collisions`] (which
    /// only fires when the cpal callback is currently mid-drain of
    /// the colliding slot — a narrower, rarer event). Operators
    /// spot sender over-runs on the `--stats` line; remediation is
    /// `raise --rx-buffer-ms` to grow the ring's `capacity_packets`,
    /// or `lower --chunk-ms` on the sender to reduce the burst rate.
    pub ring_overruns: u64,
    /// Number of times the jitter buffer detected a sender restart
    /// (incoming seq far behind the play head) and reset its state,
    /// anchoring on the new stream. Non-zero means the sender process
    /// was stopped and re-launched (or the network path changed).
    pub sender_restarts: u64,
    /// Number of whole packets skipped to bring fill back under the
    /// high-water mark (adaptive latency trim). Non-zero means the
    /// receiver discarded audio to prevent lag creep past the target.
    pub latency_trims: u64,
    /// Number of times the jitter buffer detected an underrun (ring
    /// empty mid-playback) and froze the play head, reverting to
    /// prebuffer mode. The receiver re-buffers to `--prebuffer-ms`
    /// instead of advancing through silence indefinitely. A non-zero
    /// value on `--stats` means a network starvation or OS scheduling
    /// stall occurred; playback resumes automatically once enough
    /// packets arrive.
    pub underrun_resyncs: u64,
}

struct Slot {
    seq: u32,
    samples: Vec<f32>,
    filled: bool,
}

/// A deep module: the public API is `new`, `push`, `pop_frames`,
/// `queued_frames`, and `stats`. All ordering, modulo-arithmetic, and
/// silence-padding logic is hidden behind this small surface.
pub struct JitterBuffer {
    samples_per_packet: usize,
    capacity_packets: usize,
    slots: Vec<Slot>,
    /// Next sequence to play. `None` while in prebuffer mode.
    head: Option<u32>,
    /// Samples already consumed from the current `head` packet.
    /// 0..=samples_per_packet. Resets to 0 when `head` advances.
    head_offset: usize,
    /// Smallest seq we've seen during prebuffer — used to anchor `head`.
    min_pushed: Option<u32>,
    /// Slice 6: optional prebuffer gate. `None` means the receiver
    /// anchors on the first packet (legacy behaviour); `Some(N)`
    /// means `pop_frames` must hold until `queued_frames() >= N`
    /// before anchoring. Set by the caller (typically main.rs)
    /// after the first packet reveals `sample_rate × channels`,
    /// which together with the operator's `--prebuffer-ms` define
    /// the gate target.
    prebuffer_target_frames: Option<usize>,
    /// When set together with [`Self::prebuffer_target_frames`],
    /// `pop_frames` trims (skips whole packets) if fill exceeds this
    /// high-water, down toward the prebuffer target. Prevents lag
    /// from climbing toward `capacity_packets` under clock skew /
    /// bursts.
    high_water_frames: Option<usize>,
    /// O(1) fill tracker: number of slots with `filled == true`.
    /// Maintained by `push` (increment on store into empty slot)
    /// and `pop_frames` / `trim_latency_if_needed` (decrement on
    /// slot clear). Used by [`Self::queued_frames`] to avoid an
    /// O(capacity) scan on every cpal callback.
    filled_count: usize,
    /// Floor sequence for post-underrun re-anchor. When
    /// `pop_frames` detects an underrun (missing head slot with
    /// empty ring), it freezes the play head and records the
    /// current `head` seq as `resync_floor`. Subsequent pushes
    /// while in prebuffer reject seq strictly earlier than this
    /// floor, so the re-anchor never replays already-played audio.
    /// Cleared by [`Self::reset_state`].
    resync_floor: Option<u32>,
    stats: Stats,
}

impl JitterBuffer {
    /// Build a buffer with `samples_per_packet` interleaved-f32 samples
    /// per packet, `capacity_packets` packet-sized ring slots, and an
    /// optional prebuffer gate (`prebuffer_target_frames`).
    ///
    /// All three values are non-zero when present. Pass `None` for the
    /// gate to disable it (legacy behaviour: anchor on first packet).
    pub fn new(
        samples_per_packet: usize,
        capacity_packets: usize,
        prebuffer_target_frames: Option<usize>,
    ) -> Self {
        Self::with_high_water(
            samples_per_packet,
            capacity_packets,
            prebuffer_target_frames,
            None,
        )
    }

    /// Like [`Self::new`] but with an explicit high-water trim target
    /// (interleaved f32 samples, same units as `queued_frames`).
    pub fn with_high_water(
        samples_per_packet: usize,
        capacity_packets: usize,
        prebuffer_target_frames: Option<usize>,
        high_water_frames: Option<usize>,
    ) -> Self {
        assert!(samples_per_packet > 0, "samples_per_packet must be > 0");
        assert!(capacity_packets > 0, "capacity_packets must be > 0");
        if let Some(t) = prebuffer_target_frames {
            assert!(t > 0, "prebuffer_target_frames must be > 0 when set");
        }
        if let Some(h) = high_water_frames {
            assert!(h > 0, "high_water_frames must be > 0 when set");
        }
        let mut slots = Vec::with_capacity(capacity_packets);
        for _ in 0..capacity_packets {
            slots.push(Slot {
                seq: 0,
                samples: vec![0.0; samples_per_packet],
                filled: false,
            });
        }
        Self {
            samples_per_packet,
            capacity_packets,
            slots,
            head: None,
            head_offset: 0,
            min_pushed: None,
            prebuffer_target_frames,
            high_water_frames,
            filled_count: 0,
            resync_floor: None,
            stats: Stats::default(),
        }
    }

    /// Reset the jitter buffer to prebuffer state, clearing all slots
    /// and discarding the play head. Called on sender restart detection.
    fn reset_state(&mut self) {
        self.head = None;
        self.head_offset = 0;
        self.min_pushed = None;
        self.resync_floor = None;
        self.filled_count = 0;
        for slot in &mut self.slots {
            slot.filled = false;
            slot.seq = 0;
        }
    }

    /// Samples per packet (interleaved f32 samples). Exposed for the
    /// recv-side callback to size scratch buffers without reaching
    /// into the jitter buffer's internals.
    pub fn samples_per_packet(&self) -> usize {
        self.samples_per_packet
    }

    /// Insert a packet. Returns the classification for diagnostics.
    ///
    /// `samples.len()` must equal `samples_per_packet`. The caller is
    /// expected to enforce this; the buffer asserts in debug builds.
    pub fn push(&mut self, seq: u32, samples: &[f32]) -> PushOutcome {
        debug_assert_eq!(
            samples.len(),
            self.samples_per_packet,
            "samples.len() must match samples_per_packet"
        );
        let idx = (seq as usize) % self.capacity_packets;

        // Prebuffer resync-floor guard: while in prebuffer
        // (head.is_none()), reject seq strictly earlier than the
        // resync_floor so a post-underrun re-anchor never replays
        // already-played audio. "Strictly earlier" means
        // `modular_is_earlier_or_equal(seq, floor) && seq != floor`.
        //
        // R1 FIX: the resync-floor guard must also detect sender
        // restarts. When the sender relaunches, seq resets to 0
        // (or a small value) while resync_floor is still high.
        // Without this escape, every post-restart packet is
        // rejected as Late forever. The gap > capacity_packets
        // check mirrors the sender-restart detection in the
        // head.is_some() branch below.
        if self.head.is_none() {
            if let Some(floor) = self.resync_floor {
                if modular_is_earlier_or_equal(seq, floor) && seq != floor {
                    let gap = floor.wrapping_sub(seq);
                    if gap > self.capacity_packets as u32 {
                        // Sender restarted — clear resync_floor and
                        // allow the packet through as a new anchor.
                        self.resync_floor = None;
                        self.stats.sender_restarts += 1;
                        // Fall through — store below.
                    } else {
                        self.stats.late_drops += 1;
                        return PushOutcome::Late;
                    }
                }
            }
        }

        // Late check and sender-restart detection.
        // BUG-1 FIX: `seq == head` with `head_offset == 0` is no
        // longer treated as Late. When the ring is empty and the
        // play head points at a cleared slot, the sender's next
        // packet legitimately has `seq == head` — rejecting it
        // would permanently lock the buffer into silence.
        if let Some(head) = self.head {
            let fwd = head.wrapping_sub(seq);
            if modular_is_earlier_or_equal(seq, head) && (fwd > 0 || self.head_offset > 0) {
                // If the gap from seq to head is larger than the
                // ring capacity, this isn't normal jitter — the
                // sender likely restarted (seq reset to 1 while
                // the receiver's play head advanced far ahead).
                // Reset the buffer and store as the new anchor.
                if fwd > self.capacity_packets as u32 {
                    self.reset_state();
                    self.stats.sender_restarts += 1;
                    // Fall through — store the packet below
                    // (which will set min_pushed in prebuffer mode).
                } else {
                    self.stats.late_drops += 1;
                    return PushOutcome::Late;
                }
            }

            // Mid-read slot collision: if the slot the new seq would
            // land in is the slot the audio callback is currently
            // draining (same modular index, partial consumption),
            // refuse. Overwriting here would corrupt the active
            // playback with the future seq's samples.
            let head_idx = (head as usize) % self.capacity_packets;
            if idx == head_idx && self.head_offset > 0 {
                self.stats.mid_read_collisions += 1;
                return PushOutcome::MidReadCollision;
            }
        }

        if self.slots[idx].filled && self.slots[idx].seq == seq {
            self.stats.duplicates += 1;
            return PushOutcome::Duplicate;
        }

        // Slice 10 (receiver back-pressure): detect a true ring
        // overrun *before* overwriting. Distinguishes from
        // MidReadCollision (which only fires when the cpal callback
        // is mid-drain of the colliding slot):
        //
        //   * Slot is currently filled.
        //   * Slot's stored seq is at-or-forward of the play head
        //     (cyclically, within ring capacity) — i.e. the audio
        //     in this slot would have been played (or is about to
        //     be played this next pop). The "==head" case is the
        //     just-cleared-then-re-pushed race window: slot cleared
        //     by pop_frames, head advanced, then sender wraps and
        //     re-fills the now-empty slot BEFORE the next pop. With
        //     `head_offset == 0`, MidReadCollision is silent —
        //     this counter is the only way the operator sees it.
        //
        // The "at-or-forward within capacity" window is computed
        // directly via `stored_seq.wrapping_sub(head) ∈ [0, capacity)`
        // — a strict forward distance check that doesn't rely on
        // the half-cycle symmetry of [`modular_is_earlier_or_equal`]
        // and so correctly excludes the stale-window slots whose
        // `seq - head` cyclically wraps around at u32::MAX/2. For
        // any sane ring capacity (`<< u32::MAX/2`) the two checks
        // are equivalent, but the explicit forward-distance form
        // is easier to reason about.
        //
        // The Late check above guarantees `seq > head` here, so
        // the stored slot.seq comparison is the "at-or-forward of
        // head" condition. We only count overruns in playback mode
        // (`head.is_some()`); prebuffer traffic is benign because
        // the gate hasn't released yet.
        if let Some(h) = self.head {
            if self.slots[idx].filled && self.slots[idx].seq != seq {
                let fwd = self.slots[idx].seq.wrapping_sub(h);
                // fwd == 0: slot sits at the head's idx (just-cleared
                //          and re-pushed race window).
                // 0 < fwd < capacity_packets: slot is strictly future
                //          within the ring window.
                let cap = self.capacity_packets as u32;
                if fwd == 0 || (fwd > 0 && fwd < cap) {
                    self.stats.ring_overruns += 1;
                }
            }
        }

        // Store the packet (safe — already passed Late/MidRead checks
        // and the slot either doesn't match seq, or is empty).
        if !self.slots[idx].filled {
            self.filled_count += 1;
        }
        self.slots[idx].seq = seq;
        self.slots[idx].samples.copy_from_slice(samples);
        self.slots[idx].filled = true;

        // In prebuffer mode, track the smallest seq so play head can anchor.
        if self.head.is_none()
            && self
                .min_pushed
                .is_none_or(|prev| modular_is_earlier_or_equal(seq, prev))
        {
            self.min_pushed = Some(seq);
        }

        PushOutcome::Stored
    }

    /// Fill `out` with up to `out.len()` interleaved `f32` samples,
    /// drawing from packet slots in cyclic seq order. Partial-packet
    /// reads are supported: the `cpal` callback is free to ask for any
    /// `out.len()` from a single sample to many — the buffer tracks
    /// `(head, head_offset)` and a packet may drain across multiple
    /// calls. Each call returns `out.len()` after filling (silence
    /// pads any missing slot at this packet's boundary).
    ///
    /// Behaviour summary:
    /// * Prebuffer (no packets pushed): request is silence-filled;
    ///   `head` is not advanced.
    /// * Prebuffer (packets pushed but gate not met, slice 6):
    ///   `queued_frames()` < `prebuffer_target_frames` → silence-fill
    ///   and increment `prebuffer_holds`. `head` is not advanced.
    /// * Mid-stream: each iteration copies `min(samples_per_packet -
    ///   head_offset, out.len() - written)` samples from the current
    ///   slot (or silence if the slot is missing), then increments
    ///   `head_offset`. When `head_offset` reaches `samples_per_packet`
    ///   the slot is cleared, `head` is advanced by 1, and
    ///   `head_offset` resets to 0. A missing slot at the boundary
    ///   increments `silence_insertions`.
    ///
    /// **Caller constraint**: `out.len()` should not exceed
    /// `samples_per_packet × capacity_packets`. Larger requests will
    /// cause head to rotate through the entire ring and may replay
    /// stale (just-cleared) slot data. cpal callback sizes are
    /// ms-long and the ring is seconds-long, so this is unrealistic
    /// in practice, but consumers feeding very large `out` should
    /// chunk at `samples_per_packet × capacity_packets` first.
    pub fn pop_frames(&mut self, out: &mut [f32]) -> usize {
        if out.is_empty() {
            return 0;
        }

        // Slice 6 prebuffer gate. Before playback starts, if a target
        // has been set and queued_frames is below it, hold silence and
        // DO NOT anchor head - even if packets are already in the ring.
        // This is what `--prebuffer-ms` buys the operator: the
        // receiver waits until enough audio is buffered, then
        // anchors and starts playing. Anchoring now (before the
        // gate is met) would play the first packets too early,
        // potentially underruning before the rest of the ring
        // fills.
        if let Some(target) = self.prebuffer_target_frames {
            if self.head.is_none() && self.queued_frames() < target {
                self.stats.prebuffer_holds += 1;
                for s in &mut out[..] {
                    *s = 0.0;
                }
                return out.len();
            }
        }

        // Latency trim: if fill grew past high-water (clock skew /
        // sender burst), skip whole packets until we're near the
        // prebuffer target again. Only after the gate has released
        // (or no gate) and only when a play head can be established.
        self.trim_latency_if_needed();

        let mut written = 0;

        while written < out.len() {
            // Anchor or stay in prebuffer.
            let h = match self.head {
                Some(h) => h,
                None => match self.min_pushed {
                    Some(m) => {
                        self.head = Some(m);
                        m
                    }
                    None => {
                        // No packets have arrived — silence-fill the
                        // remainder of `out` and return.
                        for s in &mut out[written..] {
                            *s = 0.0;
                        }
                        return out.len();
                    }
                },
            };
            let idx = (h as usize) % self.capacity_packets;
            let remaining_in_packet = self.samples_per_packet - self.head_offset;
            let chunk = remaining_in_packet.min(out.len() - written);
            let target = written + chunk;

            if self.slots[idx].filled && self.slots[idx].seq == h {
                let src_start = self.head_offset;
                let src_end = src_start + chunk;
                out[written..target].copy_from_slice(&self.slots[idx].samples[src_start..src_end]);
            } else {
                // BUG-1 FIX: missing slot + empty ring → freeze.
                // Instead of advancing the play head through an
                // infinite void of silence (which makes the sender
                // permanently "behind" and rejects every future
                // push as Late), revert to prebuffer mode. The
                // prebuffer gate re-arms; once enough packets
                // arrive, playback resumes at the new anchor.
                if self.filled_count == 0 {
                    for s in &mut out[written..] {
                        *s = 0.0;
                    }
                    self.head = None;
                    self.head_offset = 0;
                    self.min_pushed = None;
                    self.resync_floor = Some(h);
                    self.stats.underrun_resyncs += 1;
                    return out.len();
                }
                // Missing packet (late arrival, gap, or initial silence).
                // Future packets exist in the ring — silence-fill
                // this chunk and keep advancing.
                for s in &mut out[written..target] {
                    *s = 0.0;
                }
            }
            written = target;
            self.head_offset += chunk;

            // Boundary: did this iteration consume the rest of the packet?
            if self.head_offset == self.samples_per_packet {
                if !self.slots[idx].filled || self.slots[idx].seq != h {
                    self.stats.silence_insertions += 1;
                }
                // Clear the slot so push can reuse it after the wrap.
                if self.slots[idx].filled {
                    self.filled_count -= 1;
                }
                self.slots[idx].filled = false;
                self.slots[idx].seq = 0;
                self.head = Some(h.wrapping_add(1));
                self.head_offset = 0;
            }
        }
        out.len()
    }

    /// Sample frames currently buffered for playback. Counts
    /// `samples.len()` for every filled slot, with one adjustment:
    /// if `head` is anchored and points at a filled slot, that slot's
    /// contribution is reduced by `head_offset` (samples already
    /// consumed mid-packet). In **prebuffer** (`head` still `None`)
    /// the sum of all filled slots is returned, treating the buffer
    /// as "all queued for playout once the head anchors."
    ///
    /// **Not counted**: in-flight silence-stuffing (a missing slot
    /// being padded with zeros). `queued_frames = 0` is the canonical
    /// "buffer is empty mid-stream" underrun signal — and is also
    /// the live value the slice-6 prebuffer gate compares against
    /// `prebuffer_target_frames`.
    ///
    /// **Note on slot-reclaim**: when more than `capacity_packets`
    /// distinct `seq` values are pushed into a ring of size
    /// `capacity_packets`, the older ones are silently overwritten
    /// (`seq % cap` collision). `queued_frames` therefore reports the
    /// NUMBER OF DISTINCT OCCUPIED SLOTS, not the cumulative push
    /// count. For the prebuffer-gate this is monotone (slots fill
    /// once, then stabilize); for stats we measure fill percentage.
    pub fn queued_frames(&self) -> usize {
        let base = self.filled_count * self.samples_per_packet;
        if let Some(h) = self.head {
            let idx = (h as usize) % self.capacity_packets;
            if self.slots[idx].filled && self.slots[idx].seq == h {
                base.saturating_sub(self.head_offset)
            } else {
                base
            }
        } else {
            base
        }
    }

    pub fn stats(&self) -> Stats {
        self.stats
    }

    /// Skip whole packets while `queued_frames` exceeds high-water,
    /// aiming for the prebuffer target fill. No-op when high-water
    /// is unset or head cannot be anchored yet.
    fn trim_latency_if_needed(&mut self) {
        let (Some(high), Some(target)) = (self.high_water_frames, self.prebuffer_target_frames)
        else {
            return;
        };
        if high <= target {
            return;
        }
        // Need a head to skip from; anchor if we have packets but
        // haven't started (gate already passed if we got here with
        // target met — or gate is None).
        if self.head.is_none() {
            if let Some(m) = self.min_pushed {
                self.head = Some(m);
                self.head_offset = 0;
            } else {
                return;
            }
        }
        while self.queued_frames() > high {
            let Some(h) = self.head else {
                break;
            };
            // Discard the remainder of the current packet, then any
            // full packets until under high-water / at target.
            let idx = (h as usize) % self.capacity_packets;
            if self.slots[idx].filled && self.slots[idx].seq == h {
                self.filled_count -= 1;
                self.slots[idx].filled = false;
                self.slots[idx].seq = 0;
            }
            self.head = Some(h.wrapping_add(1));
            self.head_offset = 0;
            self.stats.latency_trims += 1;
            // Stop once at or below target so we don't underrun.
            if self.queued_frames() <= target {
                break;
            }
        }
    }
}

/// True if `a` is at or before `b` in forward iteration of the cyclic
/// `u32` ring (i.e. walking forward from `a` reaches `b` within half a
/// cycle). Used for both `min_pushed` tracking and late-packet detection.
#[inline]
fn modular_is_earlier_or_equal(a: u32, b: u32) -> bool {
    let fwd = b.wrapping_sub(a);
    fwd < u32::MAX / 2
}

#[cfg(test)]
mod unit {
    use super::*;

    // ----- Modular comparison (existing) -----

    #[test]
    fn modular_compare_basic() {
        assert!(modular_is_earlier_or_equal(0, 0));
        assert!(modular_is_earlier_or_equal(0, 1));
        assert!(!modular_is_earlier_or_equal(1, 0));
        assert!(modular_is_earlier_or_equal(u32::MAX - 1, u32::MAX));
        assert!(modular_is_earlier_or_equal(u32::MAX, 0));
        assert!(modular_is_earlier_or_equal(u32::MAX - 1, 0));
        assert!(!modular_is_earlier_or_equal(0, u32::MAX - 1));
    }

    // ----- Helpers for the new tests -----

    /// Build a 4-sample-per-packet buffer with 4 ring slots and NO
    /// prebuffer gate (legacy behaviour). Compact for tests so
    /// sample reads are obvious.
    fn small_buffer() -> JitterBuffer {
        JitterBuffer::new(4, 4, None)
    }

    /// Same as small_buffer, but with an explicit prebuffer gate.
    fn gated_buffer(prebuffer_target_frames: usize) -> JitterBuffer {
        JitterBuffer::new(4, 4, Some(prebuffer_target_frames))
    }

    /// Fill a packet with a tagged monotonic ramp so each slot has an
    /// immediately-distinguishable signature: packet `seq` contains
    /// `[seq as f32; 4]` (with a tiny offset so consecutive packets
    /// don't collide on tag).
    fn tag_packet(buf: &mut JitterBuffer, seq: u32) {
        let mut samples = vec![seq as f32; 4];
        for s in samples.iter_mut() {
            *s += 0.001;
        }
        assert_eq!(
            buf.push(seq, &samples),
            PushOutcome::Stored,
            "expected Stored for fresh seq {seq}"
        );
    }

    // ----- prebuffer + partial-request handling (the silent-failure bug) -----

    #[test]
    fn partial_request_in_prebuffer_emits_silence_without_advancing() {
        // cpal asks for 2 samples before any packet arrives: output
        // must be 2 zero samples, head must NOT advance, queued_frames
        // must be 0 (still prebuffer).
        let mut buf = small_buffer();
        let mut out = [99.0_f32; 2];
        let n = buf.pop_frames(&mut out);
        assert_eq!(n, 2);
        assert_eq!(out, [0.0, 0.0], "prebuffer must silence-fill");
        assert_eq!(buf.stats().silence_insertions, 0, "no packet-silence yet");
        assert_eq!(buf.queued_frames(), 0, "still in prebuffer");
    }

    #[test]
    fn three_partial_pops_do_not_yet_cross_silence_boundary() {
        // Symmetric companion to `partial_reads_from_single_packet_*`:
        // pop 2 frames three times — only 6 frames total, but the
        // first 4 are slot[0] filled, and the third pop only writes
        // 2 frames of silence into slot[1]. head_offset advances to
        // 2 < samples_per_packet = 4, so the silence-packet
        // boundary is NOT yet crossed. silence_insertions must
        // remain 0. This pins the boundary semantics: the counter
        // advances only at the boundary, not per frame.
        let mut buf = small_buffer();
        tag_packet(&mut buf, 0);

        let mut a = [99.0_f32; 2];
        buf.pop_frames(&mut a);
        let mut b = [99.0_f32; 2];
        buf.pop_frames(&mut b);
        let mut c = [99.0_f32; 2];
        buf.pop_frames(&mut c);

        assert_eq!(a, [0.001, 0.001]);
        assert_eq!(b, [0.001, 0.001]);
        assert_eq!(c, [0.0, 0.0], "silence for slot[1] mid-read");
        assert_eq!(
            buf.stats().silence_insertions,
            0,
            "boundary not yet crossed after 3 partial pops"
        );
    }

    // ----- single-packet partial reads across multiple callbacks -----

    #[test]
    fn partial_reads_from_single_packet_then_underrun_freeze() {
        // Push one packet (seq=0) of 4 samples; pop in 2-frame chunks.
        // The first two pops drain seq=0 fully (head advances to seq=1,
        // slot[0] cleared, filled_count drops to 0). The third pop
        // hits slot[1] which is missing AND the ring is empty →
        // BUG-1 FIX: underrun freeze (not silence-stuff + advance).
        // The buffer reverts to prebuffer mode (head = None) and
        // records resync_floor = 1.
        let mut buf = small_buffer();
        tag_packet(&mut buf, 0);

        let mut a = [99.0_f32; 2];
        buf.pop_frames(&mut a);
        assert_eq!(a, [0.001, 0.001]);

        let mut b = [99.0_f32; 2];
        buf.pop_frames(&mut b);
        assert_eq!(b, [0.001, 0.001]);

        // Third pop: slot[1] missing, ring empty → underrun freeze.
        // Silence-fills the remainder and returns to prebuffer.
        let mut c = [99.0_f32; 2];
        buf.pop_frames(&mut c);
        assert_eq!(c, [0.0, 0.0], "underrun freeze silence-fills");
        assert_eq!(
            buf.stats().underrun_resyncs,
            1,
            "one underrun resync recorded"
        );
        assert_eq!(
            buf.stats().silence_insertions,
            0,
            "no silence_insertions — we froze, not advanced"
        );

        // Fourth pop: head is None, min_pushed is None → pure silence.
        let mut d = [99.0_f32; 2];
        buf.pop_frames(&mut d);
        assert_eq!(d, [0.0, 0.0], "prebuffer silence after freeze");
    }

    // ----- partial read crossing packet boundary -----

    #[test]
    fn partial_read_crosses_packet_boundary_cleanly() {
        // Push seq=0 and seq=1. Ask for 6 samples (1.5 packets). Expect
        // to see all 4 samples of seq=0, then the first 2 of seq=1.
        let mut buf = small_buffer();
        tag_packet(&mut buf, 0);
        tag_packet(&mut buf, 1);

        let mut out = [99.0_f32; 6];
        buf.pop_frames(&mut out);
        assert_eq!(out.len(), 6);
        assert_eq!(&out[..4], &[0.001, 0.001, 0.001, 0.001][..]);
        assert_eq!(&out[4..6], &[1.001, 1.001][..]);
        assert_eq!(
            buf.stats().silence_insertions,
            0,
            "no missing packets in this scenario"
        );
    }

    // ----- silence-stuffing count with partial reads -----

    #[test]
    fn partial_reads_count_silence_once_per_missing_packet() {
        // Push seq=0. Skip seq=1. Push seq=2.
        // Pop in 2-frame chunks. Across the whole sequence only ONE
        // missing-packet-silence event (seq=1) should be counted, no
        // matter how many partial reads split the silence-stuffing.
        let mut buf = small_buffer();
        tag_packet(&mut buf, 0);
        tag_packet(&mut buf, 2);

        let mut a = [99.0_f32; 2];
        buf.pop_frames(&mut a); // seq=0 samples 0-1
        let mut b = [99.0_f32; 2];
        buf.pop_frames(&mut b); // seq=0 samples 2-3
        let mut c = [99.0_f32; 2];
        buf.pop_frames(&mut c); // silence: seq=1 missing, partial packet read
        let mut d = [99.0_f32; 2];
        buf.pop_frames(&mut d); // silence: seq=1 missing, completes the 4-frame silence

        assert_eq!(c, [0.0, 0.0]);
        assert_eq!(d, [0.0, 0.0]);
        assert_eq!(
            buf.stats().silence_insertions,
            1,
            "exactly one missing-packet silence event counted"
        );

        // Continue: 4 samples of seq=2.
        let mut e = [99.0_f32; 4];
        buf.pop_frames(&mut e);
        assert_eq!(e, [2.001, 2.001, 2.001, 2.001]);
        assert_eq!(
            buf.stats().silence_insertions,
            1,
            "still one — no new missing packet"
        );
    }

    // ----- mid-read slot collision reject path -----

    #[test]
    fn mid_read_slot_collision_short_circuits_to_mid_read_outcome() {
        // Push seq=0. Begin reading — pop 2 of 4 so head_offset = 2.
        // The slot index for seq=0 is `0 % 4 = 0`. A push with seq =
        // capacity (= 4) lands at slot index 4 % 4 = 0 — same slot,
        // and head_offset > 0, so the buffer must refuse and report
        // MidReadCollision (NOT overwrite the active slot).
        let mut buf = JitterBuffer::new(4, 4, None);
        tag_packet(&mut buf, 0);

        let mut a = [0.0_f32; 2];
        buf.pop_frames(&mut a);
        assert_eq!(buf.stats().silence_insertions, 0);

        let mut collision_packet = vec![99.0_f32; 4];
        for s in collision_packet.iter_mut() {
            *s = 42.0;
        }
        let outcome = buf.push(4, &collision_packet);
        assert_eq!(
            outcome,
            PushOutcome::MidReadCollision,
            "mid-read slot collision must be refused"
        );
        assert_eq!(
            buf.stats().mid_read_collisions,
            1,
            "mid_read_collisions counter must increment"
        );

        // Continue draining seq=0; the data must NOT have been
        // corrupted by the refused push.
        let mut b = [0.0_f32; 2];
        buf.pop_frames(&mut b);
        assert_eq!(b, [0.001, 0.001], "active slot data must survive");
    }

    #[test]
    fn mid_read_collision_does_not_fire_when_head_offset_is_zero() {
        // Edge: capacity = 4, head = 1 just anchored (head_offset = 0),
        // slot 0 is empty (just cleared after consuming seq=0). Push
        // seq=4 — slot idx collides, but head_offset is 0 so the
        // buffer is not actively reading it. This is a clean overwrite.
        let mut buf = JitterBuffer::new(4, 4, None);
        tag_packet(&mut buf, 0);
        let mut out = [0.0_f32; 4];
        buf.pop_frames(&mut out); // consumes seq=0 fully, head_offset resets to 0
        assert_eq!(
            buf.stats().silence_insertions,
            0,
            "seq=0 was present, not silence"
        );

        let mut tag = [42.0_f32; 4];
        for s in tag.iter_mut() {
            *s = 4.0 + 0.001;
        }
        let outcome = buf.push(4, &tag);
        assert_eq!(
            outcome,
            PushOutcome::Stored,
            "head_offset=0 means no mid-read; clean store"
        );
        assert_eq!(buf.stats().mid_read_collisions, 0);
    }

    // ----- queued_frames accounting -----

    #[test]
    fn queued_frames_tracks_buffered_samples_including_head_offset() {
        // Push seq=0..=2 (3 full packets of 4 samples = 12 frames
        // buffered). Drained head_offset should reduce the count.
        let mut buf = small_buffer();
        tag_packet(&mut buf, 0);
        tag_packet(&mut buf, 1);
        tag_packet(&mut buf, 2);
        assert_eq!(buf.queued_frames(), 12, "3 full packets = 12 frames");

        // Drain 5 frames (4 of seq=0 + 1 of seq=1).
        let mut a = [0.0_f32; 5];
        buf.pop_frames(&mut a);
        // seq=0 drained fully (4 frames), seq=1 partial (1 frame so
        // 3 frames remain in slot 1 = 4 - 1).
        // Total = 4 (slot 2) + 3 (slot 1 partial) = 7 frames.
        assert_eq!(
            buf.queued_frames(),
            7,
            "head_offset=1 on seq=1 leaves 3 frames in slot 1; slot 2 still full"
        );
    }

    #[test]
    fn queued_frames_in_prebuffer_counts_all_filled_slots() {
        // Prebuffer mode: head=None, min_pushed=Some after pushes.
        // queued_frames should report sum of all filled slots'
        // sample lengths so the slice-6 prebuffer gate sees the
        // growing fill before head anchors.
        let mut buf = small_buffer();
        assert_eq!(buf.queued_frames(), 0, "no packets yet");
        tag_packet(&mut buf, 0);
        assert_eq!(buf.queued_frames(), 4);
        tag_packet(&mut buf, 1);
        assert_eq!(buf.queued_frames(), 8);
        tag_packet(&mut buf, 2);
        assert_eq!(buf.queued_frames(), 12, "3 full packets, no head anchor");
    }

    // ----- Slice 6 prebuffer gate -----

    #[test]
    fn prebuffer_gate_holds_silence_until_target_met() {
        // Gate at 8 frames (2 full packets). Push seq=0 only (4 frames
        // queued). pop_frames must return silence, increment
        // prebuffer_holds, and NOT anchor head — even though packets
        // are present.
        let mut buf = gated_buffer(8);
        tag_packet(&mut buf, 0);
        assert_eq!(buf.queued_frames(), 4, "1 packet = 4 frames");

        let mut out = [99.0_f32; 4];
        let n = buf.pop_frames(&mut out);
        assert_eq!(n, 4);
        assert_eq!(out, [0.0, 0.0, 0.0, 0.0], "gate holds silence");
        assert_eq!(
            buf.stats().prebuffer_holds,
            1,
            "one pop_frames call hit the gate"
        );
        assert_eq!(buf.stats().silence_insertions, 0, "no packet-silence yet");
        // head must NOT have been anchored: queued_frames is unchanged.
        assert_eq!(buf.queued_frames(), 4, "gate held; no anchor");
    }

    #[test]
    fn prebuffer_gate_releases_when_queued_crosses_target() {
        // Gate at 8 frames. Push seq=0, then seq=1 — queued_frames
        // crosses 8. pop_frames should now anchor and play the
        // buffered audio (no gate hit, no silence emission).
        let mut buf = gated_buffer(8);
        tag_packet(&mut buf, 0);
        tag_packet(&mut buf, 1);
        assert_eq!(buf.queued_frames(), 8, "2 packets = 8 frames = target");

        let mut out = [99.0_f32; 4];
        let n = buf.pop_frames(&mut out);
        assert_eq!(n, 4);
        assert_eq!(
            out,
            [0.001, 0.001, 0.001, 0.001],
            "gate released; seq=0 plays"
        );
        assert_eq!(
            buf.stats().prebuffer_holds,
            0,
            "gate did not fire on the call that crossed the target"
        );
        assert_eq!(buf.stats().silence_insertions, 0);
    }

    #[test]
    fn prebuffer_gate_does_not_reengage_after_playback_starts() {
        // Once the initial target is met and head is anchored, falling
        // below the target is normal steady-state drain. Reapplying the
        // startup gate here would periodically mute playback until the
        // full prebuffer accumulated again.
        let mut buf = gated_buffer(8);
        tag_packet(&mut buf, 0);
        tag_packet(&mut buf, 1);

        let mut first = [99.0_f32; 4];
        buf.pop_frames(&mut first);
        assert_eq!(first, [0.001, 0.001, 0.001, 0.001]);
        assert_eq!(buf.queued_frames(), 4, "fill is now below target");

        let mut second = [99.0_f32; 4];
        buf.pop_frames(&mut second);
        assert_eq!(
            second,
            [1.001, 1.001, 1.001, 1.001],
            "an anchored stream must keep playing below the startup target"
        );
        assert_eq!(buf.stats().prebuffer_holds, 0);
    }

    #[test]
    fn prebuffer_gate_holds_with_partial_fill_then_releases() {
        // Start with one packet (queued=4). Pop is held by the
        // gate (target=8). On the second packet (queued=8), the
        // gate releases; the pop plays seq=0.
        let mut buf = gated_buffer(8);

        // Pop while only seq=0 packet is present — gate holds.
        tag_packet(&mut buf, 0);
        {
            let mut out = [99.0_f32; 4];
            buf.pop_frames(&mut out);
            assert_eq!(out, [0.0, 0.0, 0.0, 0.0], "first pop held by gate");
            assert_eq!(buf.stats().prebuffer_holds, 1);
        }

        // Add the second packet; queued_frames now hits the target.
        tag_packet(&mut buf, 1);
        {
            let mut out = [99.0_f32; 4];
            buf.pop_frames(&mut out);
            assert_eq!(
                out,
                [0.001, 0.001, 0.001, 0.001],
                "second pop releases; seq=0 plays"
            );
            assert_eq!(
                buf.stats().prebuffer_holds,
                1,
                "still 1 — this pop DID NOT hit the gate"
            );
        }
    }

    #[test]
    fn new_rejects_zero_prebuffer_target() {
        let result = std::panic::catch_unwind(|| {
            JitterBuffer::new(4, 4, Some(0));
        });
        assert!(
            result.is_err(),
            "Some(0) must panic — a zero target is nonsensical"
        );
    }

    // ----- Slice 10 ring_overruns -----
    //
    // The ring_overruns counter fires when push silently overwrites
    // a slot whose stored seq is strictly *future* w.r.t. head.
    // This is the canonical "sender outpaced the receiver" signature
    // — distinct from MidReadCollision (cpal mid-drain collision).

    #[test]
    fn ring_overruns_default_is_zero() {
        // Stats::default() already initializes ring_overruns = 0. Pin
        // this so a future contributor adding a field doesn't
        // accidentally surface `ring_overruns=0` despite non-default
        // Default impl.
        let buf = small_buffer();
        assert_eq!(buf.stats().ring_overruns, 0);
    }

    #[test]
    fn ring_overruns_does_not_fire_on_a_clean_store_in_empty_slot() {
        // No prior occupancy → we're storing into a fresh slot.
        // ring_overruns must remain 0.
        let mut buf = small_buffer();
        tag_packet(&mut buf, 0);
        assert_eq!(buf.stats().ring_overruns, 0);
    }

    #[test]
    fn ring_overruns_does_not_fire_on_duplicate_of_same_seq() {
        // Same seq re-pushed → Duplicate outcome, NOT an overrun.
        // The slot is filled with the same seq; it's not an
        // unplayed future packet being clobbered.
        let mut buf = small_buffer();
        tag_packet(&mut buf, 0);
        assert_eq!(
            buf.push(0, &[99.0_f32; 4]),
            PushOutcome::Duplicate,
            "duplicate must NOT be classified as overrun"
        );
        assert_eq!(buf.stats().ring_overruns, 0);
    }

    #[test]
    fn ring_overruns_does_not_fire_on_late_packet_in_prebuffer() {
        // In prebuffer mode (head=None), every push fills a fresh
        // slot. ring_overruns must stay 0 even though the modular
        // index would otherwise collide after capacity pushes.
        let mut buf = small_buffer();
        for s in 0..16u32 {
            tag_packet(&mut buf, s);
        }
        assert_eq!(buf.stats().ring_overruns, 0);
    }

    #[test]
    fn ring_overruns_increments_when_overwriting_unplayed_future_slot() {
        // 4-slot ring, fill seq=0..=3 (4 Stored). Play seq=0 only
        // (advance head), then push seq=4. Slot idx 4 % 4 = 0,
        // currently filled with seq=0 — strictly equal to head
        // (already played, not future). Head advances to 1.
        // Push seq=5: slot idx 1, filled with seq=1 — strictly
        // future relative to new head=1. This must trigger
        // ring_overruns++.
        let mut buf = small_buffer();
        for s in 0..4u32 {
            tag_packet(&mut buf, s);
        }
        // Play exactly one packet to anchor head at 1.
        let mut out = [0.0_f32; 4];
        buf.pop_frames(&mut out);
        // Now slot[0] is empty (just cleared), slots[1..4] hold
        // seq=1,2,3 — all strictly > head=1.
        assert_eq!(
            buf.stats().ring_overruns,
            0,
            "first push after anchor cannot yet overrun"
        );
        // Push seq=4 into slot[0] (empty) — clean store.
        let mut s4 = [4.0_f32; 4];
        s4[0] += 0.001;
        assert_eq!(buf.push(4, &s4), PushOutcome::Stored);
        assert_eq!(
            buf.stats().ring_overruns,
            0,
            "store into cleared slot is not an overrun"
        );
        // Push seq=5 into slot[1] (currently holds seq=1, unplayed
        // future). True ring overrun.
        let mut s5 = [5.0_f32; 4];
        s5[0] += 0.001;
        let outcome = buf.push(5, &s5);
        assert_eq!(
            outcome,
            PushOutcome::Stored,
            "overwriting future slot is Stored"
        );
        assert_eq!(
            buf.stats().ring_overruns,
            1,
            "overwriting unplayed future slot must increment ring_overruns"
        );
    }

    #[test]
    fn ring_overruns_does_not_fire_on_mid_read_collision() {
        // MidReadCollision is a narrower reject path (slot == head_idx
        // AND head_offset > 0) and the packet is REFUSED. ring_overruns
        // must remain 0 in this branch because no overwrite happens.
        // (See the existing mid_read_slot_collision_short_circuits
        // test — we replicate the scenario just to pin the disjoint
        // accounting.)
        let mut buf = JitterBuffer::new(4, 4, None);
        tag_packet(&mut buf, 0);
        let mut a = [0.0_f32; 2];
        buf.pop_frames(&mut a);
        let collision_packet = vec![42.0_f32; 4];
        let outcome = buf.push(4, &collision_packet);
        assert_eq!(outcome, PushOutcome::MidReadCollision);
        assert_eq!(
            buf.stats().ring_overruns,
            0,
            "refused push must not count as overrun"
        );
    }

    #[test]
    fn latency_trim_skips_packets_when_over_high_water() {
        // 4 samples/packet, 8-slot ring. Prebuffer target = 8 frames
        // (2 packets), high-water = 16 frames (4 packets). Fill 6
        // packets (24 frames) then pop: trim should drop until <=
        // target, and latency_trims must be non-zero.
        let mut buf = JitterBuffer::with_high_water(4, 8, Some(8), Some(16));
        for s in 0..6u32 {
            tag_packet(&mut buf, s);
        }
        assert_eq!(buf.queued_frames(), 24);
        let mut out = [0.0_f32; 4];
        buf.pop_frames(&mut out);
        assert!(
            buf.stats().latency_trims > 0,
            "expected latency trims when fill > high-water"
        );
        assert!(
            buf.queued_frames() <= 16,
            "fill should be at or under high-water after trim; got {}",
            buf.queued_frames()
        );
    }

    // ----- R1: post-freeze sender restart -----

    #[test]
    fn sender_restart_after_underrun_freeze_is_accepted() {
        // Push packets at high seq so all 4 slots match their head
        // positions and get played. Then request MORE than 16 samples
        // so the loop continues past the 4th packet boundary and
        // hits the underrun freeze (empty slot + filled_count=0).
        let mut buf = JitterBuffer::new(4, 4, None);
        for s in 100..104u32 {
            tag_packet(&mut buf, s);
        }
        // Pop 20: 16 for the 4 matching packets + 4 for the
        // underrun silence. The 5th iteration finds head=104,
        // slot empty, filled_count=0 → freeze. resync_floor=104.
        let mut out = [0.0_f32; 20];
        buf.pop_frames(&mut out);
        assert_eq!(
            &out[..16],
            &[
                100.001, 100.001, 100.001, 100.001, //
                101.001, 101.001, 101.001, 101.001, //
                102.001, 102.001, 102.001, 102.001, //
                103.001, 103.001, 103.001, 103.001, //
            ],
            "4 packets play, then underrun silence"
        );
        assert_eq!(&out[16..20], &[0.0, 0.0, 0.0, 0.0], "underrun silence");
        assert_eq!(
            buf.stats().underrun_resyncs,
            1,
            "underrun freeze at head=104"
        );

        // Sender restarts at seq=0. Gap from 0 to resync_floor=104
        // is 104 > capacity=4 → sender restart detected.
        let restart_samples = vec![0.0_f32; 4];
        let result = buf.push(0, &restart_samples);
        assert_eq!(
            result,
            PushOutcome::Stored,
            "sender restart after freeze must be accepted"
        );
        assert_eq!(
            buf.stats().sender_restarts,
            1,
            "sender_restarts must increment"
        );
        // Playback resumes: pop_frames anchors at min_pushed=0,
        // slot 0 has seq=0 → match → play the restart packet.
        let mut pop_out = [0.0_f32; 4];
        buf.pop_frames(&mut pop_out);
        assert_eq!(pop_out, [0.0, 0.0, 0.0, 0.0], "restart packet plays");
    }

    // ----- R5: seq == head with head_offset == 0 stores -----

    #[test]
    fn seq_equals_head_with_zero_offset_stores_not_late() {
        // Core Bug-1 fix: when head = h, head_offset = 0, and the
        // slot for h is cleared (just consumed), pushing seq = h
        // must be Stored, not Late.
        let mut buf = small_buffer();
        tag_packet(&mut buf, 0);
        // Drain seq=0 fully → head advances to 1, slot[0] cleared.
        let mut out = [0.0_f32; 4];
        buf.pop_frames(&mut out);
        assert_eq!(out, [0.001, 0.001, 0.001, 0.001]);
        // Now head=1, head_offset=0. Push seq=1 — this is the
        // current head. With the old code this was Late; with the
        // fix it must be Stored.
        let samples = vec![9.9_f32; 4];
        let result = buf.push(1, &samples);
        assert_eq!(result, PushOutcome::Stored, "seq==head, offset=0 → Stored");
        // Verify the packet is actually playable.
        let mut pop_out = [0.0_f32; 4];
        buf.pop_frames(&mut pop_out);
        assert_eq!(pop_out, [9.9, 9.9, 9.9, 9.9], "stored packet must play");
    }

    // ----- R5: idle-gap resume -----

    #[test]
    fn idle_gap_resume_with_zero_packet_loss() {
        // Simulate: push exactly 4 packets (fills the 4-slot ring),
        // drain 3 (heads 0,1,2 play), then drain the last (head 3
        // plays) → head=4, filled_count=0 → underrun freeze at
        // resync_floor=4. Then the sender resumes with packets
        // starting at seq=4 (the resync_floor boundary). Verify
        // playback continues with zero packet loss.
        //
        // CRITICAL: push exactly capacity (0..4), not more. Pushing
        // 0..5 would overwrite seq=0 (4%4=0) and break the
        // assertions for out[..4].
        let mut buf = small_buffer(); // 4 samples/packet, 4 slots
        for s in 0..4u32 {
            tag_packet(&mut buf, s);
        }
        // Drain 12 samples = 3 packets (seq 0, 1, 2 — all match).
        let mut out = [0.0_f32; 12];
        buf.pop_frames(&mut out);
        assert_eq!(&out[..4], &[0.001, 0.001, 0.001, 0.001], "seq 0 plays");
        assert_eq!(&out[4..8], &[1.001, 1.001, 1.001, 1.001], "seq 1 plays");
        assert_eq!(&out[8..12], &[2.001, 2.001, 2.001, 2.001], "seq 2 plays");

        // Drain remaining 4 samples = 1 packet (seq 3 plays),
        // then head=4, filled_count=0 → underrun freeze.
        let mut out2 = [0.0_f32; 8];
        buf.pop_frames(&mut out2);
        assert_eq!(&out2[..4], &[3.001, 3.001, 3.001, 3.001], "seq 3 plays");
        // out2[4..8] is silence from the freeze.
        assert_eq!(buf.stats().underrun_resyncs, 1, "underrun freeze");

        // Sender resumes: push seq=4..=7. seq=4 == resync_floor,
        // which is accepted (not strictly earlier). seq=4 becomes
        // the new anchor.
        for s in 4..8u32 {
            tag_packet(&mut buf, s);
        }
        // Pop should play seq=4 onwards (no loss).
        let mut out3 = [0.0_f32; 4];
        buf.pop_frames(&mut out3);
        assert_eq!(
            out3,
            [4.001, 4.001, 4.001, 4.001],
            "resume must play from the re-anchor point"
        );
        assert_eq!(buf.stats().late_drops, 0, "zero packet loss on resume");
    }

    #[test]
    fn ring_overruns_independent_of_mid_read_collisions() {
        // A push that lands in the active mid-read slot AND would
        // also be a true overrun (i.e., not strictly MidReadCollision
        // because head_offset == 0 by then) still bumps ring_overruns.
        // Conversely a MidReadCollision that gets refused bumps
        // neither. The two counters track disjoint events.
        let mut buf = small_buffer();
        for s in 0..4u32 {
            tag_packet(&mut buf, s);
        }
        // Drain 4 samples — seq=0 fully, head advances to 1,
        // head_offset=0, slot[0] cleared.
        let mut out = [0.0_f32; 4];
        buf.pop_frames(&mut out);
        // Push seq=4 into slot[0] (cleared) — clean store, no
        // ring_overruns bump.
        let mut s4 = [4.0_f32; 4];
        s4[0] += 0.001;
        buf.push(4, &s4);
        assert_eq!(buf.stats().ring_overruns, 0);
        // Push seq=5 into slot[1] (currently holds seq=1, which
        // equals head=1 — this is the "just-cleared-then-re-pushed
        // race window" with head_offset=0, so MidReadCollision does
        // NOT fire; ring_overruns does).
        let mut s5 = [5.0_f32; 4];
        s5[0] += 0.001;
        buf.push(5, &s5);
        assert_eq!(
            buf.stats().ring_overruns,
            1,
            "slot.seq == head race window counts as overrun"
        );
        // Push seq=6 into slot[2] (currently holds seq=2, strictly
        // future within ring capacity — another ring_overruns++).
        let mut s6 = [6.0_f32; 4];
        s6[0] += 0.001;
        buf.push(6, &s6);
        assert_eq!(
            buf.stats().ring_overruns,
            2,
            "strict future within capacity also counts"
        );
        // No mid_read_collisions happened in this scenario.
        assert_eq!(buf.stats().mid_read_collisions, 0);
    }
}
