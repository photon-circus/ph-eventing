//! Code-size probe. Each `#[unsafe(no_mangle)] extern "C"` function below is
//! measured as its own `.text.<name>` section, so the numbers attribute to one
//! API shape rather than to the whole binary.
//!
//! Run via `scripts/codesize.sh`, not directly.
#![no_std]

use core::panic::PanicInfo;
use ph_eventing::EventBuf;
#[cfg(feature = "latest-matrix")]
use ph_eventing::{
    LatestBuf, LatestItem, PublishReport,
    latest_buf::{Consumer as LatestConsumer, Producer as LatestProducer},
};

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

// These exported statics make the exact channel layouts visible as individual
// object sections. LatestBuf's private role indices are encoded so the initial
// image is all zero and can land in `.bss`; extracting the target object pins
// that no-flash/no-startup-copy property rather than assuming it from source.
#[cfg(feature = "latest-matrix")]
#[unsafe(no_mangle)]
pub static LATEST_U32_BUF: LatestBuf<u32> = LatestBuf::new();

#[cfg(feature = "latest-matrix")]
#[unsafe(no_mangle)]
pub static LATEST_W16_BUF: LatestBuf<[u32; 4]> = LatestBuf::new();

#[cfg(feature = "latest-matrix")]
#[unsafe(no_mangle)]
pub static LATEST_BLOCK_BUF: LatestBuf<[u32; 32]> = LatestBuf::new();

// Code size does not depend on whether the runtime path is first/replacement
// publication or empty/pending take, so each payload needs one publish and one
// take monomorph. The cycle probe separates those runtime paths.
#[cfg(feature = "latest-matrix")]
macro_rules! latest_operation_probe {
    ($publish:ident, $take:ident, $payload:ty) => {
        #[unsafe(no_mangle)]
        pub fn $publish(producer: &LatestProducer<'_, $payload>, value: $payload) -> PublishReport {
            producer.publish(value)
        }

        #[unsafe(no_mangle)]
        pub fn $take(consumer: &LatestConsumer<'_, $payload>) -> Option<LatestItem<$payload>> {
            consumer.take_latest()
        }
    };
}

#[cfg(feature = "latest-matrix")]
latest_operation_probe!(latest_u32_publish, latest_u32_take, u32);
#[cfg(feature = "latest-matrix")]
latest_operation_probe!(latest_w16_publish, latest_w16_take, [u32; 4]);
#[cfg(feature = "latest-matrix")]
latest_operation_probe!(latest_block_publish, latest_block_take, [u32; 32]);

// Role acquisition and release are payload-independent. Keep producer and
// consumer paths separate so a future state-persistence candidate can be
// compared without changing the runner.
#[cfg(feature = "latest-matrix")]
#[unsafe(no_mangle)]
pub fn latest_producer_reacquire(channel: &LatestBuf<u32>) -> bool {
    match channel.try_producer() {
        Some(producer) => {
            drop(producer);
            true
        }
        None => false,
    }
}

#[cfg(feature = "latest-matrix")]
#[unsafe(no_mangle)]
pub fn latest_consumer_reacquire(channel: &LatestBuf<u32>) -> bool {
    match channel.try_consumer() {
        Some(consumer) => {
            drop(consumer);
            true
        }
        None => false,
    }
}
