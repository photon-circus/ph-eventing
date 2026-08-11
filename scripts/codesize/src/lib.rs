//! Code-size probe. Each `#[unsafe(no_mangle)] extern "C"` function below is
//! measured as its own `.text.<name>` section, so the numbers attribute to one
//! API shape rather than to the whole binary.
//!
//! Run via `scripts/codesize.sh`, not directly.
#![no_std]

use core::panic::PanicInfo;
use ph_eventing::event_flags::{Consumer as FlagsConsumer, Producer as FlagsProducer};
use ph_eventing::{EventBuf, EventFlags, EventMask};

#[panic_handler]
fn panic(_: &PanicInfo) -> ! {
    loop {}
}

/// The claim under test: a const `new` puts the buffer in `.bss`, so it costs
/// no flash and no startup code. Measured as `.bss.*BUF`.
static BUF: EventBuf<u32, 64> = EventBuf::new();

/// EventFlags is three atomic words: pending plus the two role claims.
static FLAGS: EventFlags = EventFlags::new();

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
pub unsafe extern "C" fn event_flags_raise(
    producer: *const FlagsProducer<'static>,
    bits: u32,
) {
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
