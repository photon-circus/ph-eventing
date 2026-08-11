//! Code-size probe. Each `#[unsafe(no_mangle)] extern "C"` function below is
//! measured as its own `.text.<name>` section, so the numbers attribute to one
//! API shape rather than to the whole binary.
//!
//! Run via `scripts/codesize.sh`, not directly.
#![no_std]

use core::panic::PanicInfo;
use ph_eventing::EventBuf;
#[cfg(feature = "slot-pool")]
use ph_eventing::slot_pool::{Consumer as SlotConsumer, Producer as SlotProducer, Reservation};

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

/// Reserve and cancel one available initialized SlotPool slot.
#[cfg(feature = "slot-pool")]
#[unsafe(no_mangle)]
pub fn slot_pool_reserve_cancel(producer: &mut SlotProducer<'_, u32, 1>) -> i32 {
    match producer.try_reserve() {
        Some(Reservation::Initialized(grant)) => {
            grant.cancel();
            1
        }
        Some(Reservation::Vacant(vacant)) => {
            vacant.cancel();
            2
        }
        None => 0,
    }
}

/// Reserve, mutate, and commit one initialized SlotPool slot.
#[cfg(feature = "slot-pool")]
#[unsafe(no_mangle)]
pub fn slot_pool_reserve_commit(producer: &mut SlotProducer<'_, u32, 1>, value: u32) -> bool {
    let Some(reservation) = producer.try_reserve() else {
        return false;
    };
    let Ok(mut grant) = reservation.into_initialized() else {
        return false;
    };
    *grant = value;
    grant.commit();
    true
}

/// Reserve a vacant SlotPool slot, construct its first value, and commit it.
#[cfg(feature = "slot-pool")]
#[unsafe(no_mangle)]
pub fn slot_pool_initialize_commit(producer: &mut SlotProducer<'_, u32, 1>, value: u32) -> bool {
    let Some(reservation) = producer.try_reserve() else {
        return false;
    };
    let Err(vacant) = reservation.into_initialized() else {
        return false;
    };
    vacant.write(value).commit();
    true
}

/// Claim and release one published SlotPool slot.
#[cfg(feature = "slot-pool")]
#[unsafe(no_mangle)]
pub fn slot_pool_claim_release(consumer: &mut SlotConsumer<'_, u32, 1>) -> bool {
    let Some(claim) = consumer.try_claim() else {
        return false;
    };
    core::hint::black_box(*claim);
    claim.release();
    true
}
