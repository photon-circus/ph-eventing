//! Lock-free SPSC overwrite ring for high-rate telemetry in no-std contexts.
//!
//! # Overview
//! - Single producer, single consumer.
//! - Producer never blocks; new writes overwrite the oldest slots when the ring wraps.
//! - Sequence numbers are monotonically increasing `u32`; `0` is reserved to mean "empty".
//! - The consumer can drain in-order (`poll_one`/`poll_up_to`) or sample the newest value (`latest`).
//! - If the consumer lags by more than `N`, it skips ahead and reports the number of dropped items.
//!
//! # Memory ordering
//! The producer invalidates the per-slot sequence, writes the value, publishes the new per-slot
//! sequence, then publishes the newest sequence. The consumer validates the per-slot sequence
//! before and after reading, which avoids observing a new value under an old sequence number when
//! the producer overwrites a slot.
//!
//! The barriers on both sides are fences rather than ordered accesses on the sequence itself: a
//! `Release` fence keeps the producer's invalidation ahead of its value write, and an `Acquire`
//! fence keeps the consumer's copy ahead of its re-check. Plain `Release`/`Acquire` on the
//! sequence stores and loads would leave the value access free to drift across the guard it is
//! supposed to be bracketed by.
//!
//! Slot values are read and written with volatile accesses, and the consumer holds its copy as
//! `MaybeUninit<T>` until the re-check passes. A copy that raced with an overwrite is therefore
//! discarded as raw bytes and never materialises as a `T` that could violate the type's validity
//! invariants.
//!
//! # Known deviation: the seqlock data race
//! This is a seqlock, and seqlocks are formally racy. The consumer may copy a slot while the
//! producer overwrites it; the sequence re-check then discards the copy. Miri's data-race
//! detector reports that copy as undefined behaviour, and it is correct to: `read_volatile`
//! constrains the compiler but does not make the access atomic.
//!
//! This cannot be resolved in stable Rust for an arbitrary `T: Copy`. A race-free version needs
//! an atomic per-word copy, which needs an array length computed from `size_of::<T>()`
//! (unstable), and any `T` carrying padding cannot be copied byte-wise without reading
//! uninitialised memory. Eliminating the race in the design instead would mean making the
//! producer wait for the consumer, which contradicts the whole point of an overwrite ring.
//!
//! What is guaranteed in practice: the volatile accesses stop the compiler from splitting,
//! duplicating, or reordering the copy; the double sequence check stops a raced copy from ever
//! being observed; and holding it as `MaybeUninit<T>` stops it from becoming an invalid value.
//! `scripts/miri.*` verifies the rest of the crate with the race detector on, and this ring's
//! logic with it off. If you need a buffer with no formal race at all, use [`crate::EventBuf`],
//! which is race-free by construction — its producer and consumer never touch the same slot.
//!
//! # Notes
//! - `T` is `Copy` to allow returning values by copy without allocation.
//! - The `&T` passed to hooks is a reference to a local copy made during the read.
//! - Sequence arithmetic goes through `seq_distance`, which accounts for the reserved value `0`
//!   that `push` skips on wrap; raw wrapping subtraction over-counts by one across that boundary.

use crate::sync::{AtomicBool, AtomicU32, Ordering, fence};
// Slots stay on `core`'s cell rather than the Loom-tracked one. The seqlock's
// slot access is racy by construction (see "Known deviation" above), so a
// tracked cell would only re-report a documented deviation and mask everything
// else Loom has to say. The sequence protocol — which is what the correctness
// argument actually rests on — is built from the atomics above, and Loom
// models that in full.
use core::cell::{Cell, UnsafeCell};
use core::marker::PhantomData;
use core::mem::MaybeUninit;
#[cfg(test)]
use core::sync::atomic::AtomicUsize;

fn atomic_u32_array<const N: usize>(init: u32) -> [AtomicU32; N] {
    core::array::from_fn(|_| AtomicU32::new(init))
}

fn unsafe_cell_array<T, const N: usize>() -> [UnsafeCell<MaybeUninit<T>>; N] {
    core::array::from_fn(|_| UnsafeCell::new(MaybeUninit::uninit()))
}

// Test-only hook state. These use `core` atomics directly rather than the
// `crate::sync` shim: Loom's atomics are not const-constructible, and this
// hook is scaffolding for a single-threaded test rather than part of the
// protocol Loom models.
#[cfg(test)]
static TEST_AFTER_READ_TARGET: AtomicUsize = AtomicUsize::new(0);
#[cfg(test)]
static TEST_AFTER_READ_SEQ: core::sync::atomic::AtomicU32 = core::sync::atomic::AtomicU32::new(0);

#[must_use]
#[derive(Copy, Clone, Debug)]
pub struct PollStats {
    /// Number of items delivered to the hook.
    pub read: usize,
    /// Number of items skipped because the consumer lagged or slots were overwritten.
    pub dropped: usize,
    /// Newest sequence observed while polling.
    pub newest: u32,
}

/// Overwrite ring for SPSC high-rate telemetry.
/// Producer never waits; consumer may drop if it lags > N.
pub struct SeqRing<T: Copy, const N: usize> {
    next_seq: AtomicU32,
    published_seq: AtomicU32,
    slot_seq: [AtomicU32; N],
    slots: [UnsafeCell<MaybeUninit<T>>; N],
    producer_taken: AtomicBool,
    consumer_taken: AtomicBool,
}

// SAFETY: SeqRing is Sync because the producer/consumer handles enforce SPSC usage,
// and all shared state is accessed via atomics. Values are written before their
// sequence numbers are published with Release and read with Acquire. T: Send ensures
// values can be transferred across threads safely.
unsafe impl<T: Copy + Send, const N: usize> Sync for SeqRing<T, N> {}

impl<T: Copy, const N: usize> SeqRing<T, N> {
    /// Create a new ring buffer.
    ///
    /// # Panics
    /// Panics if `N == 0`.
    pub fn new() -> Self {
        assert!(N > 0);
        Self {
            next_seq: AtomicU32::new(0),
            published_seq: AtomicU32::new(0),
            slot_seq: atomic_u32_array::<N>(0),
            slots: unsafe_cell_array::<T, N>(),
            producer_taken: AtomicBool::new(false),
            consumer_taken: AtomicBool::new(false),
        }
    }

    /// Maximum number of items the ring can hold.
    #[inline]
    pub const fn capacity(&self) -> usize {
        N
    }

    #[inline(always)]
    const fn idx_for(seq: u32) -> usize {
        ((seq.wrapping_sub(1)) as usize) % N
    }

    /// Create the producer handle. Only one producer may be active.
    ///
    /// # Panics
    /// Panics if a producer handle is already active.
    #[inline]
    pub fn producer(&self) -> Producer<'_, T, N> {
        assert!(
            !self.producer_taken.swap(true, Ordering::AcqRel),
            "SeqRing::producer() called while a producer is active"
        );
        Producer {
            ring: self,
            _not_sync: PhantomData,
        }
    }

    /// Create the consumer handle. Only one consumer may be active.
    ///
    /// # Panics
    /// Panics if a consumer handle is already active.
    #[inline]
    pub fn consumer(&self) -> Consumer<'_, T, N> {
        assert!(
            !self.consumer_taken.swap(true, Ordering::AcqRel),
            "SeqRing::consumer() called while a consumer is active"
        );
        Consumer {
            ring: self,
            last_seq: 0,
            dropped_accum: 0,
            _not_sync: PhantomData,
        }
    }

    #[inline]
    fn newest_seq(&self) -> u32 {
        self.published_seq.load(Ordering::Acquire)
    }

    #[inline]
    fn push_inner(&self, value: T) -> u32 {
        let mut seq = self
            .next_seq
            .fetch_add(1, Ordering::Relaxed)
            .wrapping_add(1);
        if seq == 0 {
            seq = 1;
            self.next_seq.store(1, Ordering::Relaxed);
        }

        let idx = Self::idx_for(seq);
        // Invalidate before writing so a concurrent reader of the previous
        // sequence cannot observe the new value under the old sequence number.
        // The Release fence keeps the invalidation ahead of the value write.
        self.slot_seq[idx].store(0, Ordering::Relaxed);
        fence(Ordering::Release);

        // SAFETY: the producer is the only writer, and `idx` is in bounds
        // because `idx_for` reduces modulo N. The write is volatile to match
        // the volatile read in `read_seq_inner`: a consumer may be copying
        // this slot concurrently, so the compiler must not split, duplicate,
        // or move the store.
        unsafe { core::ptr::write_volatile(self.slots[idx].get(), MaybeUninit::new(value)) };

        self.slot_seq[idx].store(seq, Ordering::Release);
        self.published_seq.store(seq, Ordering::Release);
        seq
    }

    /// Advance past the reserved empty sequence `0`.
    #[inline(always)]
    const fn next_after(seq: u32) -> u32 {
        match seq.wrapping_add(1) {
            0 => 1,
            n => n,
        }
    }

    /// How many sequence numbers `push` actually assigned in `(from, to]`.
    ///
    /// Plain wrapping subtraction over-counts by one whenever the span crosses
    /// the reserved value `0`, because `push` skips it. The span crosses `0`
    /// exactly when `to` compares below `from`, since that is the only way the
    /// walk from `from` up to `to` can pass through the wrap point.
    #[inline(always)]
    const fn seq_distance(from: u32, to: u32) -> u32 {
        let raw = to.wrapping_sub(from);
        if to < from { raw - 1 } else { raw }
    }

    #[inline]
    fn read_seq_inner(&self, seq: u32) -> Option<T> {
        let idx = Self::idx_for(seq);

        let s1 = self.slot_seq[idx].load(Ordering::Acquire);
        if s1 != seq {
            return None;
        }

        // Copy the slot as raw bytes. The producer may be overwriting it right
        // now, so the bytes are not trusted until the sequence re-check below
        // passes — holding the copy as `MaybeUninit<T>` means a torn read
        // cannot produce an invalid `T`, only bytes that are then discarded.
        //
        // SAFETY: `idx` is in bounds because `idx_for` reduces modulo N. The
        // read is volatile so the compiler cannot split, duplicate, or hoist
        // it, and `MaybeUninit<T>` has no validity invariant to violate.
        let v: MaybeUninit<T> = unsafe { core::ptr::read_volatile(self.slots[idx].get()) };

        #[cfg(test)]
        self.test_after_read_hook(idx);

        // Pin the copy above the re-check. An Acquire fence orders preceding
        // loads ahead of what follows; a plain Acquire load on `s2` would only
        // stop *later* accesses from moving up, which would let the copy sink
        // past the check that is supposed to validate it.
        fence(Ordering::Acquire);

        let s2 = self.slot_seq[idx].load(Ordering::Relaxed);
        if s2 != seq {
            return None;
        }

        // SAFETY: the slot sequence matched `seq` both before and after the
        // copy, and the producer invalidates the sequence before it touches a
        // slot, so no write overlapped the read and the bytes are a complete,
        // initialised `T`.
        Some(unsafe { v.assume_init() })
    }

    #[cfg(test)]
    fn test_after_read_hook(&self, idx: usize) {
        let target = TEST_AFTER_READ_TARGET.load(Ordering::Acquire);
        if target == self as *const _ as usize {
            let seq = TEST_AFTER_READ_SEQ.load(Ordering::Relaxed);
            self.slot_seq[idx].store(seq, Ordering::Release);
            TEST_AFTER_READ_TARGET.store(0, Ordering::Release);
        }
    }
}

impl<T: Copy, const N: usize> Default for SeqRing<T, N> {
    fn default() -> Self {
        Self::new()
    }
}

impl<T: Copy, const N: usize> core::fmt::Debug for SeqRing<T, N> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("SeqRing")
            .field("capacity", &N)
            .field("published_seq", &self.published_seq.load(Ordering::Relaxed))
            .finish()
    }
}

/// Producer handle for writing into the ring.
///
/// This handle is `!Sync` to prevent concurrent producers.
pub struct Producer<'a, T: Copy, const N: usize> {
    ring: &'a SeqRing<T, N>,
    _not_sync: PhantomData<Cell<()>>,
}

impl<'a, T: Copy, const N: usize> Producer<'a, T, N> {
    /// Write a value into the ring.
    ///
    /// Returns the sequence number assigned to the write (never 0).
    #[inline]
    pub fn push(&self, value: T) -> u32 {
        self.ring.push_inner(value)
    }
}

impl<'a, T: Copy, const N: usize> Drop for Producer<'a, T, N> {
    fn drop(&mut self) {
        self.ring.producer_taken.store(false, Ordering::Release);
    }
}

impl<T: Copy, const N: usize> core::fmt::Debug for Producer<'_, T, N> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("seq_ring::Producer")
            .field("capacity", &N)
            .finish()
    }
}

/// Consumer handle for reading from the ring.
///
/// This handle is `!Sync` to prevent concurrent consumers.
pub struct Consumer<'a, T: Copy, const N: usize> {
    ring: &'a SeqRing<T, N>,
    last_seq: u32,
    dropped_accum: usize,
    _not_sync: PhantomData<Cell<()>>,
}

impl<'a, T: Copy, const N: usize> Consumer<'a, T, N> {
    /// How many items have been dropped since consumer creation (or since reset).
    ///
    /// The counter saturates at [`usize::MAX`] rather than wrapping, so on a
    /// 32-bit target a very long-lived lagging consumer reports "at least this
    /// many" instead of overflowing. Call [`reset_dropped`](Self::reset_dropped)
    /// periodically if exact long-run totals matter.
    #[inline]
    pub fn dropped(&self) -> usize {
        self.dropped_accum
    }

    /// Reset the internal drop counter.
    #[inline]
    pub fn reset_dropped(&mut self) {
        self.dropped_accum = 0;
    }

    /// Drain at most one item (in-order).
    /// Returns true if an item was delivered to the hook.
    #[inline]
    pub fn poll_one(&mut self, hook: impl FnOnce(u32, &T)) -> bool {
        let mut hook = Some(hook);
        let stats = self.poll_up_to(1, |seq, v| {
            if let Some(hook) = hook.take() {
                hook(seq, v);
            }
        });
        stats.read == 1
    }

    /// Drain up to `max` items (in-order).
    /// Hook sees `&T` but it is a reference to a **local copy** inside poll.
    ///
    /// If `max == 0`, this returns immediately with `read = 0`, `dropped = 0`, and
    /// `newest` set to the latest published sequence.
    pub fn poll_up_to(&mut self, max: usize, mut hook: impl FnMut(u32, &T)) -> PollStats {
        if max == 0 {
            return PollStats {
                read: 0,
                dropped: 0,
                newest: self.ring.newest_seq(),
            };
        }

        let mut newest = self.ring.newest_seq();
        if newest == 0 || newest == self.last_seq {
            return PollStats {
                read: 0,
                dropped: 0,
                newest,
            };
        }

        let mut read = 0usize;
        let mut dropped = 0usize;

        while read < max {
            newest = self.ring.newest_seq();
            if self.last_seq == newest {
                break;
            }

            let lag = SeqRing::<T, N>::seq_distance(self.last_seq, newest) as usize;
            if lag > N {
                let keep_from = newest.wrapping_sub((N - 1) as u32);
                let resume_after = keep_from.wrapping_sub(1);
                // Everything in (last_seq, keep_from) is gone; count what was
                // really assigned rather than the raw sequence span.
                let jumped = SeqRing::<T, N>::seq_distance(self.last_seq, resume_after) as usize;
                dropped = dropped.saturating_add(jumped);
                self.last_seq = resume_after;
                continue;
            }

            let next = SeqRing::<T, N>::next_after(self.last_seq);

            match self.ring.read_seq_inner(next) {
                Some(v) => {
                    hook(next, &v);
                    self.last_seq = next;
                    read += 1;
                }
                None => {
                    self.last_seq = next;
                    dropped = dropped.saturating_add(1);
                }
            }
        }

        // Saturate rather than wrap. `usize` is 32 bits on every target this
        // crate ships to, and the sequence space is also 32 bits, so a
        // long-running consumer that lags can genuinely reach the top of the
        // range. Overflow here would panic in debug and silently wrap in
        // release — on an embedded target, in a hot path.
        self.dropped_accum = self.dropped_accum.saturating_add(dropped);

        PollStats {
            read,
            dropped,
            newest,
        }
    }

    /// "Give me the newest thing right now" (not in-order).
    /// Returns true if it delivered something.
    ///
    /// This does not advance the consumer cursor.
    #[inline]
    pub fn latest(&self, hook: impl FnOnce(u32, &T)) -> bool {
        let newest = self.ring.newest_seq();
        if newest == 0 {
            return false;
        }
        if let Some(v) = self.ring.read_seq_inner(newest) {
            hook(newest, &v);
            true
        } else {
            false
        }
    }

    /// Fast-forward consumer so the *next* `poll_one()` yields the newest item
    /// (i.e. skip backlog).
    ///
    /// This does not modify the dropped counter.
    #[inline]
    pub fn skip_to_latest(&mut self) {
        let newest = self.ring.newest_seq();
        if newest != 0 {
            self.last_seq = newest.wrapping_sub(1);
        }
    }
}

impl<'a, T: Copy, const N: usize> Drop for Consumer<'a, T, N> {
    fn drop(&mut self) {
        self.ring.consumer_taken.store(false, Ordering::Release);
    }
}

impl<T: Copy, const N: usize> core::fmt::Debug for Consumer<'_, T, N> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("seq_ring::Consumer")
            .field("capacity", &N)
            .field("last_seq", &self.last_seq)
            .field("dropped", &self.dropped_accum)
            .finish()
    }
}

impl<T: Copy, const N: usize> crate::traits::Sink<T> for Producer<'_, T, N> {
    type Error = core::convert::Infallible;

    #[inline]
    fn try_push(&mut self, val: T) -> Result<(), core::convert::Infallible> {
        self.push(val);
        Ok(())
    }
}

impl<T: Copy, const N: usize> crate::traits::Source<T> for Consumer<'_, T, N> {
    #[inline]
    fn try_pop(&mut self) -> Option<T> {
        let mut result = None;
        self.poll_one(|_seq, v| result = Some(*v));
        result
    }
}

#[cfg(test)]
mod tests {
    use super::{SeqRing, TEST_AFTER_READ_SEQ, TEST_AFTER_READ_TARGET};
    use core::sync::atomic::Ordering;
    use std::vec::Vec;

    #[test]
    fn poll_one_empty_returns_false() {
        let ring = SeqRing::<u32, 4>::new();
        let mut consumer = ring.consumer();
        let ok = consumer.poll_one(|_, _| {});
        assert!(!ok);
    }

    #[test]
    fn polls_in_order() {
        let ring = SeqRing::<u32, 8>::new();
        let producer = ring.producer();
        let mut consumer = ring.consumer();

        producer.push(10);
        producer.push(11);
        producer.push(12);

        let mut seen = Vec::new();
        let stats = consumer.poll_up_to(10, |seq, v| seen.push((seq, *v)));

        assert_eq!(stats.read, 3);
        assert_eq!(stats.dropped, 0);
        assert_eq!(stats.newest, 3);
        assert_eq!(&seen[..], &[(1, 10), (2, 11), (3, 12)]);
    }

    #[test]
    fn drops_when_consumer_lags() {
        let ring = SeqRing::<u32, 4>::new();
        let producer = ring.producer();
        let mut consumer = ring.consumer();

        for i in 0..10 {
            producer.push(i);
        }

        let mut seen = Vec::new();
        let stats = consumer.poll_up_to(10, |seq, v| seen.push((seq, *v)));

        assert_eq!(stats.read, 4);
        assert_eq!(stats.dropped, 6);
        assert_eq!(stats.newest, 10);
        assert_eq!(&seen[..], &[(7, 6), (8, 7), (9, 8), (10, 9)]);
    }

    #[test]
    fn latest_reads_newest() {
        let ring = SeqRing::<u32, 8>::new();
        let producer = ring.producer();
        let consumer = ring.consumer();

        producer.push(1);
        producer.push(2);

        let mut got = None;
        let ok = consumer.latest(|seq, v| got = Some((seq, *v)));

        assert!(ok);
        assert_eq!(got, Some((2, 2)));
    }

    #[test]
    fn skip_to_latest_makes_next_poll_latest() {
        let ring = SeqRing::<u32, 8>::new();
        let producer = ring.producer();
        let mut consumer = ring.consumer();

        producer.push(10);
        producer.push(11);
        producer.push(12);

        consumer.skip_to_latest();

        let mut got = None;
        let ok = consumer.poll_one(|seq, v| got = Some((seq, *v)));

        assert!(ok);
        assert_eq!(got, Some((3, 12)));
    }

    #[test]
    fn poll_up_to_zero_returns_newest_only() {
        let ring = SeqRing::<u32, 4>::new();
        let producer = ring.producer();
        let mut consumer = ring.consumer();

        producer.push(42);

        let stats = consumer.poll_up_to(0, |_, _| panic!("hook should not run"));

        assert_eq!(stats.read, 0);
        assert_eq!(stats.dropped, 0);
        assert_eq!(stats.newest, 1);
    }

    #[test]
    fn dropped_counter_can_reset() {
        let ring = SeqRing::<u32, 2>::new();
        let producer = ring.producer();
        let mut consumer = ring.consumer();

        for i in 0..5 {
            producer.push(i);
        }

        let stats = consumer.poll_up_to(10, |_, _| {});

        assert_eq!(consumer.dropped(), stats.dropped);

        consumer.reset_dropped();

        assert_eq!(consumer.dropped(), 0);
    }

    #[test]
    fn latest_empty_returns_false() {
        let ring = SeqRing::<u32, 4>::new();
        let consumer = ring.consumer();

        let ok = consumer.latest(|_, _| {});

        assert!(!ok);
    }

    #[test]
    fn latest_returns_false_when_slot_missing() {
        let ring = SeqRing::<u32, 4>::new();
        let consumer = ring.consumer();

        ring.published_seq.store(1, Ordering::Release);

        let ok = consumer.latest(|_, _| {});

        assert!(!ok);
    }

    #[test]
    fn poll_up_to_counts_dropped_when_slot_missing() {
        let ring = SeqRing::<u32, 4>::new();
        let mut consumer = ring.consumer();

        ring.published_seq.store(1, Ordering::Release);

        let stats = consumer.poll_up_to(1, |_, _| panic!("hook should not run"));

        assert_eq!(stats.read, 0);
        assert_eq!(stats.dropped, 1);
        assert_eq!(consumer.dropped(), 1);
    }

    #[test]
    fn read_seq_inner_detects_overwrite_during_read() {
        let ring = SeqRing::<u32, 4>::new();
        let producer = ring.producer();
        let seq = producer.push(7);

        TEST_AFTER_READ_SEQ.store(seq.wrapping_add(1), Ordering::Relaxed);
        TEST_AFTER_READ_TARGET.store(&ring as *const _ as usize, Ordering::Release);

        let got = ring.read_seq_inner(seq);

        TEST_AFTER_READ_TARGET.store(0, Ordering::Release);

        assert!(got.is_none());
    }

    #[test]
    fn push_wraps_seq_from_zero_to_one() {
        let ring = SeqRing::<u32, 4>::new();

        ring.next_seq.store(u32::MAX, Ordering::Relaxed);

        let seq = ring.producer().push(1);

        assert_eq!(seq, 1);
        assert_eq!(ring.next_seq.load(Ordering::Relaxed), 1);
    }

    #[test]
    fn read_seq_inner_rejects_invalidated_slot() {
        let ring = SeqRing::<u32, 4>::new();
        let producer = ring.producer();
        let seq = producer.push(7);

        ring.slot_seq[SeqRing::<u32, 4>::idx_for(seq)].store(0, Ordering::Release);

        assert!(ring.read_seq_inner(seq).is_none());
    }

    #[test]
    fn consumer_skips_reserved_seq_zero_on_wrap() {
        let ring = SeqRing::<u32, 4>::new();
        let producer = ring.producer();
        let mut consumer = ring.consumer();

        ring.next_seq.store(u32::MAX - 1, Ordering::Relaxed);
        assert_eq!(producer.push(10), u32::MAX);

        consumer.skip_to_latest();
        let mut got = None;
        assert!(consumer.poll_one(|s, v| got = Some((s, *v))));
        assert_eq!(got, Some((u32::MAX, 10)));

        assert_eq!(producer.push(20), 1);

        let mut got = None;
        let stats = consumer.poll_up_to(4, |s, v| got = Some((s, *v)));

        assert_eq!(stats.read, 1);
        assert_eq!(stats.dropped, 0);
        assert_eq!(got, Some((1, 20)));
    }

    #[test]
    fn lag_across_wrap_counts_drops_exactly() {
        let ring = SeqRing::<u32, 4>::new();
        let producer = ring.producer();
        let mut consumer = ring.consumer();

        // Park the sequence just below the wrap and consume one item, so the
        // consumer's cursor sits in the pre-wrap region.
        ring.next_seq.store(u32::MAX - 6, Ordering::Relaxed);
        assert_eq!(producer.push(100), u32::MAX - 5);

        let mut got = None;
        assert!(consumer.poll_one(|s, v| got = Some((s, *v))));
        assert_eq!(got, Some((u32::MAX - 5, 100)));

        // A fresh consumer counts every sequence published before it existed
        // as dropped; clear that so the assertions below measure only the
        // wrap-crossing jump.
        consumer.reset_dropped();

        // 15 more pushes: five before the wrap, then 1..=10 after it. `push`
        // skips the reserved 0, so the raw sequence span is 16 while only 15
        // items exist — the drop accounting must not count the gap.
        let pushed: Vec<u32> = (0..15u32).map(|i| producer.push(i)).collect();
        assert_eq!(pushed.last().copied(), Some(10));

        let mut seen = Vec::new();
        let stats = consumer.poll_up_to(16, |seq, v| seen.push((seq, *v)));

        assert_eq!(stats.read, 4);
        assert_eq!(stats.dropped, 11);
        assert_eq!(stats.read + stats.dropped, pushed.len());

        let seqs: Vec<u32> = seen.iter().map(|(s, _)| *s).collect();
        assert_eq!(&seqs[..], &[7, 8, 9, 10]);
    }

    #[test]
    fn dropped_accum_saturates_instead_of_overflowing() {
        let ring = SeqRing::<u32, 4>::new();
        let producer = ring.producer();
        let mut consumer = ring.consumer();

        // A consumer that starts at 0 against a producer near the top of the
        // sequence space books close to 2^32 drops in one poll. On a 32-bit
        // target that is most of `usize`, so a second poll must not overflow
        // the accumulator — every target this crate ships to is 32-bit.
        ring.next_seq.store(u32::MAX - 2, Ordering::Relaxed);
        producer.push(1);
        let _ = consumer.poll_up_to(4, |_, _| {});
        let after_first = consumer.dropped();
        assert!(after_first > 0);

        for _ in 0..8 {
            producer.push(2);
            let _ = consumer.poll_up_to(4, |_, _| {});
        }

        assert!(
            consumer.dropped() >= after_first,
            "dropped counter went backwards — it wrapped instead of saturating"
        );
    }

    #[test]
    fn seq_distance_skips_the_reserved_zero() {
        type R = SeqRing<u32, 4>;

        // No wrap: plain difference.
        assert_eq!(R::seq_distance(0, 0), 0);
        assert_eq!(R::seq_distance(0, 5), 5);
        assert_eq!(R::seq_distance(5, 9), 4);

        // Spanning the wrap: one fewer than the raw span, because 0 is never
        // assigned by `push`.
        assert_eq!(R::seq_distance(u32::MAX, 1), 1);
        assert_eq!(R::seq_distance(u32::MAX - 5, 6), 11);
        assert_eq!(R::seq_distance(u32::MAX, u32::MAX), 0);
    }

    #[test]
    fn concurrent_overwrite_never_yields_a_mismatched_value() {
        use core::sync::atomic::AtomicBool;

        // Each payload repeats its counter four times, so a torn read shows up
        // as elements that disagree with each other. A small ring against an
        // unthrottled producer keeps the consumer permanently behind, which is
        // exactly the overwrite pressure the slot-invalidation guards against.
        let ring = SeqRing::<[u32; 4], 2>::new();
        let total = crate::test_support::iterations(20_000);
        let done = AtomicBool::new(false);

        std::thread::scope(|scope| {
            scope.spawn(|| {
                let producer = ring.producer();
                for i in 0..total {
                    producer.push([i; 4]);
                }
                done.store(true, Ordering::Release);
            });

            scope.spawn(|| {
                let mut consumer = ring.consumer();
                let mut last_seq = 0u32;
                let mut read_total = 0usize;

                loop {
                    // Sample before polling: if the producer finishes after
                    // this load, the next iteration still drains the tail.
                    let finished = done.load(Ordering::Acquire);

                    let mut batch_last = last_seq;
                    let stats = consumer.poll_up_to(8, |seq, v| {
                        assert!(
                            seq > batch_last,
                            "sequence went backwards: {seq} after {batch_last}"
                        );
                        batch_last = seq;

                        // Pushes are consecutive from 0, so sequence `n`
                        // always carries payload `n - 1`. Anything else means
                        // a stale value surfaced under a fresh sequence, or a
                        // fresh value under a stale one.
                        let expected = seq - 1;
                        assert_eq!(
                            *v, [expected; 4],
                            "sequence {seq} carried a stale or torn payload"
                        );
                    });

                    last_seq = batch_last;
                    read_total += stats.read;

                    if finished && stats.read == 0 && stats.dropped == 0 {
                        break;
                    }
                }

                // Every published sequence was either delivered or counted as
                // dropped — the consumer's accounting must be exact, not
                // approximate.
                assert_eq!(last_seq, total, "consumer stopped short of the tail");
                assert_eq!(
                    read_total + consumer.dropped(),
                    total as usize,
                    "read + dropped must account for every published item"
                );
            });
        });
    }

    #[test]
    fn capacity_returns_n() {
        let ring = SeqRing::<u32, 8>::new();
        assert_eq!(ring.capacity(), 8);
    }
}
