//! Exploratory bounded SPSC ownership transfer over reusable slots.
//!
//! This module is an evaluation vehicle for the `SlotPool` proposal. It is
//! available to downstream experiments only with the
//! `experimental-slot-pool` feature and is not yet a committed public API.
//!
//! Slots are supplied fully initialized. A producer grant mutably borrows one
//! free slot and publishes it on commit. A consumer claim immutably borrows the
//! oldest published slot and releases it on drop. Cancelling a grant does not
//! roll back mutations; it only keeps the slot unpublished. This restriction
//! intentionally postpones the generic `MaybeUninit<T>` API until its safety
//! contract can be proved.

use crate::sync::{AtomicBool, AtomicU32, Ordering};
use core::cell::{Cell, UnsafeCell};
use core::marker::PhantomData;
use core::ops::{Deref, DerefMut};

/// A fixed-capacity SPSC pool of pre-initialized reusable slots.
///
/// `SlotPool` is FIFO and applies backpressure: reservation returns `None`
/// while all slots are published or consumer-owned. Operations do a fixed
/// number of atomic accesses and never scan the pool.
///
/// This is an exploratory API. Enable `experimental-slot-pool` to use it
/// outside this crate's tests.
pub struct SlotPool<T, const N: usize> {
    head: AtomicU32,
    tail: AtomicU32,
    slots: UnsafeCell<[T; N]>,
    producer_taken: AtomicBool,
    consumer_taken: AtomicBool,
}

// SAFETY: the handles enforce SPSC use. A producer can mutably access only the
// free slot at `head`; a consumer can immutably access only the published slot
// at `tail`. Release/Acquire cursor pairs prevent either endpoint from
// accessing a slot before the other endpoint relinquishes it. `T: Send` is
// required because ownership of each slot crosses contexts.
unsafe impl<T: Send, const N: usize> Sync for SlotPool<T, N> {}

impl<T, const N: usize> SlotPool<T, N> {
    /// Construct a pool from `N` initialized reusable slots.
    ///
    /// On a normal build this is const, permitting statically allocated pools.
    /// `N` must be in `1..=u32::MAX`; the cursor representation cannot express
    /// a larger bounded occupancy.
    #[cfg(not(loom))]
    pub const fn new(slots: [T; N]) -> Self {
        const {
            assert!(N > 0, "SlotPool capacity N must be > 0");
            assert!(N <= u32::MAX as usize, "SlotPool capacity must fit in u32");
        }
        Self {
            head: AtomicU32::new(0),
            tail: AtomicU32::new(0),
            slots: UnsafeCell::new(slots),
            producer_taken: AtomicBool::new(false),
            consumer_taken: AtomicBool::new(false),
        }
    }

    /// Construct a pool in a Loom model.
    ///
    /// # Panics
    /// Panics when `N == 0` or `N` does not fit in the cursor representation.
    #[cfg(loom)]
    pub fn new(slots: [T; N]) -> Self {
        assert!(N > 0, "SlotPool capacity N must be > 0");
        assert!(N <= u32::MAX as usize, "SlotPool capacity must fit in u32");
        Self {
            head: AtomicU32::new(0),
            tail: AtomicU32::new(0),
            slots: UnsafeCell::new(slots),
            producer_taken: AtomicBool::new(false),
            consumer_taken: AtomicBool::new(false),
        }
    }

    /// Return the number of reusable slots.
    #[inline]
    pub const fn capacity(&self) -> usize {
        N
    }

    /// Try to acquire the single producer handle.
    #[inline]
    pub fn try_producer(&self) -> Option<Producer<'_, T, N>> {
        if self.producer_taken.swap(true, Ordering::AcqRel) {
            None
        } else {
            Some(Producer {
                pool: self,
                _not_sync: PhantomData,
            })
        }
    }

    /// Try to acquire the single consumer handle.
    #[inline]
    pub fn try_consumer(&self) -> Option<Consumer<'_, T, N>> {
        if self.consumer_taken.swap(true, Ordering::AcqRel) {
            None
        } else {
            Some(Consumer {
                pool: self,
                _not_sync: PhantomData,
            })
        }
    }

    #[inline(always)]
    const fn slot_index(position: u32) -> usize {
        (position as usize) % N
    }

    /// Borrow one slot through its raw backing storage.
    ///
    /// # Safety
    /// The caller must own `index` in the cursor state machine and must not
    /// create an overlapping mutable or shared reference to that slot.
    #[inline(always)]
    unsafe fn slot_ref(&self, index: usize) -> &T {
        let base = self.slots.get().cast::<T>();
        // SAFETY: upheld by the caller; `[T; N]` is contiguous and index is
        // produced modulo N.
        unsafe { &*base.add(index) }
    }
}

impl<T, const N: usize> core::fmt::Debug for SlotPool<T, N> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("SlotPool").field("capacity", &N).finish()
    }
}

/// The single producer endpoint for a [`SlotPool`].
pub struct Producer<'pool, T, const N: usize> {
    pool: &'pool SlotPool<T, N>,
    _not_sync: PhantomData<Cell<()>>,
}

impl<'pool, T, const N: usize> Producer<'pool, T, N> {
    /// Try to reserve the next free slot.
    ///
    /// Returns `None` when every slot is published or consumer-owned. The
    /// returned grant borrows this handle, so only one reservation can be in
    /// flight from a producer.
    #[inline]
    pub fn try_reserve(&mut self) -> Option<Grant<'_, 'pool, T, N>> {
        let head = self.pool.head.load(Ordering::Relaxed);
        let tail = self.pool.tail.load(Ordering::Acquire);
        if head.wrapping_sub(tail) as usize >= N {
            return None;
        }
        Some(Grant {
            producer: self,
            position: head,
            committed: false,
            _not_send_or_sync: PhantomData,
        })
    }
}

impl<T, const N: usize> Drop for Producer<'_, T, N> {
    fn drop(&mut self) {
        self.pool.producer_taken.store(false, Ordering::Release);
    }
}

impl<T, const N: usize> core::fmt::Debug for Producer<'_, T, N> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("slot_pool::Producer")
            .field("capacity", &N)
            .finish()
    }
}

/// Exclusive producer access to one free reusable slot.
///
/// Call [`commit`](Self::commit) to publish it. Dropping the grant cancels the
/// reservation in bounded time; mutations remain in the reusable slot but are
/// not observable by the consumer.
pub struct Grant<'grant, 'pool, T, const N: usize> {
    producer: &'grant mut Producer<'pool, T, N>,
    position: u32,
    committed: bool,
    _not_send_or_sync: PhantomData<*mut ()>,
}

impl<T, const N: usize> Grant<'_, '_, T, N> {
    /// Publish this slot as the newest FIFO entry.
    #[inline]
    pub fn commit(mut self) {
        self.producer
            .pool
            .head
            .store(self.position.wrapping_add(1), Ordering::Release);
        self.committed = true;
    }

    /// Cancel this reservation explicitly.
    #[inline]
    pub fn cancel(self) {}
}

impl<T, const N: usize> Deref for Grant<'_, '_, T, N> {
    type Target = T;

    #[inline]
    fn deref(&self) -> &T {
        let index = SlotPool::<T, N>::slot_index(self.position);
        // SAFETY: an uncommitted grant exclusively owns the free slot at head.
        // The consumer cannot reach it until `commit` publishes head.
        unsafe { self.producer.pool.slot_ref(index) }
    }
}

impl<T, const N: usize> DerefMut for Grant<'_, '_, T, N> {
    #[inline]
    fn deref_mut(&mut self) -> &mut T {
        let index = SlotPool::<T, N>::slot_index(self.position);
        let base = self.producer.pool.slots.get().cast::<T>();
        // SAFETY: the grant is unique because it borrows the sole producer
        // mutably. This position is outside the published cursor range, and
        // `[T; N]` is contiguous with index produced modulo N.
        unsafe { &mut *base.add(index) }
    }
}

impl<T, const N: usize> core::fmt::Debug for Grant<'_, '_, T, N> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("slot_pool::Grant")
            .field("position", &self.position)
            .field("committed", &self.committed)
            .finish()
    }
}

/// The single consumer endpoint for a [`SlotPool`].
pub struct Consumer<'pool, T, const N: usize> {
    pool: &'pool SlotPool<T, N>,
    _not_sync: PhantomData<Cell<()>>,
}

impl<'pool, T, const N: usize> Consumer<'pool, T, N> {
    /// Try to claim the oldest published slot.
    ///
    /// The claim borrows this handle, so at most one claim is in flight. It
    /// releases the slot automatically on drop.
    #[inline]
    pub fn try_claim(&mut self) -> Option<Claim<'_, 'pool, T, N>> {
        let tail = self.pool.tail.load(Ordering::Relaxed);
        let head = self.pool.head.load(Ordering::Acquire);
        if tail == head {
            return None;
        }
        Some(Claim {
            consumer: self,
            position: tail,
            _not_send_or_sync: PhantomData,
        })
    }
}

impl<T, const N: usize> Drop for Consumer<'_, T, N> {
    fn drop(&mut self) {
        self.pool.consumer_taken.store(false, Ordering::Release);
    }
}

impl<T, const N: usize> core::fmt::Debug for Consumer<'_, T, N> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("slot_pool::Consumer")
            .field("capacity", &N)
            .finish()
    }
}

/// Shared consumer access to the oldest published slot.
///
/// Dropping the claim releases the slot back to the producer. A claim cannot
/// outlive or be used concurrently through its consumer handle.
pub struct Claim<'claim, 'pool, T, const N: usize> {
    consumer: &'claim mut Consumer<'pool, T, N>,
    position: u32,
    _not_send_or_sync: PhantomData<*mut ()>,
}

impl<T, const N: usize> Claim<'_, '_, T, N> {
    /// Release this claim explicitly.
    #[inline]
    pub fn release(self) {}
}

impl<T, const N: usize> Deref for Claim<'_, '_, T, N> {
    type Target = T;

    #[inline]
    fn deref(&self) -> &T {
        let index = SlotPool::<T, N>::slot_index(self.position);
        // SAFETY: the claim owns the published slot at tail. The producer
        // cannot reuse it until this claim's Drop advances tail.
        unsafe { self.consumer.pool.slot_ref(index) }
    }
}

impl<T, const N: usize> Drop for Claim<'_, '_, T, N> {
    #[inline]
    fn drop(&mut self) {
        self.consumer
            .pool
            .tail
            .store(self.position.wrapping_add(1), Ordering::Release);
    }
}

impl<T, const N: usize> core::fmt::Debug for Claim<'_, '_, T, N> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("slot_pool::Claim")
            .field("position", &self.position)
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn committed_slots_are_claimed_fifo() {
        let pool = SlotPool::new([[0_u8; 8]; 2]);
        let mut producer = pool.try_producer().unwrap();
        let mut consumer = pool.try_consumer().unwrap();

        let mut first = producer.try_reserve().unwrap();
        first[0] = 10;
        first.commit();
        let mut second = producer.try_reserve().unwrap();
        second[0] = 20;
        second.commit();

        assert_eq!(consumer.try_claim().unwrap()[0], 10);
        assert_eq!(consumer.try_claim().unwrap()[0], 20);
        assert!(consumer.try_claim().is_none());
    }

    #[test]
    fn dropped_grant_is_not_published_and_slot_is_reused() {
        let pool = SlotPool::new([[0_u8; 4]]);
        let mut producer = pool.try_producer().unwrap();
        let mut consumer = pool.try_consumer().unwrap();

        producer.try_reserve().unwrap()[0] = 7;
        assert!(consumer.try_claim().is_none());

        let grant = producer.try_reserve().unwrap();
        assert_eq!(grant[0], 7);
        grant.commit();
        assert_eq!(consumer.try_claim().unwrap()[0], 7);
    }

    #[test]
    fn held_claim_applies_backpressure_until_drop() {
        let pool = SlotPool::new([[0_u8; 4]]);
        let mut producer = pool.try_producer().unwrap();
        let mut consumer = pool.try_consumer().unwrap();

        producer.try_reserve().unwrap().commit();
        let claim = consumer.try_claim().unwrap();
        assert!(producer.try_reserve().is_none());
        drop(claim);
        assert!(producer.try_reserve().is_some());
    }

    #[test]
    fn non_copy_slots_drop_once_with_the_pool() {
        struct DropSpy<'a>(&'a Cell<usize>);
        impl Drop for DropSpy<'_> {
            fn drop(&mut self) {
                self.0.set(self.0.get() + 1);
            }
        }

        let drops = Cell::new(0);
        {
            let pool = SlotPool::new([DropSpy(&drops), DropSpy(&drops)]);
            let mut producer = pool.try_producer().unwrap();
            let mut consumer = pool.try_consumer().unwrap();
            producer.try_reserve().unwrap().commit();
            drop(consumer.try_claim().unwrap());
        }
        assert_eq!(drops.get(), 2);
    }

    #[test]
    fn endpoint_handles_are_unique_and_reacquirable() {
        fn assert_send<T: Send>() {}
        fn assert_sync<T: Sync>() {}
        assert_send::<Producer<'_, u32, 2>>();
        assert_send::<Consumer<'_, u32, 2>>();
        assert_sync::<SlotPool<u32, 2>>();

        let pool = SlotPool::new([0_u32; 2]);
        let producer = pool.try_producer().unwrap();
        assert!(pool.try_producer().is_none());
        let consumer = pool.try_consumer().unwrap();
        assert!(pool.try_consumer().is_none());
        drop(producer);
        drop(consumer);
        assert!(pool.try_producer().is_some());
        assert!(pool.try_consumer().is_some());
    }

    #[test]
    fn concurrent_transfer_preserves_fifo_and_exclusivity() {
        let count = crate::test_support::iterations(5_000);
        let pool = SlotPool::new([0_u32; 2]);
        let mut producer = pool.try_producer().unwrap();
        let mut consumer = pool.try_consumer().unwrap();

        std::thread::scope(|scope| {
            scope.spawn(move || {
                for expected in 0..count {
                    loop {
                        if let Some(mut grant) = producer.try_reserve() {
                            *grant = expected;
                            grant.commit();
                            break;
                        }
                        std::thread::yield_now();
                    }
                }
            });
            scope.spawn(move || {
                for expected in 0..count {
                    loop {
                        if let Some(claim) = consumer.try_claim() {
                            assert_eq!(*claim, expected);
                            break;
                        }
                        std::thread::yield_now();
                    }
                }
            });
        });
    }
}
