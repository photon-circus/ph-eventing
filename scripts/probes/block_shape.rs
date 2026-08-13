//! Probe-only structural twins of `Block`/`BlockBuilder`, pinned at the
//! evaluation revision (`bc54a9a`).
//!
//! Origin: during the 0.3.0 cycle the LatestBuf and BlockBuf candidates lived
//! on separate branches that were not allowed to stack, so the D3 composition
//! probes measured against this twin instead of importing the BlockBuf
//! branch. Both types now ship from one tree, so the twin's remaining job is
//! stability: the composition rows keep measuring the exact evaluated layout.
//! Switching these probes to the real `ph_eventing::Block` types is a
//! deliberate future re-measure (new rows, new bless), not a cleanup.

#![allow(dead_code)]

use core::mem::MaybeUninit;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BlockShape<T: Copy, const N: usize> {
    first_sequence: u32,
    last_sequence: u32,
    samples: [T; N],
}

impl<T: Copy, const N: usize> BlockShape<T, N> {
    pub const fn filled(sample: T) -> Self {
        Self {
            first_sequence: 1,
            last_sequence: N as u32,
            samples: [sample; N],
        }
    }
}

pub struct BlockBuilderShape<T: Copy, const N: usize> {
    samples: [MaybeUninit<T>; N],
    len: usize,
    first_sequence: u32,
    last_sequence: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FillErrorShape<T: Copy> {
    ReservedSequence {
        sample: T,
    },
    Discontinuous {
        expected: u32,
        received: u32,
        sample: T,
    },
}

impl<T: Copy, const N: usize> BlockBuilderShape<T, N> {
    pub const fn new() -> Self {
        const { assert!(N > 0, "BlockBuilder capacity must be greater than zero") };
        Self {
            samples: [const { MaybeUninit::uninit() }; N],
            len: 0,
            first_sequence: 0,
            last_sequence: 0,
        }
    }

    pub fn push(
        &mut self,
        sequence: u32,
        sample: T,
    ) -> Result<Option<BlockShape<T, N>>, FillErrorShape<T>> {
        if sequence == 0 {
            return Err(FillErrorShape::ReservedSequence { sample });
        }

        if self.len != 0 {
            let expected = next_sequence(self.last_sequence);
            if sequence != expected {
                return Err(FillErrorShape::Discontinuous {
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
        let block = BlockShape {
            first_sequence: self.first_sequence,
            last_sequence: self.last_sequence,
            samples,
        };
        self.clear();
        Ok(Some(block))
    }

    pub fn clear(&mut self) {
        self.len = 0;
        self.first_sequence = 0;
        self.last_sequence = 0;
    }
}

const fn next_sequence(sequence: u32) -> u32 {
    if sequence == u32::MAX {
        1
    } else {
        sequence + 1
    }
}
