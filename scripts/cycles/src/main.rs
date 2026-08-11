//! Instruction-cost probe for ph-eventing, measured under QEMU.
//!
//! Code size says nothing about *time*. This measures the other half of the
//! determinism claim: that the hot paths cost a bounded, knowable number of
//! instructions **regardless of buffer state**. A `push` into an empty buffer
//! and into a nearly-full one should differ by a small constant, not scale with
//! `N` or with occupancy.
//!
//! ## How regions are delimited
//!
//! QEMU models neither `DWT_CYCCNT` nor SysTick on `lm3s6965evb` or
//! `mps2-an385` -- both read zero, verified -- and Ubuntu's `qemu-system-arm`
//! ships no TCG plugins. What works without either is QEMU's execution trace:
//! `-accel tcg,one-insn-per-tb=on -d exec` emits one line per retired
//! instruction.
//!
//! Each measured region is bracketed by a never-inlined marker whose *symbol
//! name carries the label*, and closed by [`m_end`]. The runner reads the
//! symbol table, so adding a measurement here needs no change there -- an
//! earlier positional scheme silently mislabelled every row after an insertion.
//!
//! Run via `scripts/cycles.sh`, not directly.
#![no_std]
#![no_main]

use core::hint::black_box;
use cortex_m_rt::entry;
use cortex_m_semihosting::debug;
use ph_eventing::{CountedSignal, EventBuf, RingBuf, SeqRing};

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! {
    debug::exit(debug::EXIT_FAILURE);
    loop {}
}

/// Declares region markers.
///
/// Each body embeds a **unique immediate**, and that is load-bearing: with
/// identical `nop`-only bodies the linker folded every marker onto a
/// single address, the runner saw one label, and the output was silently empty.
/// `r12` is call-clobbered under AAPCS, so writing it in a `-> ()` function is
/// free and harmless.
macro_rules! markers {
    ($($idx:expr => $name:ident),* $(,)?) => {
        $(
            #[inline(never)]
            #[unsafe(no_mangle)]
            pub extern "C" fn $name() {
                unsafe {
                    core::arch::asm!(
                        "mov r12, {tag}",
                        tag = const $idx,
                        options(nomem, nostack, preserves_flags)
                    )
                };
            }
        )*
    };
}

markers! {
    0 => m_end,
    1 => m_overhead,
    // EventBuf -- backpressure SPSC
    2 => m_eb_push_empty,
    3 => m_eb_push_nearly_full,
    4 => m_eb_push_full_rejected,
    5 => m_eb_pop_full,
    6 => m_eb_pop_empty,
    7 => m_eb_peek,
    8 => m_eb_len,
    // SeqRing -- overwrite SPSC
    9 => m_sr_push_empty,
    10 => m_sr_push_overwriting,
    11 => m_sr_poll_value,
    12 => m_sr_poll_empty,
    13 => m_sr_poll_lagged,
    14 => m_sr_latest,
    // RingBuf -- single owner
    15 => m_rb_push_empty,
    16 => m_rb_push_overwriting,
    17 => m_rb_get,
    18 => m_rb_latest,
    19 => m_sr_poll_lagged_far,
    // CountedSignal -- payload-free SPSC
    20 => m_cs_increment,
    21 => m_cs_take_count,
    22 => m_cs_increment_saturated,
}

fn counted_signal_costs() {
    let signal = CountedSignal::new();
    let tx = signal.try_producer().expect("producer");
    let rx = signal.try_consumer().expect("consumer");

    m_cs_increment();
    tx.increment();
    m_end();

    m_cs_take_count();
    let _ = black_box(rx.take_count());
    m_end();
}

/// The sentinel arm only runs when the load observes `u32::MAX`, which is
/// unreachable through the public API in bounded time. The hidden
/// `_cycles-probe` feature seeds it directly so the saturated worst case is a
/// measured region, not a source-review claim. The third arm (stale `MAX`
/// re-read below `MAX` after a take) needs a racing consumer and is bounded by
/// this region plus one `fetch_add` by construction.
///
/// Kept in its own never-inlined frame with the signal reference escaped
/// through `black_box`: sharing `counted_signal_costs`'s frame was measured to
/// perturb the hot-path region (`cs increment` 8 -> 10) by changing its
/// codegen, and an unescaped local signal would let the optimiser fold the
/// seeded `MAX` into the branch being measured.
#[inline(never)]
fn counted_signal_saturated_costs() {
    let saturated = CountedSignal::with_count_for_probe(u32::MAX);
    let tx = black_box(&saturated).try_producer().expect("producer");

    m_cs_increment_saturated();
    tx.increment();
    m_end();
}

fn event_buf_costs() {
    let buf = EventBuf::<u32, 8>::new();
    let tx = buf.try_producer().expect("producer");
    let rx = buf.try_consumer().expect("consumer");

    m_eb_push_empty();
    black_box(tx.push(black_box(1))).ok();
    m_end();

    for i in 2..8 {
        tx.push(i).ok();
    }

    m_eb_push_nearly_full();
    black_box(tx.push(black_box(8))).ok();
    m_end();

    m_eb_push_full_rejected();
    black_box(tx.push(black_box(99))).ok();
    m_end();

    m_eb_peek();
    black_box(rx.peek());
    m_end();

    m_eb_len();
    black_box(buf.len());
    m_end();

    m_eb_pop_full();
    black_box(rx.pop());
    m_end();

    while rx.pop().is_some() {}

    m_eb_pop_empty();
    black_box(rx.pop());
    m_end();
}

fn seq_ring_costs() {
    let ring = SeqRing::<u32, 8>::new();
    let tx = ring.try_producer().expect("producer");
    let mut rx = ring.try_consumer().expect("consumer");

    m_sr_push_empty();
    black_box(tx.push(black_box(1)));
    m_end();

    m_sr_poll_value();
    black_box(rx.poll_one_value());
    m_end();

    m_sr_poll_empty();
    black_box(rx.poll_one_value());
    m_end();

    // Fill past capacity so the producer is overwriting live slots.
    for i in 0..16 {
        tx.push(i);
    }

    m_sr_push_overwriting();
    black_box(tx.push(black_box(99)));
    m_end();

    m_sr_latest();
    black_box(rx.latest_value());
    m_end();

    // The consumer is now far behind: this is the lag-recovery jump, and it is
    // the one place a naive implementation would walk the whole backlog.
    m_sr_poll_lagged();
    black_box(rx.poll_one_value());
    m_end();

    // Same again with a backlog two orders of magnitude larger. If recovery
    // were proportional to lag this would explode; if it is a jump, it costs
    // the same. This is the measurement that makes "bounded" a claim about the
    // algorithm rather than about one convenient input.
    for i in 0..2000 {
        tx.push(i);
    }
    m_sr_poll_lagged_far();
    black_box(rx.poll_one_value());
    m_end();
}

fn ring_buf_costs() {
    let mut ring = RingBuf::<u32, 8>::new();

    m_rb_push_empty();
    ring.push(black_box(1));
    m_end();

    for i in 2..=8 {
        ring.push(i);
    }

    m_rb_push_overwriting();
    ring.push(black_box(9));
    m_end();

    m_rb_get();
    black_box(ring.get(black_box(3)));
    m_end();

    m_rb_latest();
    black_box(ring.latest());
    m_end();
}

#[entry]
fn main() -> ! {
    // Two adjacent markers: the cost of the markers themselves, subtracted
    // from every other region by the runner.
    m_overhead();
    m_end();

    event_buf_costs();
    seq_ring_costs();
    ring_buf_costs();
    counted_signal_costs();
    counted_signal_saturated_costs();

    debug::exit(debug::EXIT_SUCCESS);
    loop {}
}
