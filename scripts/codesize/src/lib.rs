//! Code-size probe. Each `#[unsafe(no_mangle)] extern "C"` function below is
//! measured as its own `.text.<name>` section, so the numbers attribute to one
//! API shape rather than to the whole binary.
//!
//! Run via `scripts/codesize.sh`, not directly.
#![no_std]

use core::panic::PanicInfo;
use ph_eventing::EventBuf;
#[cfg(feature = "block-matrix")]
use ph_eventing::{Block, BlockBuilder, event_buf::Producer};

#[panic_handler]
fn panic(_: &PanicInfo) -> ! {
    loop {}
}

/// The claim under test: a const `new` puts the buffer in `.bss`, so it costs
/// no flash and no startup code. Measured as `.bss.*BUF`.
static BUF: EventBuf<u32, 64> = EventBuf::new();

#[cfg(feature = "split")]
static SPLIT_BUF: EventBuf<u32, 64> = EventBuf::new();

/// Bring-up via two independent fallible calls.
#[unsafe(no_mangle)]
pub extern "C" fn bringup_two_calls() -> i32 {
    let p = match BUF.try_producer() {
        Some(p) => p,
        None => return -1,
    };
    let c = match BUF.try_consumer() {
        Some(c) => c,
        None => return -2,
    };
    if p.push(1).is_err() {
        return -3;
    }
    match c.pop() {
        Some(_) => 0,
        None => -4,
    }
}

/// Bring-up via a single `try_split`. Only present on branches that have it.
#[cfg(feature = "split")]
#[unsafe(no_mangle)]
pub extern "C" fn bringup_split() -> i32 {
    let (p, c) = match SPLIT_BUF.try_split() {
        Some(x) => x,
        None => return -1,
    };
    if p.push(1).is_err() {
        return -3;
    }
    match c.pop() {
        Some(_) => 0,
        None => -4,
    }
}

// Each function below starts from a builder that the caller has already
// filled with N - 1 samples. Its section therefore attributes only the final
// completion and EventBuf publication shape, not the O(N) acquisition loop.
// Accepted and rejected publication share this code; the cycle probe measures
// both runtime paths separately.
#[cfg(feature = "block-matrix")]
macro_rules! block_publish_probe {
    ($name:ident, $sample:ty, $n:literal) => {
        #[unsafe(no_mangle)]
        pub fn $name(
            fill: &mut BlockBuilder<$sample, $n>,
            tx: &Producer<'_, Block<$sample, $n>, 1>,
            sequence: u32,
            sample: $sample,
        ) -> bool {
            match fill.push(sequence, sample) {
                Ok(Some(block)) => tx.push(block).is_ok(),
                Ok(None) | Err(_) => false,
            }
        }
    };
}

#[cfg(feature = "block-matrix")]
block_publish_probe!(block_w2_n8_publish, u16, 8);
#[cfg(feature = "block-matrix")]
block_publish_probe!(block_w2_n32_publish, u16, 32);
#[cfg(feature = "block-matrix")]
block_publish_probe!(block_w2_n128_publish, u16, 128);
#[cfg(feature = "block-matrix")]
block_publish_probe!(block_w8_n8_publish, u64, 8);
#[cfg(feature = "block-matrix")]
block_publish_probe!(block_w8_n32_publish, u64, 32);
#[cfg(feature = "block-matrix")]
block_publish_probe!(block_w8_n128_publish, u64, 128);
#[cfg(feature = "block-matrix")]
block_publish_probe!(block_w16_n8_publish, [u64; 2], 8);
#[cfg(feature = "block-matrix")]
block_publish_probe!(block_w16_n32_publish, [u64; 2], 32);
#[cfg(feature = "block-matrix")]
block_publish_probe!(block_w16_n128_publish, [u64; 2], 128);
