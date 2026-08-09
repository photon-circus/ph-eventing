//! Bounded SPSC event buffer with backpressure — no heap, no alloc.
//!
//! [`EventBuf`] is a fixed-size, lock-free, single-producer single-consumer
//! ring buffer that **rejects** pushes when full instead of overwriting.
//! This gives the producer explicit backpressure so no events are silently
//! lost.
//!
//! # When to use
//! Use `EventBuf` when every event matters and the producer can afford to
//! handle a "buffer full" signal (retry, log, or apply its own policy).
//! If losing old events is acceptable, prefer [`crate::SeqRing`].
//! If you only need a single-owner ring, see [`crate::RingBuf`].
//!
//! # Memory ordering
//! This is a classic Lamport SPSC queue:
//! - The producer owns `head` (Relaxed load, Release store) and reads
//!   `tail` with Acquire to see consumer progress.
//! - The consumer owns `tail` (Relaxed load, Release store) and reads
//!   `head` with Acquire to see producer progress.
//! - A slot is written before `head` is advanced and read before `tail` is
//!   advanced, so the Release/Acquire pairs on the cursors act as the
//!   publication fence.
//!
//! # Example
//! ```
//! use ph_eventing::EventBuf;
//!
//! let buf = EventBuf::<u32, 4>::new();
//! let producer = buf.producer();
//! let consumer = buf.consumer();
//!
//! assert!(producer.push(1).is_ok());
//! assert!(producer.push(2).is_ok());
//! assert_eq!(consumer.pop(), Some(1));
//! assert_eq!(consumer.pop(), Some(2));
//! assert_eq!(consumer.pop(), None); // empty
//! ```

use core::cell::{Cell, UnsafeCell};
use core::marker::PhantomData;
use core::mem::MaybeUninit;
use core::sync::atomic::Ordering;
#[cfg(target_has_atomic = "32")]
use core::sync::atomic::{AtomicBool, AtomicU32};
#[cfg(all(not(target_has_atomic = "32"), feature = "portable-atomic"))]
use portable_atomic::{AtomicBool, AtomicU32};

fn unsafe_cell_array<T, const N: usize>() -> [UnsafeCell<MaybeUninit<T>>; N] {
    core::array::from_fn(|_| UnsafeCell::new(MaybeUninit::uninit()))
}

/// Bounded SPSC event buffer with backpressure.
///
/// When the buffer is full, [`Producer::push`] returns `Err(val)` instead
/// of overwriting, giving the producer a chance to retry, drop, or log.
/// The consumer drains items with [`Consumer::pop`] or [`Consumer::drain`].
///
/// # Panics
/// - `EventBuf::new()` panics if `N == 0`.
/// - `producer()` / `consumer()` panic if called while another handle of
///   the same kind is already active.
pub struct EventBuf<T: Copy, const N: usize> {
    head: AtomicU32,
    tail: AtomicU32,
    slots: [UnsafeCell<MaybeUninit<T>>; N],
    producer_taken: AtomicBool,
    consumer_taken: AtomicBool,
}

// SAFETY: EventBuf is Sync because the producer/consumer handles enforce
// SPSC usage, and the head/tail cursors are accessed via atomics with
// Release/Acquire ordering that guarantees slot visibility. T: Send ensures
// values can be transferred across threads safely.
unsafe impl<T: Copy + Send, const N: usize> Sync for EventBuf<T, N> {}

impl<T: Copy, const N: usize> EventBuf<T, N> {
    /// Create a new, empty event buffer.
    ///
    /// # Panics
    /// Panics if `N == 0`.
    pub fn new() -> Self {
        assert!(N > 0, "EventBuf capacity N must be > 0");
        Self {
            head: AtomicU32::new(0),
            tail: AtomicU32::new(0),
            slots: unsafe_cell_array::<T, N>(),
            producer_taken: AtomicBool::new(false),
            consumer_taken: AtomicBool::new(false),
        }
    }

    #[inline(always)]
    const fn slot_index(pos: u32) -> usize {
        (pos as usize) % N
    }

    /// Maximum number of items the buffer can hold.
    #[inline]
    pub const fn capacity(&self) -> usize {
        N
    }

    /// Approximate number of items currently buffered.
    ///
    /// Returns a consistent `(tail, head)` snapshot (retrying if `tail` moves
    /// mid-load). The value may still be stale by the time the caller acts on
    /// it, but it will not spuriously exceed [`capacity`](Self::capacity).
    #[inline]
    pub fn len(&self) -> usize {
        // Load tail around head so a concurrent consumer advance cannot make
        // `head.wrapping_sub(tail)` look like a huge unsigned value.
        loop {
            let t1 = self.tail.load(Ordering::Acquire);
            let h = self.head.load(Ordering::Acquire);
            let t2 = self.tail.load(Ordering::Acquire);
            if t1 == t2 {
                return h.wrapping_sub(t1) as usize;
            }
        }
    }

    /// Returns `true` if the buffer contains no items (approximate).
    #[inline]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Returns `true` if the buffer is at capacity (approximate).
    #[inline]
    pub fn is_full(&self) -> bool {
        self.len() >= N
    }

    /// Create the producer handle. Only one producer may be active.
    ///
    /// # Panics
    /// Panics if a producer handle is already active.
    #[inline]
    pub fn producer(&self) -> Producer<'_, T, N> {
        assert!(
            !self.producer_taken.swap(true, Ordering::AcqRel),
            "EventBuf: only one Producer may be active at a time"
        );
        Producer {
            buf: self,
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
            "EventBuf: only one Consumer may be active at a time"
        );
        Consumer {
            buf: self,
            _not_sync: PhantomData,
        }
    }
}

impl<T: Copy, const N: usize> Default for EventBuf<T, N> {
    fn default() -> Self {
        Self::new()
    }
}

impl<T: Copy, const N: usize> core::fmt::Debug for EventBuf<T, N> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("EventBuf")
            .field("len", &self.len())
            .field("capacity", &N)
            .finish()
    }
}

/// Write handle for an [`EventBuf`].
///
/// Dropping the producer releases the slot so a new one can be created.
pub struct Producer<'a, T: Copy, const N: usize> {
    buf: &'a EventBuf<T, N>,
    _not_sync: PhantomData<Cell<()>>,
}

impl<T: Copy, const N: usize> Producer<'_, T, N> {
    /// Try to push a value into the buffer.
    ///
    /// Returns `Ok(())` on success, or `Err(val)` if the buffer is full
    /// (the value is returned to the caller so nothing is lost).
    #[inline]
    pub fn push(&self, val: T) -> Result<(), T> {
        let head = self.buf.head.load(Ordering::Relaxed);
        let tail = self.buf.tail.load(Ordering::Acquire);
        if head.wrapping_sub(tail) as usize >= N {
            return Err(val);
        }
        let idx = EventBuf::<T, N>::slot_index(head);
        // SAFETY: producer is the only writer to this slot; the consumer
        // will not read it until head is advanced (Release below).
        unsafe {
            (*self.buf.slots[idx].get()).write(val);
        }
        self.buf.head.store(head.wrapping_add(1), Ordering::Release);
        Ok(())
    }
}

impl<T: Copy, const N: usize> Drop for Producer<'_, T, N> {
    fn drop(&mut self) {
        self.buf.producer_taken.store(false, Ordering::Release);
    }
}

impl<T: Copy, const N: usize> core::fmt::Debug for Producer<'_, T, N> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("event_buf::Producer")
            .field("capacity", &N)
            .finish()
    }
}

/// Read handle for an [`EventBuf`].
///
/// Dropping the consumer releases the slot so a new one can be created.
pub struct Consumer<'a, T: Copy, const N: usize> {
    buf: &'a EventBuf<T, N>,
    _not_sync: PhantomData<Cell<()>>,
}

impl<T: Copy, const N: usize> Consumer<'_, T, N> {
    /// Pop the oldest item from the buffer.
    ///
    /// Returns `None` if the buffer is empty.
    #[inline]
    pub fn pop(&self) -> Option<T> {
        let tail = self.buf.tail.load(Ordering::Relaxed);
        let head = self.buf.head.load(Ordering::Acquire);
        if tail == head {
            return None;
        }
        let idx = EventBuf::<T, N>::slot_index(tail);
        // SAFETY: consumer is the only reader of this slot; the producer
        // will not overwrite it until tail is advanced (Release below).
        let val = unsafe { (*self.buf.slots[idx].get()).assume_init_read() };
        self.buf.tail.store(tail.wrapping_add(1), Ordering::Release);
        Some(val)
    }

    /// Drain up to `max` items, passing each to `hook`.
    ///
    /// Returns the number of items consumed.
    #[inline]
    pub fn drain(&self, max: usize, mut hook: impl FnMut(T)) -> usize {
        let mut count = 0;
        while count < max {
            match self.pop() {
                Some(val) => {
                    hook(val);
                    count += 1;
                }
                None => break,
            }
        }
        count
    }
}

impl<T: Copy, const N: usize> Drop for Consumer<'_, T, N> {
    fn drop(&mut self) {
        self.buf.consumer_taken.store(false, Ordering::Release);
    }
}

impl<T: Copy, const N: usize> core::fmt::Debug for Consumer<'_, T, N> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("event_buf::Consumer")
            .field("capacity", &N)
            .finish()
    }
}

impl<T: Copy, const N: usize> crate::traits::Sink<T> for Producer<'_, T, N> {
    type Error = T;

    #[inline]
    fn try_push(&mut self, val: T) -> Result<(), T> {
        self.push(val)
    }
}

impl<T: Copy, const N: usize> crate::traits::Source<T> for Consumer<'_, T, N> {
    #[inline]
    fn try_pop(&mut self) -> Option<T> {
        self.pop()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_buf_is_empty() {
        let buf = EventBuf::<u32, 4>::new();
        assert!(buf.is_empty());
        assert!(!buf.is_full());
        assert_eq!(buf.len(), 0);
        assert_eq!(buf.capacity(), 4);
    }

    #[test]
    fn push_and_pop_fifo() {
        let buf = EventBuf::<u32, 4>::new();
        let p = buf.producer();
        let c = buf.consumer();

        assert!(p.push(10).is_ok());
        assert!(p.push(20).is_ok());
        assert!(p.push(30).is_ok());

        assert_eq!(c.pop(), Some(10));
        assert_eq!(c.pop(), Some(20));
        assert_eq!(c.pop(), Some(30));
        assert_eq!(c.pop(), None);
    }

    #[test]
    fn push_rejects_when_full() {
        let buf = EventBuf::<u32, 2>::new();
        let p = buf.producer();
        let c = buf.consumer();

        assert!(p.push(1).is_ok());
        assert!(p.push(2).is_ok());
        assert_eq!(p.push(3), Err(3)); // full — value returned

        // drain one, then push succeeds
        assert_eq!(c.pop(), Some(1));
        assert!(p.push(3).is_ok());
    }

    #[test]
    fn drain_returns_count() {
        let buf = EventBuf::<u32, 8>::new();
        let p = buf.producer();
        let c = buf.consumer();

        for i in 0..5 {
            p.push(i).unwrap();
        }

        let mut out = std::vec::Vec::new();
        let n = c.drain(3, |v| out.push(v));
        assert_eq!(n, 3);
        assert_eq!(out, [0, 1, 2]);

        // remaining
        let n = c.drain(100, |v| out.push(v));
        assert_eq!(n, 2);
        assert_eq!(out, [0, 1, 2, 3, 4]);
    }

    #[test]
    fn drain_on_empty_returns_zero() {
        let buf = EventBuf::<u32, 4>::new();
        let _p = buf.producer();
        let c = buf.consumer();

        let n = c.drain(10, |_| panic!("should not be called"));
        assert_eq!(n, 0);
    }

    #[test]
    fn producer_consumer_can_be_recreated() {
        let buf = EventBuf::<u32, 4>::new();
        {
            let p = buf.producer();
            p.push(1).unwrap();
        }
        // producer dropped — can create a new one
        let p = buf.producer();
        p.push(2).unwrap();

        {
            let c = buf.consumer();
            assert_eq!(c.pop(), Some(1));
        }
        // consumer dropped — can create a new one
        let c = buf.consumer();
        assert_eq!(c.pop(), Some(2));
        assert_eq!(c.pop(), None);
    }

    #[test]
    #[should_panic(expected = "only one Producer")]
    fn double_producer_panics() {
        let buf = EventBuf::<u32, 4>::new();
        let _p1 = buf.producer();
        let _p2 = buf.producer();
    }

    #[test]
    #[should_panic(expected = "only one Consumer")]
    fn double_consumer_panics() {
        let buf = EventBuf::<u32, 4>::new();
        let _c1 = buf.consumer();
        let _c2 = buf.consumer();
    }

    #[test]
    fn wraps_around_correctly() {
        let buf = EventBuf::<u32, 3>::new();
        let p = buf.producer();
        let c = buf.consumer();

        // fill, drain, fill again — exercises the wrap
        for round in 0u32..4 {
            let base = round * 3;
            for i in 0..3 {
                assert!(p.push(base + i).is_ok());
            }
            assert_eq!(p.push(99), Err(99)); // full
            for i in 0..3 {
                assert_eq!(c.pop(), Some(base + i));
            }
            assert_eq!(c.pop(), None); // empty
        }
    }

    #[test]
    fn default_is_new() {
        let buf: EventBuf<u8, 4> = EventBuf::default();
        assert!(buf.is_empty());
    }

    #[test]
    fn len_and_full_track_state() {
        let buf = EventBuf::<u32, 3>::new();
        let p = buf.producer();
        let c = buf.consumer();

        assert_eq!(buf.len(), 0);
        assert!(buf.is_empty());

        p.push(1).unwrap();
        assert_eq!(buf.len(), 1);

        p.push(2).unwrap();
        p.push(3).unwrap();
        assert_eq!(buf.len(), 3);
        assert!(buf.is_full());

        c.pop();
        assert_eq!(buf.len(), 2);
        assert!(!buf.is_full());
    }

    #[test]
    fn handles_are_send() {
        fn assert_send<T: Send>() {}
        assert_send::<super::Producer<'_, u32, 4>>();
        assert_send::<super::Consumer<'_, u32, 4>>();
    }
}
