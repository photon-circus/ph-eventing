//! Code-size probe. Each `#[unsafe(no_mangle)] extern "C"` function below is
//! measured as its own `.text.<name>` section, so the numbers attribute to one
//! API shape rather than to the whole binary.
//!
//! Run via `scripts/codesize.sh`, not directly.
#![no_std]

#[cfg(feature = "latest-block-matrix")]
use core::mem::MaybeUninit;
use core::panic::PanicInfo;
use ph_eventing::counted_signal::{Consumer as CountConsumer, Producer as CountProducer};
use ph_eventing::event_flags::{Consumer as FlagsConsumer, Producer as FlagsProducer};
#[cfg(feature = "block-matrix")]
use ph_eventing::{Block, BlockBuilder, event_buf::Producer};
use ph_eventing::{EventBuf, EventFlags, EventMask, SeqRing};
#[cfg(any(feature = "latest-matrix", feature = "latest-block-matrix"))]
use ph_eventing::{
    LatestBuf, LatestItem, PublishReport,
    latest_buf::{Consumer as LatestConsumer, Producer as LatestProducer},
};

#[cfg(feature = "latest-block-matrix")]
#[path = "../../probes/block_shape.rs"]
pub mod block_probe;
#[cfg(feature = "latest-block-matrix")]
use block_probe::{BlockBuilderShape, BlockShape};

#[cfg(all(feature = "latest-matrix", feature = "latest-block-matrix"))]
compile_error!("latest-matrix and latest-block-matrix are mutually exclusive probes");

#[panic_handler]
fn panic(_: &PanicInfo) -> ! {
    loop {}
}

/// The claim under test: a const `new` puts the buffer in `.bss`, so it costs
/// no flash and no startup code. Measured as `.bss.*BUF`.
static BUF: EventBuf<u32, 64> = EventBuf::new();

/// EventFlags is one AtomicU32 plus two packed AtomicBool role claims (8 B).
static FLAGS: EventFlags = EventFlags::new();

/// SeqRing's `.bss` placement, measured directly instead of inferred from the
/// EventBuf static (the 0.3.0 review flagged that inference): its layout adds
/// `N` per-slot sequence atomics over the payload array. `no_mangle` gives the
/// section the stable name `.bss.SEQ_BUF` the runner extracts.
#[unsafe(no_mangle)]
pub static SEQ_BUF: SeqRing<u32, 64> = SeqRing::new();

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

/// Acquire both EventFlags roles without including either hot-path operation.
#[unsafe(no_mangle)]
pub extern "C" fn event_flags_acquire_roles() -> i32 {
    let producer = match FLAGS.try_producer() {
        Some(producer) => producer,
        None => return -1,
    };
    let consumer = match FLAGS.try_consumer() {
        Some(consumer) => consumer,
        None => return -2,
    };

    // The probe is never executed; forgetting keeps Drop's role-release stores
    // out of the acquisition section so the row attributes only acquisition.
    core::mem::forget(producer);
    core::mem::forget(consumer);
    0
}

/// Raise through an already-acquired EventFlags producer.
///
/// # Safety
/// `producer` must point to a live producer handle for the duration of the call.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn event_flags_raise(producer: *const FlagsProducer<'static>, bits: u32) {
    // SAFETY: required by this function's contract.
    unsafe { &*producer }.raise(EventMask::from_bits(bits));
}

/// Take through an already-acquired EventFlags consumer.
///
/// This one branch-free function is the empty and non-empty take code-size row.
///
/// # Safety
/// `consumer` must point to a live consumer handle for the duration of the call.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn event_flags_take(consumer: *const FlagsConsumer<'static>) -> u32 {
    // SAFETY: required by this function's contract.
    unsafe { &*consumer }.take_all().bits()
}

/// ISR-side CountedSignal operation, isolated from bring-up and take costs.
#[unsafe(no_mangle)]
pub fn counted_increment(producer: &CountProducer<'_>) {
    producer.increment();
}

/// Consumer-side CountedSignal take, isolated from bring-up costs.
#[unsafe(no_mangle)]
pub fn counted_take(consumer: &CountConsumer<'_>) -> u32 {
    consumer.take_count().count()
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

#[cfg(feature = "latest-block-matrix")]
macro_rules! latest_composition_static {
    ($name:ident, $payload:ty) => {
        #[unsafe(no_mangle)]
        pub static $name: LatestBuf<$payload> = LatestBuf::new();
    };
}

#[cfg(feature = "latest-block-matrix")]
macro_rules! latest_builder_static {
    ($name:ident, $payload:ty) => {
        #[unsafe(no_mangle)]
        pub static $name: MaybeUninit<$payload> = MaybeUninit::uninit();
    };
}

#[cfg(feature = "latest-block-matrix")]
latest_composition_static!(LATEST_SAMPLE_W2_BUF, u16);
#[cfg(feature = "latest-block-matrix")]
latest_composition_static!(LATEST_SAMPLE_W8_BUF, u64);
#[cfg(feature = "latest-block-matrix")]
latest_composition_static!(LATEST_SAMPLE_W16_BUF, [u64; 2]);
#[cfg(feature = "latest-block-matrix")]
latest_composition_static!(LATEST_BLOCK_W2_N8_BUF, BlockShape<u16, 8>);
#[cfg(feature = "latest-block-matrix")]
latest_composition_static!(LATEST_BLOCK_W2_N32_BUF, BlockShape<u16, 32>);
#[cfg(feature = "latest-block-matrix")]
latest_composition_static!(LATEST_BLOCK_W2_N128_BUF, BlockShape<u16, 128>);
#[cfg(feature = "latest-block-matrix")]
latest_composition_static!(LATEST_BLOCK_W8_N8_BUF, BlockShape<u64, 8>);
#[cfg(feature = "latest-block-matrix")]
latest_composition_static!(LATEST_BLOCK_W8_N32_BUF, BlockShape<u64, 32>);
#[cfg(feature = "latest-block-matrix")]
latest_composition_static!(LATEST_BLOCK_W8_N128_BUF, BlockShape<u64, 128>);
#[cfg(feature = "latest-block-matrix")]
latest_composition_static!(LATEST_BLOCK_W16_N8_BUF, BlockShape<[u64; 2], 8>);
#[cfg(feature = "latest-block-matrix")]
latest_composition_static!(LATEST_BLOCK_W16_N32_BUF, BlockShape<[u64; 2], 32>);
#[cfg(feature = "latest-block-matrix")]
latest_composition_static!(LATEST_BLOCK_W16_N128_BUF, BlockShape<[u64; 2], 128>);
#[cfg(feature = "latest-block-matrix")]
latest_builder_static!(LATEST_BUILDER_W2_N8, BlockBuilderShape<u16, 8>);
#[cfg(feature = "latest-block-matrix")]
latest_builder_static!(LATEST_BUILDER_W2_N32, BlockBuilderShape<u16, 32>);
#[cfg(feature = "latest-block-matrix")]
latest_builder_static!(LATEST_BUILDER_W2_N128, BlockBuilderShape<u16, 128>);
#[cfg(feature = "latest-block-matrix")]
latest_builder_static!(LATEST_BUILDER_W8_N8, BlockBuilderShape<u64, 8>);
#[cfg(feature = "latest-block-matrix")]
latest_builder_static!(LATEST_BUILDER_W8_N32, BlockBuilderShape<u64, 32>);
#[cfg(feature = "latest-block-matrix")]
latest_builder_static!(LATEST_BUILDER_W8_N128, BlockBuilderShape<u64, 128>);
#[cfg(feature = "latest-block-matrix")]
latest_builder_static!(LATEST_BUILDER_W16_N8, BlockBuilderShape<[u64; 2], 8>);
#[cfg(feature = "latest-block-matrix")]
latest_builder_static!(LATEST_BUILDER_W16_N32, BlockBuilderShape<[u64; 2], 32>);
#[cfg(feature = "latest-block-matrix")]
latest_builder_static!(LATEST_BUILDER_W16_N128, BlockBuilderShape<[u64; 2], 128>);

// Code size does not depend on whether the runtime path is first/replacement
// publication or empty/pending take, so each payload needs one publish and one
// take monomorph. The cycle probe separates those runtime paths.
#[cfg(any(feature = "latest-matrix", feature = "latest-block-matrix"))]
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

#[cfg(feature = "latest-block-matrix")]
latest_operation_probe!(latest_sample_w2_publish, latest_sample_w2_take, u16);
#[cfg(feature = "latest-block-matrix")]
latest_operation_probe!(latest_sample_w8_publish, latest_sample_w8_take, u64);
#[cfg(feature = "latest-block-matrix")]
latest_operation_probe!(latest_sample_w16_publish, latest_sample_w16_take, [u64; 2]);
#[cfg(feature = "latest-block-matrix")]
macro_rules! latest_take_probe {
    ($take:ident, $payload:ty) => {
        #[unsafe(no_mangle)]
        pub fn $take(consumer: &LatestConsumer<'_, $payload>) -> Option<LatestItem<$payload>> {
            consumer.take_latest()
        }
    };
}

#[cfg(feature = "latest-block-matrix")]
latest_take_probe!(latest_block_w2_n8_take, BlockShape<u16, 8>);
#[cfg(feature = "latest-block-matrix")]
latest_take_probe!(latest_block_w2_n32_take, BlockShape<u16, 32>);
#[cfg(feature = "latest-block-matrix")]
latest_take_probe!(latest_block_w2_n128_take, BlockShape<u16, 128>);
#[cfg(feature = "latest-block-matrix")]
latest_take_probe!(latest_block_w8_n8_take, BlockShape<u64, 8>);
#[cfg(feature = "latest-block-matrix")]
latest_take_probe!(latest_block_w8_n32_take, BlockShape<u64, 32>);
#[cfg(feature = "latest-block-matrix")]
latest_take_probe!(latest_block_w8_n128_take, BlockShape<u64, 128>);
#[cfg(feature = "latest-block-matrix")]
latest_take_probe!(latest_block_w16_n8_take, BlockShape<[u64; 2], 8>);
#[cfg(feature = "latest-block-matrix")]
latest_take_probe!(latest_block_w16_n32_take, BlockShape<[u64; 2], 32>);
#[cfg(feature = "latest-block-matrix")]
latest_take_probe!(latest_block_w16_n128_take, BlockShape<[u64; 2], 128>);

#[cfg(feature = "latest-block-matrix")]
macro_rules! latest_complete_probe {
    ($name:ident, $sample:ty, $n:literal) => {
        #[unsafe(no_mangle)]
        pub fn $name(
            fill: &mut BlockBuilderShape<$sample, $n>,
            producer: &LatestProducer<'_, BlockShape<$sample, $n>>,
            sequence: u32,
            sample: $sample,
        ) -> bool {
            match fill.push(sequence, sample) {
                Ok(Some(block)) => producer.publish(block).replaced_unread,
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
#[cfg(feature = "latest-block-matrix")]
latest_complete_probe!(latest_complete_w2_n8_publish, u16, 8);
#[cfg(feature = "latest-block-matrix")]
latest_complete_probe!(latest_complete_w2_n32_publish, u16, 32);
#[cfg(feature = "latest-block-matrix")]
latest_complete_probe!(latest_complete_w2_n128_publish, u16, 128);
#[cfg(feature = "latest-block-matrix")]
latest_complete_probe!(latest_complete_w8_n8_publish, u64, 8);
#[cfg(feature = "latest-block-matrix")]
latest_complete_probe!(latest_complete_w8_n32_publish, u64, 32);
#[cfg(feature = "latest-block-matrix")]
latest_complete_probe!(latest_complete_w8_n128_publish, u64, 128);
#[cfg(feature = "latest-block-matrix")]
latest_complete_probe!(latest_complete_w16_n8_publish, [u64; 2], 8);
#[cfg(feature = "latest-block-matrix")]
latest_complete_probe!(latest_complete_w16_n32_publish, [u64; 2], 32);
#[cfg(feature = "latest-block-matrix")]
latest_complete_probe!(latest_complete_w16_n128_publish, [u64; 2], 128);

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
