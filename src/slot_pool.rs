//! Exploratory bounded SPSC ownership transfer over reusable slots.
//!
//! This module is an evaluation vehicle for the `SlotPool` proposal. It is
//! available to downstream experiments only with the
//! `experimental-slot-pool` feature and is not yet a committed public API.
//!
//! A pool may start with initialized slots via [`SlotPool::new`] or with
//! generic uninitialized storage via [`SlotPool::new_uninit`]. Reservation is
//! type-state checked: a vacant slot cannot be committed until
//! [`VacantGrant::write`] constructs a complete `T`. Consequently every
//! [`Claim`] points at a valid initialized value.
//!
//! Once initialized, a slot stays initialized for the pool's lifetime. A
//! producer mutates it through a [`Grant`] and publishes it on commit. A
//! consumer reads the oldest published slot through a claim and releases it on
//! drop. Cancelling a grant does not roll back mutations and never invokes
//! `T::drop`; it only keeps the slot unpublished.

use crate::sync::{AtomicBool, AtomicU32, Ordering, TrackedCell};
use core::cell::Cell;
use core::marker::PhantomData;
use core::mem::MaybeUninit;
#[cfg(not(loom))]
use core::ops::{Deref, DerefMut};

/// A fixed-capacity SPSC pool of reusable slots.
///
/// `SlotPool` is FIFO and applies backpressure: reservation returns `None`
/// while all slots are published or consumer-owned. Operations do a fixed
/// number of atomic accesses and never scan the pool.
///
/// Slots may be supplied initialized with [`new`](Self::new), or constructed
/// lazily with [`new_uninit`](Self::new_uninit). In the latter case a vacant
/// reservation must pass through [`VacantGrant::write`] before it can be
/// committed. Initialization proceeds in FIFO slot order, so one prefix count
/// tracks teardown obligations without adding per-slot state.
///
/// This is an exploratory API. Enable `experimental-slot-pool` to use it
/// outside this crate's tests.
pub struct SlotPool<T, const N: usize> {
    head: AtomicU32,
    tail: AtomicU32,
    slots: TrackedCell<MaybeUninit<[T; N]>>,
    initialized: TrackedCell<u32>,
    producer_taken: AtomicBool,
    consumer_taken: AtomicBool,
}

// SAFETY: the handles enforce SPSC use. A producer can mutably access only the
// free slot at `head`; a consumer can immutably access only the published slot
// at `tail`. Release/Acquire cursor pairs prevent either endpoint from
// accessing a slot before the other endpoint relinquishes it. Only the sole
// producer touches `initialized`, and the pool cannot be dropped while that
// producer exists. `T: Send` is required because ownership of each slot crosses
// contexts.
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
            slots: TrackedCell::new(MaybeUninit::new(slots)),
            initialized: TrackedCell::new(N as u32),
            producer_taken: AtomicBool::new(false),
            consumer_taken: AtomicBool::new(false),
        }
    }

    /// Construct a pool whose slots have not yet been initialized.
    ///
    /// [`Producer::try_reserve`] returns [`Reservation::Vacant`] for each slot
    /// until [`VacantGrant::write`] constructs its first `T`. A vacant grant has
    /// no `commit` method, so safe code cannot publish uninitialized storage.
    /// Once constructed, a slot is reused through ordinary mutable [`Grant`]
    /// access and is dropped exactly once with the pool.
    ///
    /// On a normal build this is const, permitting statically allocated pools.
    /// `N` must be in `1..=u32::MAX`.
    ///
    /// # Example
    ///
    /// ```
    /// use ph_eventing::slot_pool::{Reservation, SlotPool};
    ///
    /// let pool = SlotPool::<[u8; 4], 1>::new_uninit();
    /// let mut producer = pool.try_producer().unwrap();
    /// let mut consumer = pool.try_consumer().unwrap();
    ///
    /// let Reservation::Vacant(vacant) = producer.try_reserve().unwrap() else {
    ///     unreachable!();
    /// };
    /// let mut grant = vacant.write([0; 4]);
    /// grant[0] = 7;
    /// grant.commit();
    /// assert_eq!(consumer.try_claim().unwrap()[0], 7);
    /// ```
    ///
    /// A vacant slot cannot be committed:
    ///
    /// ```compile_fail,E0599
    /// use ph_eventing::slot_pool::{Reservation, SlotPool};
    /// let pool = SlotPool::<u32, 1>::new_uninit();
    /// let mut producer = pool.try_producer().unwrap();
    /// let Reservation::Vacant(vacant) = producer.try_reserve().unwrap() else {
    ///     unreachable!();
    /// };
    /// vacant.commit();
    /// ```
    #[cfg(not(loom))]
    pub const fn new_uninit() -> Self {
        const {
            assert!(N > 0, "SlotPool capacity N must be > 0");
            assert!(N <= u32::MAX as usize, "SlotPool capacity must fit in u32");
        }
        Self {
            head: AtomicU32::new(0),
            tail: AtomicU32::new(0),
            slots: TrackedCell::new(MaybeUninit::uninit()),
            initialized: TrackedCell::new(0),
            producer_taken: AtomicBool::new(false),
            consumer_taken: AtomicBool::new(false),
        }
    }

    /// Construct an initialized pool in a Loom model.
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
            slots: TrackedCell::new(MaybeUninit::new(slots)),
            initialized: TrackedCell::new(N as u32),
            producer_taken: AtomicBool::new(false),
            consumer_taken: AtomicBool::new(false),
        }
    }

    /// Construct an uninitialized pool in a Loom model.
    ///
    /// # Panics
    /// Panics when `N == 0` or `N` does not fit in the cursor representation.
    #[cfg(loom)]
    pub fn new_uninit() -> Self {
        assert!(N > 0, "SlotPool capacity N must be > 0");
        assert!(N <= u32::MAX as usize, "SlotPool capacity must fit in u32");
        Self {
            head: AtomicU32::new(0),
            tail: AtomicU32::new(0),
            slots: TrackedCell::new(MaybeUninit::uninit()),
            initialized: TrackedCell::new(0),
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

    /// Whether the current producer position already contains a `T`.
    ///
    /// # Safety
    /// Only the active producer may call this. Before all slots are initialized,
    /// `head` cannot wrap or skip a slot: it advances only after the current
    /// slot is initialized and committed. The initialized slots therefore form
    /// exactly one prefix, including a value left by a cancelled first grant.
    #[inline(always)]
    unsafe fn position_is_initialized(&self, position: u32) -> bool {
        let initialized = self.initialized.with(|value| {
            // SAFETY: the sole producer owns this non-atomic field for its
            // entire handle lifetime; pool drop cannot overlap that borrow.
            unsafe { *value }
        });
        initialized == N as u32 || Self::slot_index(position) < initialized as usize
    }

    /// Extend the initialized prefix after constructing the current slot.
    ///
    /// # Safety
    /// The caller must be the sole producer, the current position must be the
    /// first vacant prefix position, and the slot value must already be a valid
    /// `T`. The type-state transition in [`VacantGrant::write`] is the only
    /// caller.
    #[inline(always)]
    unsafe fn mark_position_initialized(&self) {
        self.initialized.with_mut(|initialized| {
            // SAFETY: the sole producer owns this field. The prefix count is
            // below N, so adding one cannot overflow its u32 representation.
            unsafe { *initialized += 1 };
        });
    }

    /// Run `f` with a pointer to one slot's storage.
    ///
    /// # Safety
    /// The caller must own `index` in the cursor state machine. If `f` treats
    /// the storage as `T`, initialization must also be proved by grant type
    /// state or publication.
    #[inline(always)]
    unsafe fn with_slot<R>(&self, index: usize, f: impl FnOnce(*const MaybeUninit<T>) -> R) -> R {
        self.slots.with(|array| {
            let base = array.cast::<MaybeUninit<T>>();
            // SAFETY: `[T; N]` is contiguous, MaybeUninit preserves layout,
            // and the index is always produced modulo N.
            let slot = unsafe { base.add(index) };
            f(slot)
        })
    }

    /// Run `f` with a mutable pointer to one slot's storage.
    ///
    /// # Safety
    /// The caller must exclusively own `index` in the cursor state machine.
    #[inline(always)]
    unsafe fn with_slot_mut<R>(&self, index: usize, f: impl FnOnce(*mut MaybeUninit<T>) -> R) -> R {
        self.slots.with_mut(|array| {
            let base = array.cast::<MaybeUninit<T>>();
            // SAFETY: `[T; N]` is contiguous, MaybeUninit preserves layout,
            // and the index is always produced modulo N.
            let slot = unsafe { base.add(index) };
            f(slot)
        })
    }

    #[cfg(not(loom))]
    #[inline(always)]
    unsafe fn slot_ref(&self, index: usize) -> &T {
        // SAFETY: the caller proves both cursor ownership and initialization.
        unsafe { self.with_slot(index, |slot| &*slot.cast::<T>()) }
    }
}

impl<T, const N: usize> Drop for SlotPool<T, N> {
    fn drop(&mut self) {
        // The pool is exclusively borrowed, so no endpoint or guard exists.
        // Initialization is sequential and permanent, making the prefix count
        // an exact drop-obligation count even after grant cancellation.
        let initialized = self.initialized.with(|value| {
            // SAFETY: `&mut self` proves there are no endpoints or guards.
            unsafe { *value as usize }
        });
        self.slots.with_mut(|array| {
            let base = array.cast::<MaybeUninit<T>>();
            for index in 0..initialized {
                // SAFETY: every slot in the counted prefix contains one valid
                // T and no slot is dropped anywhere else.
                unsafe { base.add(index).cast::<T>().drop_in_place() };
            }
        });
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
    /// returned reservation borrows this handle, so only one reservation can
    /// be in flight from a producer. It is [`Reservation::Initialized`] for a
    /// reusable slot and [`Reservation::Vacant`] until that slot's first `T`
    /// has been constructed.
    #[inline]
    pub fn try_reserve(&mut self) -> Option<Reservation<'_, 'pool, T, N>> {
        let head = self.pool.head.load(Ordering::Relaxed);
        let tail = self.pool.tail.load(Ordering::Acquire);
        if head.wrapping_sub(tail) as usize >= N {
            return None;
        }

        // SAFETY: this is the sole producer handle and its mutable borrow keeps
        // another reservation from observing or changing the prefix state.
        let initialized = unsafe { self.pool.position_is_initialized(head) };
        if initialized {
            Some(Reservation::Initialized(Grant {
                producer: self,
                position: head,
                _not_send_or_sync: PhantomData,
            }))
        } else {
            Some(Reservation::Vacant(VacantGrant {
                producer: self,
                position: head,
                _not_send_or_sync: PhantomData,
            }))
        }
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

/// A reservation of the next free slot.
///
/// An initialized reservation contains a reusable [`Grant`]. A vacant
/// reservation contains a [`VacantGrant`] that must construct `T` before a
/// committable grant exists. This enum is the runtime observation feeding the
/// compile-time initialization witness.
pub enum Reservation<'grant, 'pool, T, const N: usize> {
    /// The slot contains a valid reusable `T`.
    Initialized(Grant<'grant, 'pool, T, N>),
    /// The slot has never contained a `T`.
    Vacant(VacantGrant<'grant, 'pool, T, N>),
}

impl<'grant, 'pool, T, const N: usize> Reservation<'grant, 'pool, T, N> {
    /// Return the initialized grant, or the vacant grant that must be written.
    #[inline]
    pub fn into_initialized(
        self,
    ) -> Result<Grant<'grant, 'pool, T, N>, VacantGrant<'grant, 'pool, T, N>> {
        match self {
            Self::Initialized(grant) => Ok(grant),
            Self::Vacant(vacant) => Err(vacant),
        }
    }

    /// Whether the reserved slot already contains a valid `T`.
    #[must_use]
    #[inline]
    pub const fn is_initialized(&self) -> bool {
        matches!(self, Self::Initialized(_))
    }

    /// Cancel this reservation explicitly.
    #[inline]
    pub fn cancel(self) {}
}

impl<T, const N: usize> core::fmt::Debug for Reservation<'_, '_, T, N> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::Initialized(grant) => f.debug_tuple("Initialized").field(grant).finish(),
            Self::Vacant(vacant) => f.debug_tuple("Vacant").field(vacant).finish(),
        }
    }
}

/// Exclusive producer ownership of a slot that has never contained a `T`.
///
/// This type deliberately has no `commit` method. [`write`](Self::write)
/// constructs a complete value and returns the initialized [`Grant`] that can
/// be published. Dropping or cancelling a vacant grant performs no work.
pub struct VacantGrant<'grant, 'pool, T, const N: usize> {
    producer: &'grant mut Producer<'pool, T, N>,
    position: u32,
    _not_send_or_sync: PhantomData<*mut ()>,
}

impl<'grant, 'pool, T, const N: usize> VacantGrant<'grant, 'pool, T, N> {
    /// Construct `value` directly in the reserved slot.
    ///
    /// This is the initialization witness: after `write` returns, the slot is
    /// permanently tracked as initialized and the returned grant can be
    /// mutated, committed, or cancelled. Cancelling it leaves the constructed
    /// value as reusable residue and does not invoke `T::drop`.
    #[inline]
    pub fn write(self, value: T) -> Grant<'grant, 'pool, T, N> {
        let index = SlotPool::<T, N>::slot_index(self.position);
        // SAFETY: a vacant grant exclusively owns the free slot at head and
        // type state proves there is no existing T to overwrite or drop.
        unsafe {
            self.producer.pool.with_slot_mut(index, |slot| {
                (*slot).write(value);
            });
            // The value is fully constructed before the prefix records its
            // drop obligation or any Grant can publish it.
            self.producer.pool.mark_position_initialized();
        }
        Grant {
            producer: self.producer,
            position: self.position,
            _not_send_or_sync: PhantomData,
        }
    }

    /// Cancel this vacant reservation explicitly.
    #[inline]
    pub fn cancel(self) {}
}

impl<T, const N: usize> core::fmt::Debug for VacantGrant<'_, '_, T, N> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("slot_pool::VacantGrant")
            .field("position", &self.position)
            .finish()
    }
}

/// Exclusive producer access to one initialized reusable slot.
///
/// Call [`commit`](Self::commit) to publish it. Dropping the grant cancels the
/// reservation in bounded time; mutations remain in the reusable slot but are
/// not observable by the consumer.
pub struct Grant<'grant, 'pool, T, const N: usize> {
    producer: &'grant mut Producer<'pool, T, N>,
    position: u32,
    _not_send_or_sync: PhantomData<*mut ()>,
}

impl<T, const N: usize> Grant<'_, '_, T, N> {
    /// Publish this slot as the newest FIFO entry.
    #[inline]
    pub fn commit(self) {
        self.producer
            .pool
            .head
            .store(self.position.wrapping_add(1), Ordering::Release);
    }

    /// Cancel this reservation explicitly.
    #[inline]
    pub fn cancel(self) {}

    /// Run a Loom-tracked read of the granted value.
    #[cfg(loom)]
    #[inline]
    pub(crate) fn with<R>(&self, f: impl FnOnce(&T) -> R) -> R {
        let index = SlotPool::<T, N>::slot_index(self.position);
        // SAFETY: Grant type state proves initialization and the grant owns the
        // slot exclusively until publication.
        unsafe {
            self.producer
                .pool
                .with_slot(index, |slot| f(&*slot.cast::<T>()))
        }
    }

    /// Run a Loom-tracked mutation of the granted value.
    #[cfg(loom)]
    #[inline]
    pub(crate) fn with_mut<R>(&mut self, f: impl FnOnce(&mut T) -> R) -> R {
        let index = SlotPool::<T, N>::slot_index(self.position);
        // SAFETY: Grant type state proves initialization and the mutable borrow
        // plus sole producer guarantee exclusive access.
        unsafe {
            self.producer
                .pool
                .with_slot_mut(index, |slot| f(&mut *slot.cast::<T>()))
        }
    }
}

#[cfg(not(loom))]
impl<T, const N: usize> Deref for Grant<'_, '_, T, N> {
    type Target = T;

    #[inline]
    fn deref(&self) -> &T {
        let index = SlotPool::<T, N>::slot_index(self.position);
        // SAFETY: Grant type state proves initialization. The consumer cannot
        // reach this free position until `commit` publishes head.
        unsafe { self.producer.pool.slot_ref(index) }
    }
}

#[cfg(not(loom))]
impl<T, const N: usize> DerefMut for Grant<'_, '_, T, N> {
    #[inline]
    fn deref_mut(&mut self) -> &mut T {
        let index = SlotPool::<T, N>::slot_index(self.position);
        // SAFETY: the grant is unique because it borrows the sole producer
        // mutably, and type state proves the slot is initialized.
        unsafe {
            self.producer
                .pool
                .with_slot_mut(index, |slot| &mut *slot.cast::<T>())
        }
    }
}

impl<T, const N: usize> core::fmt::Debug for Grant<'_, '_, T, N> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("slot_pool::Grant")
            .field("position", &self.position)
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
    /// releases the slot automatically on drop. Only an initialized [`Grant`]
    /// can advance `head`, so every returned claim contains a valid `T`.
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

/// Shared consumer access to the oldest published initialized slot.
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

    /// Run a Loom-tracked read of the claimed value.
    #[cfg(loom)]
    #[inline]
    pub(crate) fn with<R>(&self, f: impl FnOnce(&T) -> R) -> R {
        let index = SlotPool::<T, N>::slot_index(self.position);
        // SAFETY: publication proves initialization and the claim owns the
        // published tail slot until Drop advances tail.
        unsafe {
            self.consumer
                .pool
                .with_slot(index, |slot| f(&*slot.cast::<T>()))
        }
    }
}

#[cfg(not(loom))]
impl<T, const N: usize> Deref for Claim<'_, '_, T, N> {
    type Target = T;

    #[inline]
    fn deref(&self) -> &T {
        let index = SlotPool::<T, N>::slot_index(self.position);
        // SAFETY: only an initialized grant can publish this position. The
        // producer cannot reuse it until this claim's Drop advances tail.
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

#[cfg(all(test, not(loom)))]
mod tests {
    use super::*;

    fn initialized<'grant, 'pool, T, const N: usize>(
        reservation: Reservation<'grant, 'pool, T, N>,
    ) -> Grant<'grant, 'pool, T, N> {
        reservation.into_initialized().expect("initialized slot")
    }

    #[test]
    fn committed_slots_are_claimed_fifo() {
        let pool = SlotPool::new([[0_u8; 8]; 2]);
        let mut producer = pool.try_producer().unwrap();
        let mut consumer = pool.try_consumer().unwrap();

        let mut first = initialized(producer.try_reserve().unwrap());
        first[0] = 10;
        first.commit();
        let mut second = initialized(producer.try_reserve().unwrap());
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

        initialized(producer.try_reserve().unwrap())[0] = 7;
        assert!(consumer.try_claim().is_none());

        let grant = initialized(producer.try_reserve().unwrap());
        assert_eq!(grant[0], 7);
        grant.commit();
        assert_eq!(consumer.try_claim().unwrap()[0], 7);
    }

    #[test]
    fn held_claim_applies_backpressure_until_drop() {
        let pool = SlotPool::new([[0_u8; 4]]);
        let mut producer = pool.try_producer().unwrap();
        let mut consumer = pool.try_consumer().unwrap();

        initialized(producer.try_reserve().unwrap()).commit();
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
            initialized(producer.try_reserve().unwrap()).commit();
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
                        if let Some(reservation) = producer.try_reserve() {
                            let mut grant = initialized(reservation);
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

    #[test]
    fn vacant_slots_require_write_before_publication() {
        let pool = SlotPool::<u32, 2>::new_uninit();
        let mut producer = pool.try_producer().unwrap();
        let mut consumer = pool.try_consumer().unwrap();

        let reservation = producer.try_reserve().unwrap();
        assert!(!reservation.is_initialized());
        reservation.cancel();
        assert!(consumer.try_claim().is_none());

        let vacant = producer
            .try_reserve()
            .unwrap()
            .into_initialized()
            .expect_err("first slot must be vacant");
        vacant.write(41).commit();
        assert_eq!(*consumer.try_claim().unwrap(), 41);
    }

    #[test]
    fn initialized_cancellation_residue_is_reused() {
        let pool = SlotPool::<u32, 1>::new_uninit();
        let mut producer = pool.try_producer().unwrap();
        let mut consumer = pool.try_consumer().unwrap();

        let vacant = producer
            .try_reserve()
            .unwrap()
            .into_initialized()
            .expect_err("first slot must be vacant");
        vacant.write(17).cancel();
        assert!(consumer.try_claim().is_none());

        let mut grant = initialized(producer.try_reserve().unwrap());
        assert_eq!(*grant, 17);
        *grant = 23;
        grant.commit();
        assert_eq!(*consumer.try_claim().unwrap(), 23);
    }

    #[test]
    fn partially_initialized_pool_drops_only_constructed_values() {
        struct DropSpy<'a>(&'a Cell<usize>);
        impl Drop for DropSpy<'_> {
            fn drop(&mut self) {
                self.0.set(self.0.get() + 1);
            }
        }

        let drops = Cell::new(0);
        {
            let pool = SlotPool::<DropSpy<'_>, 2>::new_uninit();
            let mut producer = pool.try_producer().unwrap();
            let vacant = producer
                .try_reserve()
                .unwrap()
                .into_initialized()
                .expect_err("first slot must be vacant");
            vacant.write(DropSpy(&drops)).cancel();
        }
        assert_eq!(drops.get(), 1);
    }

    #[test]
    fn representative_block_handoff_fills_and_claims_the_same_storage() {
        #[derive(Debug)]
        struct Block<const SAMPLES: usize> {
            first_sequence: u32,
            len: usize,
            samples: [u16; SAMPLES],
        }

        let pool = SlotPool::new([Block {
            first_sequence: 0,
            len: 0,
            samples: [0; 8],
        }]);
        let mut producer = pool.try_producer().unwrap();
        let mut consumer = pool.try_consumer().unwrap();

        let mut grant = initialized(producer.try_reserve().unwrap());
        let producer_address = (&*grant) as *const Block<8>;
        grant.first_sequence = 100;
        for (offset, sample) in grant.samples.iter_mut().enumerate() {
            *sample = (offset as u16) + 1;
        }
        grant.len = grant.samples.len();
        grant.commit();

        let claim = consumer.try_claim().unwrap();
        let consumer_address = (&*claim) as *const Block<8>;
        assert_eq!(producer_address, consumer_address);
        assert_eq!(claim.first_sequence, 100);
        assert_eq!(claim.len, 8);
        assert_eq!(claim.samples, [1, 2, 3, 4, 5, 6, 7, 8]);
    }
}
