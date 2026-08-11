//! Freshness-first SPSC snapshot channel.
//!
//! [`LatestBuf`] retains at most one unread publication. A producer always
//! publishes the newest complete `T`; if an older unread value is displaced,
//! [`PublishReport::replaced_unread`] reports it. The consumer takes only the
//! latest value and receives its generation plus the exact number skipped.
//!
//! The prototype uses three slots with exclusive ownership: one producer
//! slot, one consumer slot, and one slot named by an atomic exchange state.
//! An endpoint accesses a slot only after acquiring it through a single
//! atomic swap. Producer and consumer therefore never touch the same payload
//! bytes concurrently, unlike a seqlock.
//!
//! Endpoint cursors live in the channel rather than the handles. Handles are
//! stateless and dropping/reacquiring one continues its slot ownership,
//! generation sequence, and skipped accounting.
//!
//! An empty consumer poll first Acquire-loads the ready bit and returns without
//! an atomic read-modify-write. A pending poll still performs the same `AcqRel`
//! swap that transfers slot ownership. The private initial role indices are
//! encoded as zero so a const-initialized channel lands in `.bss` rather than
//! carrying its three payload slots in the flash-backed `.data` image.
//!
//! # Decision status
//!
//! Decision D1 (wrap-ambiguity policy) is **closed** as documented
//! approximation plus payload escape hatch (contract §9, non-promise X6):
//! skipped counts are exact while fewer than `u32::MAX` non-zero
//! generations separate successful takes, and beyond that full wrap span
//! the `u32` result is a documented under-count —
//! [`LatestItem::skipped`] carries the full disclosure.
//!
//! Decision D2 (`Source<T>` policy) is **closed**: [`Consumer`] does not
//! implement [`crate::Source`], because `try_pop` cannot report the
//! displacement that is this channel's *designed* overload behaviour —
//! [`crate::LatestSource`] is the consumer's designed contract surface
//! (contract §9, non-promise X7), and the absent impl is pinned by a
//! `compile_fail` doctest on [`Consumer`].
//!
//! Decision D3 (first deliverable form) is **closed**: `T` stays generic
//! by decision, not default — a complete block is a payload
//! (`LatestBuf<Block<T, N>>` via the BlockBuf composition),
//! sample-versus-block is release scheduling and RAM, and no separate
//! latest-block type exists (contract §9).
//!
//! Review caveat A.3 (handle-state continuation) is **closed** the same
//! way this implementation works: role state is channel-resident in
//! role-owned storage and handles are stateless, so a drop-and-reacquire
//! continues by construction (contract H4; proposal Appendix A.3).
//! Contract non-promise X8 states the role-recovery boundary: the role is
//! held until the handle is dropped, handle lifetime is an application
//! property, and there is deliberately no out-of-band role reset.
//!
//! Every LatestBuf decision (D1–D3, A.3) is closed; the contract §9 and
//! proposal Appendix A.3 carry the records.
//!
//! # Example
//!
//! ```
//! use ph_eventing::LatestBuf;
//!
//! let channel = LatestBuf::<u32>::new();
//! let producer = channel.try_producer().expect("producer");
//! let consumer = channel.try_consumer().expect("consumer");
//!
//! assert!(!producer.publish(10).replaced_unread);
//! assert!(producer.publish(20).replaced_unread);
//! let item = consumer.take_latest().expect("latest value");
//! assert_eq!((item.value, item.generation, item.skipped), (20, 2, 1));
//! ```

use crate::sync::{AtomicBool, AtomicU32, Ordering, TrackedCell};
use core::cell::Cell;
use core::marker::PhantomData;
use core::mem::MaybeUninit;

const SLOT_MASK: u32 = 0b11;
const READY_BIT: u32 = 0b100;
// Encode the initially owned slots as zero so a const-initialized channel is
// an all-zero image and lands in `.bss`. XOR is its own inverse, and these
// role-owned fields never enter the shared exchange state.
const PRODUCER_SLOT_XOR: u32 = 1;
const CONSUMER_SLOT_XOR: u32 = 2;

#[derive(Clone, Copy)]
struct Entry<T: Copy> {
    generation: u32,
    value: T,
}

#[derive(Clone, Copy)]
struct ProducerState {
    back_encoded: u32,
    next_generation: u32,
}

#[derive(Clone, Copy)]
struct ConsumerState {
    front_encoded: u32,
    last_generation: u32,
}

#[cfg(not(loom))]
const fn slot_array<T: Copy>() -> [TrackedCell<MaybeUninit<Entry<T>>>; 3] {
    [const { TrackedCell::new(MaybeUninit::uninit()) }; 3]
}

#[cfg(loom)]
fn slot_array<T: Copy>() -> [TrackedCell<MaybeUninit<Entry<T>>>; 3] {
    core::array::from_fn(|_| TrackedCell::new(MaybeUninit::uninit()))
}

/// Result of one successful latest-value publication.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[must_use]
pub struct PublishReport {
    /// Non-zero transport generation assigned to this publication.
    pub generation: u32,
    /// Whether this publication displaced an unread older publication.
    pub replaced_unread: bool,
}

/// A value claimed from a [`LatestBuf`].
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[must_use]
pub struct LatestItem<T> {
    /// Complete value copied from the claimed publication.
    pub value: T,
    /// Non-zero transport generation assigned when the value was published.
    pub generation: u32,
    /// Publications assigned after the prior take and before this one.
    ///
    /// One formula in every case (contract C3): the wrap-aware generation
    /// distance from the previously taken value to this one, minus one,
    /// saturated at zero. Exact while fewer than `u32::MAX` non-zero
    /// generations — one full wrap span — separate two successful takes.
    /// Beyond that span the same formula under-counts: by whole spans
    /// while the endpoints differ, and a gap of exactly one or more full
    /// cycles reports zero — a silent under-count, accepted and
    /// documented by decision D1 (contract non-promise X6).
    ///
    /// The boundary is a rate × take-interval property: it is crossed
    /// exactly when a full span of publications occurs between two
    /// successful takes — whether because the consumer stopped (fault)
    /// or because the deployment deliberately takes rarely against a
    /// fast producer (design). At representative rates that is roughly
    /// 49.7 days between takes at continuous 1 kHz publishing, or
    /// 72 minutes at 1 MHz; the crate bounds neither rate nor cadence,
    /// so this arithmetic is the caller's to run. The channel
    /// deliberately does not detect the crossing; detection would price
    /// wider hot-path state into every operation and still could not
    /// recover the lost count. If the count itself is the requirement
    /// (audit, metering, loss accounting), carry a wider
    /// producer-assigned sequence inside `T`; if consumer liveness is,
    /// use a watchdog — each catches its condition sooner and correctly.
    pub skipped: u32,
}

/// Three-slot freshness-first SPSC snapshot channel.
///
/// `LatestBuf<T>` has fixed storage for three `T` values and never allocates.
/// Publishing always succeeds and takes one atomic exchange. Taking is also
/// bounded to one load, at most one exchange, and at most one payload copy.
pub struct LatestBuf<T: Copy> {
    exchange: AtomicU32,
    slots: [TrackedCell<MaybeUninit<Entry<T>>>; 3],
    producer_state: TrackedCell<ProducerState>,
    consumer_state: TrackedCell<ConsumerState>,
    producer_taken: AtomicBool,
    consumer_taken: AtomicBool,
}

// SAFETY: the handle-acquisition flags enforce one endpoint per role. Each
// payload slot belongs exclusively to the producer, consumer, or atomic
// exchange state, and AcqRel swaps transfer both ownership and visibility.
// The role-state cells are accessed only by their unique active handle; the
// Release drop / AcqRel acquisition pair orders access across reacquisition.
unsafe impl<T: Copy + Send> Sync for LatestBuf<T> {}

impl<T: Copy> LatestBuf<T> {
    /// Create an empty channel.
    ///
    /// On normal builds this is `const`, so a channel may live in a `static`.
    /// Under `--cfg loom` it is non-const because Loom's primitives are not
    /// const-constructible.
    #[cfg(not(loom))]
    pub const fn new() -> Self {
        Self {
            exchange: AtomicU32::new(0),
            slots: slot_array(),
            producer_state: TrackedCell::new(ProducerState {
                back_encoded: 0,
                next_generation: 0,
            }),
            consumer_state: TrackedCell::new(ConsumerState {
                front_encoded: 0,
                last_generation: 0,
            }),
            producer_taken: AtomicBool::new(false),
            consumer_taken: AtomicBool::new(false),
        }
    }

    /// Create an empty channel for Loom model checking.
    #[cfg(loom)]
    pub fn new() -> Self {
        Self {
            exchange: AtomicU32::new(0),
            slots: slot_array(),
            producer_state: TrackedCell::new(ProducerState {
                back_encoded: 0,
                next_generation: 0,
            }),
            consumer_state: TrackedCell::new(ConsumerState {
                front_encoded: 0,
                last_generation: 0,
            }),
            producer_taken: AtomicBool::new(false),
            consumer_taken: AtomicBool::new(false),
        }
    }

    /// Try to acquire the unique producer role.
    ///
    /// Returns `None` while another producer handle is active. Dropping the
    /// active handle makes the role available without resetting its state.
    ///
    /// The role is held until the active handle is *dropped* — a handle
    /// that is forgotten, or owned by an execution context destroyed
    /// without running destructors, leaves the role held with channel
    /// state intact. There is deliberately no out-of-band role reset: a
    /// forced release could free a role while a live handle still exists,
    /// defeating the exclusive ownership that soundness rests on
    /// (contract non-promise X8). Handle lifetime is the application's
    /// property; the channel observes only acquisition and drop.
    #[inline]
    pub fn try_producer(&self) -> Option<Producer<'_, T>> {
        if self.producer_taken.swap(true, Ordering::AcqRel) {
            None
        } else {
            Some(Producer {
                buf: self,
                _not_sync: PhantomData,
            })
        }
    }

    /// Try to acquire the unique consumer role.
    ///
    /// Returns `None` while another consumer handle is active. Dropping the
    /// active handle makes the role available without resetting its state.
    ///
    /// Role recovery follows the same boundary as [`Self::try_producer`]:
    /// held until dropped, no out-of-band reset, handle lifetime owned by
    /// the application (contract non-promise X8).
    #[inline]
    pub fn try_consumer(&self) -> Option<Consumer<'_, T>> {
        if self.consumer_taken.swap(true, Ordering::AcqRel) {
            None
        } else {
            Some(Consumer {
                buf: self,
                _not_sync: PhantomData,
            })
        }
    }

    #[inline(always)]
    const fn encode(slot: u32, ready: bool) -> u32 {
        slot | if ready { READY_BIT } else { 0 }
    }

    #[inline(always)]
    const fn slot(state: u32) -> usize {
        (state & SLOT_MASK) as usize
    }

    #[inline(always)]
    const fn ready(state: u32) -> bool {
        state & READY_BIT != 0
    }

    #[inline(always)]
    const fn producer_slot(encoded: u32) -> u32 {
        encoded ^ PRODUCER_SLOT_XOR
    }

    #[inline(always)]
    const fn encode_producer_slot(slot: u32) -> u32 {
        slot ^ PRODUCER_SLOT_XOR
    }

    #[inline(always)]
    const fn consumer_slot(encoded: u32) -> u32 {
        encoded ^ CONSUMER_SLOT_XOR
    }

    #[inline(always)]
    const fn encode_consumer_slot(slot: u32) -> u32 {
        slot ^ CONSUMER_SLOT_XOR
    }

    #[inline(always)]
    const fn next_generation(current: u32) -> u32 {
        match current.wrapping_add(1) {
            0 => 1,
            generation => generation,
        }
    }

    /// Assigned-generation distance in `(from, to]`, excluding reserved zero.
    #[inline(always)]
    const fn generation_distance(from: u32, to: u32) -> u32 {
        let raw = to.wrapping_sub(from);
        if to < from { raw - 1 } else { raw }
    }
}

impl<T: Copy> Default for LatestBuf<T> {
    fn default() -> Self {
        Self::new()
    }
}

impl<T: Copy> core::fmt::Debug for LatestBuf<T> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("LatestBuf").finish_non_exhaustive()
    }
}

/// Unique, stateless write handle for a [`LatestBuf`].
pub struct Producer<'a, T: Copy> {
    buf: &'a LatestBuf<T>,
    _not_sync: PhantomData<Cell<()>>,
}

impl<T: Copy> Producer<'_, T> {
    /// Publish a complete value as the newest channel state.
    ///
    /// This always succeeds, executes one payload write and one atomic swap,
    /// and never waits for consumer progress.
    #[inline]
    pub fn publish(&self, value: T) -> PublishReport {
        // SAFETY: producer_taken grants the unique producer handle exclusive
        // access to producer_state for its lifetime. Reacquisition is ordered
        // by the handle's Release drop and AcqRel acquisition.
        let (back, generation) = self.buf.producer_state.with_mut(|state| unsafe {
            let state = &mut *state;
            let generation = LatestBuf::<T>::next_generation(state.next_generation);
            state.next_generation = generation;
            (
                LatestBuf::<T>::producer_slot(state.back_encoded),
                generation,
            )
        });

        // SAFETY: `back` is exclusively producer-owned. The producer does not
        // relinquish it until the following atomic exchange.
        self.buf.slots[back as usize].with_mut(|slot| unsafe {
            (*slot).write(Entry { generation, value });
        });

        let previous = self
            .buf
            .exchange
            .swap(LatestBuf::<T>::encode(back, true), Ordering::AcqRel);

        // SAFETY: the exchange transferred its previous slot exclusively to
        // the producer. No other producer handle exists.
        self.buf.producer_state.with_mut(|state| unsafe {
            (*state).back_encoded = LatestBuf::<T>::encode_producer_slot(previous & SLOT_MASK);
        });

        PublishReport {
            generation,
            replaced_unread: LatestBuf::<T>::ready(previous),
        }
    }
}

impl<T: Copy> Drop for Producer<'_, T> {
    fn drop(&mut self) {
        self.buf.producer_taken.store(false, Ordering::Release);
    }
}

impl<T: Copy> core::fmt::Debug for Producer<'_, T> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("latest_buf::Producer").finish()
    }
}

/// Unique, stateless read handle for a [`LatestBuf`].
///
/// # No `Source<T>` implementation — by decision, not omission
/// `Source::try_pop` cannot report the displacement that is this channel's
/// designed overload behaviour, so a generic pipeline would silently
/// discard loss evidence during *normal* operation (decision D2; contract
/// non-promise X7). [`crate::LatestSource`] is the consumer's designed
/// contract surface. A caller whose domain genuinely permits discarding
/// the skipped count writes its own adapter, so the discard is signed in
/// application code, never by the transport. This `compile_fail` doctest
/// pins the absent impl so a convenience `Source` cannot arrive silently,
/// and pinning the error code keeps it honest:
///
/// ```compile_fail,E0277
/// use ph_eventing::Source;
/// let channel = ph_eventing::LatestBuf::<u32>::new();
/// let mut consumer = channel.try_consumer().unwrap();
/// let _ = Source::try_pop(&mut consumer);
/// ```
pub struct Consumer<'a, T: Copy> {
    buf: &'a LatestBuf<T>,
    _not_sync: PhantomData<Cell<()>>,
}

impl<T: Copy> Consumer<'_, T> {
    /// Claim and copy the latest unread publication.
    ///
    /// Returns `None` when no publication is pending. The operation performs
    /// an `Acquire` load first, so an empty poll returns without an atomic
    /// read-modify-write. A pending publication is still claimed with the
    /// ownership-transferring `AcqRel` swap.
    #[inline]
    pub fn take_latest(&self) -> Option<LatestItem<T>> {
        // A false load can linearize before a concurrent publication: the
        // publication remains pending for the next poll. This path transfers
        // no slot and therefore must not access or update either owned role.
        // A true load is only a hint; the AcqRel swap below remains the actual
        // ownership transfer and may claim a newer replacement publication.
        if !LatestBuf::<T>::ready(self.buf.exchange.load(Ordering::Acquire)) {
            return None;
        }

        // SAFETY: consumer_taken grants this handle exclusive role-state
        // access, ordered across reacquisition by Release/AcqRel.
        let front = self
            .buf
            .consumer_state
            .with(|state| unsafe { LatestBuf::<T>::consumer_slot((*state).front_encoded) });

        let previous = self
            .buf
            .exchange
            .swap(LatestBuf::<T>::encode(front, false), Ordering::AcqRel);
        let claimed = previous & SLOT_MASK;

        // SAFETY: the exchange transferred `claimed` exclusively to this
        // consumer; preserving it even on the empty path maintains the three
        // disjoint ownership roles.
        self.buf.consumer_state.with_mut(|state| unsafe {
            (*state).front_encoded = LatestBuf::<T>::encode_consumer_slot(claimed);
        });

        if !LatestBuf::<T>::ready(previous) {
            return None;
        }

        // SAFETY: the ready bit means the producer initialized this Entry
        // before publishing it. AcqRel swap acquired both ownership and the
        // initialized bytes, and no producer can reacquire the slot until a
        // later exchange relinquishes it.
        let entry = self.buf.slots[LatestBuf::<T>::slot(previous)]
            .with(|slot| unsafe { (*slot).assume_init_read() });

        // SAFETY: this unique consumer handle exclusively owns consumer_state.
        let skipped = self.buf.consumer_state.with_mut(|state| unsafe {
            let state = &mut *state;
            let distance =
                LatestBuf::<T>::generation_distance(state.last_generation, entry.generation);
            state.last_generation = entry.generation;
            distance.saturating_sub(1)
        });

        Some(LatestItem {
            value: entry.value,
            generation: entry.generation,
            skipped,
        })
    }
}

impl<T: Copy> Drop for Consumer<'_, T> {
    fn drop(&mut self) {
        self.buf.consumer_taken.store(false, Ordering::Release);
    }
}

impl<T: Copy> core::fmt::Debug for Consumer<'_, T> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("latest_buf::Consumer").finish()
    }
}

impl<T: Copy> crate::traits::LatestSink<T> for Producer<'_, T> {
    #[inline]
    fn publish_latest(&mut self, value: T) -> PublishReport {
        self.publish(value)
    }
}

impl<T: Copy> crate::traits::LatestSource<T> for Consumer<'_, T> {
    #[inline]
    fn try_take_latest(&mut self) -> Option<LatestItem<T>> {
        self.take_latest()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn starts_empty_and_takes_each_publication_at_most_once() {
        let channel = LatestBuf::<u32>::new();
        let producer = channel.try_producer().unwrap();
        let consumer = channel.try_consumer().unwrap();
        assert_eq!(consumer.take_latest(), None);
        assert_eq!(producer.publish(7).generation, 1);
        assert_eq!(
            consumer.take_latest(),
            Some(LatestItem {
                value: 7,
                generation: 1,
                skipped: 0
            })
        );
        assert_eq!(consumer.take_latest(), None);
    }

    #[test]
    fn replacement_is_reported_on_both_endpoints() {
        let channel = LatestBuf::<u32>::new();
        let producer = channel.try_producer().unwrap();
        let consumer = channel.try_consumer().unwrap();
        assert!(!producer.publish(10).replaced_unread);
        assert!(producer.publish(20).replaced_unread);
        assert!(producer.publish(30).replaced_unread);
        assert_eq!(
            consumer.take_latest(),
            Some(LatestItem {
                value: 30,
                generation: 3,
                skipped: 2
            })
        );
    }

    #[test]
    fn handle_reacquisition_continues_role_state() {
        let channel = LatestBuf::<u32>::new();
        {
            let producer = channel.try_producer().unwrap();
            assert_eq!(producer.publish(1).generation, 1);
        }
        let producer = channel.try_producer().unwrap();
        assert_eq!(
            producer.publish(2),
            PublishReport {
                generation: 2,
                replaced_unread: true
            }
        );

        {
            let consumer = channel.try_consumer().unwrap();
            assert_eq!(consumer.take_latest().unwrap().skipped, 1);
        }
        let _ = producer.publish(3);
        let _ = producer.publish(4);
        let consumer = channel.try_consumer().unwrap();
        assert_eq!(
            consumer.take_latest(),
            Some(LatestItem {
                value: 4,
                generation: 4,
                skipped: 1
            })
        );
    }

    #[test]
    fn role_acquisition_is_unique_and_handles_are_send() {
        fn assert_send<T: Send>() {}
        assert_send::<Producer<'_, u32>>();
        assert_send::<Consumer<'_, u32>>();

        let channel = LatestBuf::<u32>::new();
        let producer = channel.try_producer().unwrap();
        let consumer = channel.try_consumer().unwrap();
        assert!(channel.try_producer().is_none());
        assert!(channel.try_consumer().is_none());
        drop(producer);
        drop(consumer);
        assert!(channel.try_producer().is_some());
        assert!(channel.try_consumer().is_some());
    }

    #[cfg(not(loom))]
    #[test]
    fn static_channel_yields_static_sendable_handles() {
        static CHANNEL: LatestBuf<u32> = LatestBuf::new();
        fn producer() -> Producer<'static, u32> {
            CHANNEL.try_producer().unwrap()
        }
        fn consumer() -> Consumer<'static, u32> {
            CHANNEL.try_consumer().unwrap()
        }
        let producer = producer();
        let consumer = consumer();
        let _ = producer.publish(42);
        assert_eq!(consumer.take_latest().unwrap().value, 42);
    }

    #[test]
    fn generation_wrap_skips_zero_and_counts_gap_exactly() {
        let channel = LatestBuf::<u32>::new();
        // SAFETY: no producer handle exists while the test seeds role state.
        channel.producer_state.with_mut(|state| unsafe {
            (*state).next_generation = u32::MAX - 1;
        });
        // SAFETY: no consumer handle exists while the test seeds role state.
        channel.consumer_state.with_mut(|state| unsafe {
            (*state).last_generation = u32::MAX - 1;
        });
        let producer = channel.try_producer().unwrap();
        let consumer = channel.try_consumer().unwrap();
        assert_eq!(producer.publish(1).generation, u32::MAX);
        assert_eq!(producer.publish(2).generation, 1);
        assert_eq!(
            consumer.take_latest(),
            Some(LatestItem {
                value: 2,
                generation: 1,
                skipped: 1
            })
        );
        assert_eq!(LatestBuf::<u32>::generation_distance(u32::MAX, 1), 1);
    }

    #[test]
    fn full_generation_cycle_uses_documented_approximation() {
        // Equal endpoints are indistinguishable from no progress after a full
        // generation cycle. D1 is closed as documented approximation plus
        // payload escape hatch (contract C3/X6); this test is the closure's
        // named pin for the full-cycle modular result.
        assert_eq!(LatestBuf::<u32>::generation_distance(17, 17), 0);
    }

    #[test]
    fn generic_payload_can_be_a_complete_block() {
        let channel = LatestBuf::<[u16; 4]>::new();
        let producer = channel.try_producer().unwrap();
        let consumer = channel.try_consumer().unwrap();
        let _ = producer.publish([1, 2, 3, 4]);
        assert_eq!(consumer.take_latest().unwrap().value, [1, 2, 3, 4]);
    }

    #[test]
    fn concurrent_publication_never_returns_torn_value() {
        let channel = LatestBuf::<[u32; 4]>::new();
        let total = crate::test_support::iterations(50_000);
        std::thread::scope(|scope| {
            scope.spawn(|| {
                let producer = channel.try_producer().unwrap();
                for value in 1..=total {
                    let _ = producer.publish([value; 4]);
                }
            });
            let consumer = channel.try_consumer().unwrap();
            let mut last_generation = 0;
            while last_generation < total {
                if let Some(item) = consumer.take_latest() {
                    assert!(item.generation > last_generation);
                    assert_eq!(item.value, [item.value[0]; 4]);
                    last_generation = item.generation;
                } else {
                    std::thread::yield_now();
                }
            }
        });
    }
}
