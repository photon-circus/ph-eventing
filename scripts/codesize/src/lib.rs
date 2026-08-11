//! Code-size probe. Each `#[unsafe(no_mangle)] extern "C"` function below is
//! measured as its own `.text.<name>` section, so the numbers attribute to one
//! API shape rather than to the whole binary.
//!
//! Run via `scripts/codesize.sh`, not directly.
#![no_std]

use core::panic::PanicInfo;
use ph_eventing::counted_signal::{Consumer as CountConsumer, Producer as CountProducer};
use ph_eventing::EventBuf;

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
