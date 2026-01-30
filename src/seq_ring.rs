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
//! The producer writes the value, publishes the per-slot sequence, then publishes the newest
//! sequence. The consumer validates the per-slot sequence before and after reading, which avoids
//! torn reads when the producer overwrites a slot.
//!
//! # Notes
//! - `T` is `Copy` to allow returning values by copy without allocation.
//! - The `&T` passed to hooks is a reference to a local copy made during the read.

use core::cell::UnsafeCell;
use core::mem::MaybeUninit;
use core::sync::atomic::{AtomicU32, Ordering};

fn atomic_u32_array<const N: usize>(init: u32) -> [AtomicU32; N] {
    core::array::from_fn(|_| AtomicU32::new(init))
}

fn unsafe_cell_array<T, const N: usize>() -> [UnsafeCell<MaybeUninit<T>>; N] {
    core::array::from_fn(|_| UnsafeCell::new(MaybeUninit::uninit()))
}

#[must_use]
#[derive(Copy, Clone, Debug)]
pub struct PollStats {
    pub read: usize,
    pub dropped: usize,
    pub newest: u32,
}

/// Overwrite ring for SPSC high-rate telemetry.
/// Producer never waits; consumer may drop if it lags > N.
pub struct SeqRing<T: Copy, const N: usize> {
    next_seq: AtomicU32,
    published_seq: AtomicU32,
    slot_seq: [AtomicU32; N],
    slots: [UnsafeCell<MaybeUninit<T>>; N],
}

unsafe impl<T: Copy + Send, const N: usize> Sync for SeqRing<T, N> {}

impl<T: Copy, const N: usize> SeqRing<T, N> {
    pub fn new() -> Self {
        assert!(N > 0);
        Self {
            next_seq: AtomicU32::new(0),
            published_seq: AtomicU32::new(0),
            slot_seq: atomic_u32_array::<N>(0),
            slots: unsafe_cell_array::<T, N>(),
        }
    }

    #[inline(always)]
    const fn idx_for(seq: u32) -> usize {
        ((seq.wrapping_sub(1)) as usize) % N
    }

    /// Create the producer handle. Only one producer may be active.
    #[inline]
    pub fn producer(&self) -> Producer<'_, T, N> {
        Producer { ring: self }
    }

    /// Create the consumer handle. Only one consumer may be active.
    #[inline]
    pub fn consumer(&self) -> Consumer<'_, T, N> {
        Consumer {
            ring: self,
            last_seq: 0,
            dropped_accum: 0,
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
        unsafe { (*self.slots[idx].get()).as_mut_ptr().write(value) };

        self.slot_seq[idx].store(seq, Ordering::Release);
        self.published_seq.store(seq, Ordering::Release);
        seq
    }

    #[inline]
    fn read_seq_inner(&self, seq: u32) -> Option<T> {
        let idx = Self::idx_for(seq);

        let s1 = self.slot_seq[idx].load(Ordering::Acquire);
        if s1 != seq {
            return None;
        }

        let v = unsafe { (*self.slots[idx].get()).assume_init_read() };

        let s2 = self.slot_seq[idx].load(Ordering::Acquire);
        if s2 != seq {
            return None;
        }

        Some(v)
    }
}

pub struct Producer<'a, T: Copy, const N: usize> {
    ring: &'a SeqRing<T, N>,
}

impl<'a, T: Copy, const N: usize> Producer<'a, T, N> {
    #[inline]
    pub fn push(&self, value: T) -> u32 {
        self.ring.push_inner(value)
    }
}

pub struct Consumer<'a, T: Copy, const N: usize> {
    ring: &'a SeqRing<T, N>,
    last_seq: u32,
    dropped_accum: usize,
}

impl<'a, T: Copy, const N: usize> Consumer<'a, T, N> {
    /// How many items have been dropped since consumer creation (or since reset).
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
    /// Hook sees `&T` but it’s a reference to a **local copy** inside poll.
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

            let lag = newest.wrapping_sub(self.last_seq) as usize;
            if lag > N {
                let next = self.last_seq.wrapping_add(1);
                let keep_from = newest.wrapping_sub((N - 1) as u32);
                let jump_drops = keep_from.wrapping_sub(next) as usize;
                dropped += jump_drops;
                self.last_seq = keep_from.wrapping_sub(1);
                continue;
            }

            let next = self.last_seq.wrapping_add(1);

            match self.ring.read_seq_inner(next) {
                Some(v) => {
                    hook(next, &v);
                    self.last_seq = next;
                    read += 1;
                }
                None => {
                    self.last_seq = next;
                    dropped += 1;
                }
            }
        }

        self.dropped_accum += dropped;

        PollStats {
            read,
            dropped,
            newest,
        }
    }

    /// “Give me the newest thing right now” (not in-order).
    /// Returns true if it delivered something.
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
    #[inline]
    pub fn skip_to_latest(&mut self) {
        let newest = self.ring.newest_seq();
        if newest != 0 {
            self.last_seq = newest.wrapping_sub(1);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::SeqRing;
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
}
