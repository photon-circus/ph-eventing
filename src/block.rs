//! Complete, contiguous sample blocks and a fill-side builder.
//!
//! `Block` is deliberately a payload, not a queue. Compose it with the
//! transport whose overload policy matches the application:
//!
//! - `EventBuf<Block<T, N>, Q>` queues up to `Q` complete blocks and rejects
//!   the newest block when full;
//! - `LatestBuf<Block<T, N>>` retains only the latest complete
//!   block (decision D3 composition).
//!
//! Publication cannot expose a partial block because [`BlockBuilder`] only
//! yields a [`Block`] after all `N` samples have been written. Dropping or
//! clearing a partially filled builder publishes nothing.
//!
//! Timestamps are payload policy: use a timestamped sample type for `T` when
//! each sample needs a stamp. The transport does not impose one.
//!
//! # Known limitation: sequence-span aliasing
//!
//! The contiguity check compares `u32` sequence values and nothing else, and
//! the successor skips reserved `0`, so sequence identity is modular over the
//! `2^32 - 1` nonzero span — the same counter-width boundary the transports
//! disclose. A partial builder held while upstream omits *exactly one whole
//! span* of sequences (or any whole multiple) sees the recurring value as the
//! expected successor and completes the block as "contiguous" despite
//! ~4.29 billion omitted samples; the gap-rejection promise (F2) is exact
//! only below one span. Reachability: the omission must span `2^32 - 1`
//! sequences while the same partial builder stays live — ~71.6 minutes of
//! outage at a sustained 1 MHz sample rate, ~5 days at 10 kHz. The chosen
//! policy is to keep the sequence one word and disclose the bound rather than
//! carry a wider epoch: recovery from outages is the application's job —
//! `clear()` the builder when your staleness watchdog or link-layer detects a
//! gap it cannot bound below one span.
//!
//! # Costs and integration (measured; bound by decisions D3 and P)
//!
//! **RAM is multiple complete blocks, always.** The latest composition
//! (`LatestBuf<Block<T, N>>`) holds three block slots plus this private
//! builder — 136–8,280 bytes of combined channel + builder RAM across the
//! measured 2/8/16-byte × `N = 8/32/128` grid. The queued composition
//! stores `Q` blocks plus the builder. The cost is fixed in size but lives
//! wherever you place the value: a `const`-constructed `static` lands in
//! `.bss` (no flash image, no startup copy); a local consumes **stack**,
//! and at up to 8,280 bytes per combined shape that is a real stack
//! budget, not a rounding error. It is not small either way: state the
//! number for your shape and charge it to the right budget.
//!
//! **Small windows can invert the economics.** Continuous block release
//! beats `N` individual sample publications for every measured 2-byte row
//! and for 8/16-byte samples at `N >= 32`, but costs 54% / 31% *more* at
//! the 8/16-byte `N = 8` corners. For tiny windows, per-sample
//! publication through a plain channel may be the cheaper shape.
//!
//! **Publication cost scales with block bytes** — 150–8,651 reference
//! instructions across the measured grid — and rejection is within 2–25
//! instructions of acceptance, because the complete rejected block is
//! preserved and returned rather than reduced to a scalar error. Budget
//! rejection like acceptance, not like error plumbing.
//!
//! **DMA integrations: the double copy is currently unavoidable in ISR
//! context.** A DMA engine has already written the samples once, and this
//! builder's storage is deliberately private — the public API offers no
//! address or writable slice a DMA controller could target — so filling
//! the builder from the DMA buffer crosses the payload a second time.
//! Either budget both copies against the accepted row for your shape, or
//! publish from task context where the copy is off the interrupt path. A
//! direct-to-granted-slot fill API is exactly the registered reopening
//! condition of cycle decision S (the deferred SlotPool foundation) — a
//! real adopter with this requirement reopens that lane rather than
//! prying the builder open. (DMA cache maintenance remains outside this
//! crate, per the taxonomy's out-of-scope list.)
//!
//! **No partial block is ever visible** — sample-level freshness inside a
//! filling window is unobtainable by design, stated here so it is chosen,
//! not discovered.
//!
//! The measured rows behind these numbers live in
//! `docs/proposals/block-buf-measurements.md` and the joint composition
//! matrix; the decision record is `docs/records/block-buf.md`.
//!
//! # Example
//! ```
//! use ph_eventing::{BlockBuilder, EventBuf};
//!
//! let mut fill = BlockBuilder::<i16, 4>::new();
//! for (sequence, sample) in [(10, 1), (11, 2), (12, 3)] {
//!     assert!(fill.push(sequence, sample).expect("contiguous").is_none());
//! }
//! let block = fill.push(13, 4).expect("contiguous").expect("complete");
//!
//! let queue = EventBuf::<_, 2>::new();
//! let producer = queue.try_producer().expect("producer");
//! let consumer = queue.try_consumer().expect("consumer");
//! // Backpressure is returned, never unwrapped: a full queue hands the
//! // complete block back through `Err` for the caller's policy.
//! assert!(producer.push(block).is_ok());
//! assert_eq!(consumer.pop().expect("one block queued").samples(), &[1, 2, 3, 4]);
//! ```
//!
//! A zero-sized block is rejected at compile time:
//!
//! ```compile_fail,E0080
//! use ph_eventing::BlockBuilder;
//! const BAD: BlockBuilder<u8, 0> = BlockBuilder::new();
//! # let _ = BAD;
//! ```

use core::mem::MaybeUninit;

/// A complete, contiguous block of `N` samples.
///
/// Sequence `0` is reserved. The first and last sequence values describe the
/// inclusive range represented by `samples`; wrap skips the reserved value.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[must_use = "a completed block is the publishable window; dropping it discards N samples"]
pub struct Block<T: Copy, const N: usize> {
    first_sequence: u32,
    last_sequence: u32,
    samples: [T; N],
}

impl<T: Copy, const N: usize> Block<T, N> {
    /// Sequence of the first sample in the block.
    #[must_use]
    pub const fn first_sequence(&self) -> u32 {
        self.first_sequence
    }

    /// Sequence of the last sample in the block.
    #[must_use]
    pub const fn last_sequence(&self) -> u32 {
        self.last_sequence
    }

    /// The complete contiguous sample array.
    #[must_use]
    pub const fn samples(&self) -> &[T; N] {
        &self.samples
    }

    /// Consume the block and return its sample array.
    #[must_use]
    pub fn into_samples(self) -> [T; N] {
        self.samples
    }
}

/// Why a sample could not be appended to a [`BlockBuilder`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[must_use = "the rejected sample rides in this error; dropping it unseen loses the sample"]
pub enum FillError<T: Copy> {
    /// Sequence `0` is reserved and never identifies a sample.
    ReservedSequence {
        /// The rejected sample.
        sample: T,
    },
    /// The sequence was not the next contiguous value.
    Discontinuous {
        /// The required next sequence.
        expected: u32,
        /// The sequence supplied by the caller.
        received: u32,
        /// The rejected sample.
        sample: T,
    },
}

/// Privately fills one block and publishes it to the caller only when complete.
///
/// A discontinuous sample is rejected without changing the partial block. The
/// caller can preserve it, or call [`clear`](Self::clear) and retry the returned
/// sample as the start of a new window. This keeps loss policy explicit.
pub struct BlockBuilder<T: Copy, const N: usize> {
    samples: [MaybeUninit<T>; N],
    len: usize,
    first_sequence: u32,
    last_sequence: u32,
}

impl<T: Copy, const N: usize> BlockBuilder<T, N> {
    /// Create an empty builder.
    ///
    /// `N == 0` is rejected at compile time.
    #[must_use]
    pub const fn new() -> Self {
        const { assert!(N > 0, "BlockBuilder capacity must be greater than zero") };
        Self {
            samples: [const { MaybeUninit::uninit() }; N],
            len: 0,
            first_sequence: 0,
            last_sequence: 0,
        }
    }

    /// Append one sequenced sample.
    ///
    /// Returns `Ok(None)` while the block is partial and `Ok(Some(block))`
    /// exactly when the `N`th sample completes it. Completion also resets the
    /// builder, ready for the next block.
    pub fn push(&mut self, sequence: u32, sample: T) -> Result<Option<Block<T, N>>, FillError<T>> {
        if sequence == 0 {
            return Err(FillError::ReservedSequence { sample });
        }

        if self.len != 0 {
            let expected = next_sequence(self.last_sequence);
            if sequence != expected {
                return Err(FillError::Discontinuous {
                    expected,
                    received: sequence,
                    sample,
                });
            }
        } else {
            self.first_sequence = sequence;
        }

        self.samples[self.len].write(sample);
        self.len += 1;
        self.last_sequence = sequence;

        if self.len != N {
            return Ok(None);
        }

        // SAFETY: `len == N`, and `len` advances only after the corresponding
        // slot is written. `T: Copy`, so copying the initialized array out does
        // not invalidate the backing `MaybeUninit` storage.
        let samples = unsafe { self.samples.as_ptr().cast::<[T; N]>().read() };
        let block = Block {
            first_sequence: self.first_sequence,
            last_sequence: self.last_sequence,
            samples,
        };
        self.clear();
        Ok(Some(block))
    }

    /// Discard the partial block, if any.
    pub fn clear(&mut self) {
        self.len = 0;
        self.first_sequence = 0;
        self.last_sequence = 0;
    }

    /// Number of samples currently held in the private partial block.
    #[must_use]
    pub const fn len(&self) -> usize {
        self.len
    }

    /// Whether no partial block is being filled.
    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// Number of samples required for a complete block.
    #[must_use]
    pub const fn capacity(&self) -> usize {
        N
    }

    /// Sequence required by the next `push`, or `None` when any non-zero
    /// sequence may start a new block.
    #[must_use]
    pub const fn expected_sequence(&self) -> Option<u32> {
        if self.len == 0 {
            None
        } else {
            Some(next_sequence(self.last_sequence))
        }
    }
}

impl<T: Copy, const N: usize> Default for BlockBuilder<T, N> {
    fn default() -> Self {
        Self::new()
    }
}

impl<T: Copy, const N: usize> core::fmt::Debug for BlockBuilder<T, N> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("BlockBuilder")
            .field("len", &self.len)
            .field("capacity", &N)
            .field("first_sequence", &self.first_sequence)
            .field("last_sequence", &self.last_sequence)
            .finish_non_exhaustive()
    }
}

const fn next_sequence(sequence: u32) -> u32 {
    if sequence == u32::MAX {
        1
    } else {
        sequence + 1
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::EventBuf;

    #[test]
    fn completes_only_after_n_contiguous_samples() {
        let mut fill = BlockBuilder::<u16, 3>::new();
        assert_eq!(fill.push(41, 4), Ok(None));
        assert_eq!(fill.push(42, 5), Ok(None));
        let block = fill.push(43, 6).unwrap().unwrap();
        assert_eq!(block.first_sequence(), 41);
        assert_eq!(block.last_sequence(), 43);
        assert_eq!(block.samples(), &[4, 5, 6]);
        assert!(fill.is_empty());
    }

    #[test]
    fn discontinuity_check_is_modular_over_the_span() {
        // The chosen policy's pin (F2 span non-promise): contiguity compares
        // `u32` sequence identity and nothing else. The recurring value after
        // exactly one whole `2^32 - 1` span is therefore indistinguishable
        // from the true successor and is accepted — including across the
        // reserved-zero wrap. If this test's expectation ever changes, the
        // policy changed (an epoch was added) and the module-doc disclosure,
        // record row, and proposal F2 text must change with it.
        let mut fill = BlockBuilder::<u16, 2>::new();
        assert_eq!(fill.push(u32::MAX, 1), Ok(None));
        // Successor skips reserved 0: expected is 1, and any occurrence of
        // sequence 1 — the immediate successor or the wrap-aliased ordinal a
        // whole span later — completes the block as contiguous.
        assert!(matches!(
            fill.push(2, 9),
            Err(FillError::Discontinuous {
                expected: 1,
                received: 2,
                ..
            })
        ));
        let block = fill.push(1, 2).unwrap().unwrap();
        assert_eq!(block.samples(), &[1, 2]);
    }

    #[test]
    fn completion_resets_for_the_next_block() {
        let mut fill = BlockBuilder::<u8, 1>::new();
        assert_eq!(fill.push(7, 9).unwrap().unwrap().into_samples(), [9]);
        assert_eq!(fill.push(8, 10).unwrap().unwrap().into_samples(), [10]);
    }

    #[test]
    fn rejects_reserved_zero_without_changing_partial_block() {
        let mut fill = BlockBuilder::<u8, 2>::new();
        assert_eq!(fill.push(9, 1), Ok(None));
        assert_eq!(
            fill.push(0, 2),
            Err(FillError::ReservedSequence { sample: 2 })
        );
        assert_eq!(fill.len(), 1);
        assert_eq!(fill.expected_sequence(), Some(10));
    }

    #[test]
    fn rejects_gap_without_hiding_loss_policy() {
        let mut fill = BlockBuilder::<u8, 2>::new();
        assert_eq!(fill.push(9, 1), Ok(None));
        assert_eq!(
            fill.push(11, 2),
            Err(FillError::Discontinuous {
                expected: 10,
                received: 11,
                sample: 2
            })
        );
        assert_eq!(fill.len(), 1);
    }

    #[test]
    fn clear_discards_a_partial_block() {
        let mut fill = BlockBuilder::<u8, 4>::new();
        assert_eq!(fill.push(1, 1), Ok(None));
        fill.clear();
        assert!(fill.is_empty());
        assert_eq!(fill.expected_sequence(), None);
        assert_eq!(fill.push(20, 2), Ok(None));
    }

    #[test]
    fn sequence_wrap_skips_zero() {
        let mut fill = BlockBuilder::<u8, 2>::new();
        assert_eq!(fill.push(u32::MAX, 1), Ok(None));
        let block = fill.push(1, 2).unwrap().unwrap();
        assert_eq!(block.first_sequence(), u32::MAX);
        assert_eq!(block.last_sequence(), 1);
    }

    #[test]
    fn works_without_default_bound() {
        #[derive(Clone, Copy, Debug, PartialEq, Eq)]
        struct NoDefault(u8);

        let mut fill = BlockBuilder::<NoDefault, 1>::new();
        let block = fill.push(1, NoDefault(3)).unwrap().unwrap();
        assert_eq!(block.samples(), &[NoDefault(3)]);
    }

    #[test]
    fn default_and_capacity_match_new() {
        let fill = BlockBuilder::<u8, 8>::default();
        assert_eq!(fill.capacity(), 8);
        assert_eq!(fill.len(), 0);
    }

    #[test]
    fn event_buf_composition_queues_and_returns_a_rejected_block() {
        let mut fill = BlockBuilder::<u8, 2>::new();
        assert_eq!(fill.push(1, 10), Ok(None));
        let first = fill.push(2, 11).unwrap().unwrap();
        assert_eq!(fill.push(3, 12), Ok(None));
        let second = fill.push(4, 13).unwrap().unwrap();

        let queue = EventBuf::<Block<u8, 2>, 1>::new();
        let producer = queue.try_producer().unwrap();
        let consumer = queue.try_consumer().unwrap();
        assert_eq!(producer.push(first), Ok(()));
        assert_eq!(producer.push(second), Err(second));
        assert_eq!(consumer.pop(), Some(first));
    }
}
