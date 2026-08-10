//! Instruction-cost probe for ph-eventing, measured under QEMU.
//!
//! Code size says nothing about *time*. This measures the other half of the
//! determinism claim: that the hot paths cost a bounded, knowable number of
//! instructions **regardless of buffer state**. A `push` into an empty buffer
//! and a `push` into a full one should differ by a small constant, not scale
//! with `N` or with how much data is queued.
//!
//! ## Why a marker function and not a cycle counter
//!
//! QEMU models neither `DWT_CYCCNT` nor SysTick on `lm3s6965evb` or
//! `mps2-an385` -- both read zero, verified. Ubuntu's `qemu-system-arm` also
//! ships no TCG plugins. What does work is QEMU's own execution trace: with
//! `-accel tcg,one-insn-per-tb=on -d exec`, every executed instruction emits
//! one line. [`mark`] is a never-inlined function at a known address; the
//! runner segments the trace on its PC and counts the instructions between
//! hits. That measures instructions retired on the guest, exactly, with no
//! dependence on any modelled peripheral.
//!
//! Run via `scripts/cycles.sh`, not directly.
#![no_std]
#![no_main]

use core::hint::black_box;
use cortex_m_rt::entry;
use cortex_m_semihosting::debug;
use ph_eventing::EventBuf;

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! {
    debug::exit(debug::EXIT_FAILURE);
    loop {}
}

/// Segment boundary. Never inlined, so it keeps a stable address the runner
/// can find with `nm`, and contains a `nop` so it cannot be folded away.
#[inline(never)]
#[unsafe(no_mangle)]
pub extern "C" fn mark() {
    unsafe { core::arch::asm!("nop", options(nomem, nostack, preserves_flags)) };
}

#[entry]
fn main() -> ! {
    let buf = EventBuf::<u32, 8>::new();
    let tx = buf.try_producer().expect("producer");
    let rx = buf.try_consumer().expect("consumer");

    // Segment 0: empty. Calibrates the cost of the markers themselves so the
    // runner can subtract it from every other segment.
    mark();
    mark();

    // Segment 1: push into an empty buffer.
    black_box(tx.push(black_box(1))).ok();
    mark();

    // Segment 2: push into a buffer with 7 of 8 slots used -- still accepted.
    for i in 2..8 {
        tx.push(i).ok();
    }
    mark();
    black_box(tx.push(black_box(8))).ok();
    mark();

    // Segment 4: push into a full buffer -- rejected. The backpressure path.
    black_box(tx.push(black_box(99))).ok();
    mark();

    // Segment 5: pop from a full buffer.
    black_box(rx.pop());
    mark();

    // Segment 6: drain, then pop from empty.
    while rx.pop().is_some() {}
    mark();
    black_box(rx.pop());
    mark();

    // Segment 8: len(), the one bounded-retry loop in the crate.
    black_box(buf.len());
    mark();

    debug::exit(debug::EXIT_SUCCESS);
    loop {}
}
