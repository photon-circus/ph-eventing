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
#[cfg(not(any(feature = "latest-matrix", feature = "latest-block-matrix")))]
use ph_eventing::EventBuf;
#[cfg(any(feature = "latest-matrix", feature = "latest-block-matrix"))]
use ph_eventing::LatestBuf;
#[cfg(feature = "block-matrix")]
use ph_eventing::{Block, BlockBuilder};
#[cfg(not(any(
    feature = "block-matrix",
    feature = "latest-matrix",
    feature = "latest-block-matrix"
)))]
use ph_eventing::{RingBuf, SeqRing};

#[cfg(feature = "latest-block-matrix")]
#[path = "../../probes/block_shape.rs"]
mod block_probe;
#[cfg(feature = "latest-block-matrix")]
use block_probe::{BlockBuilderShape, BlockShape};

#[cfg(all(feature = "latest-matrix", feature = "latest-block-matrix"))]
compile_error!("latest-matrix and latest-block-matrix are mutually exclusive probes");

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! {
    debug::exit(debug::EXIT_FAILURE);
    loop {}
}

/// Declares region markers.
///
/// Each body embeds a **unique immediate**, and that is load-bearing: with
/// identical `nop`-only bodies the linker folded all nineteen markers onto a
/// single address, the runner saw one label, and the output was silently empty.
/// `r12` is call-clobbered under AAPCS, so writing it in a `-> ()` function is
/// free and harmless. The assembly deliberately does not claim `nomem`: each
/// marker is also a compiler memory barrier, preventing setup for an adjacent
/// payload from being hoisted into the measured region.
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
                        options(nostack, preserves_flags)
                    )
                };
            }
        )*
    };
}

#[cfg(not(any(
    feature = "block-matrix",
    feature = "latest-matrix",
    feature = "latest-block-matrix"
)))]
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
}

#[cfg(feature = "block-matrix")]
markers! {
    0 => m_end,
    1 => m_overhead,
    // BlockBuilder completion + EventBuf publication. Width is bytes/sample.
    20 => m_bp_w2_n8_accepted,
    21 => m_bp_w2_n8_rejected,
    22 => m_bp_w2_n32_accepted,
    23 => m_bp_w2_n32_rejected,
    24 => m_bp_w2_n128_accepted,
    25 => m_bp_w2_n128_rejected,
    26 => m_bp_w8_n8_accepted,
    27 => m_bp_w8_n8_rejected,
    28 => m_bp_w8_n32_accepted,
    29 => m_bp_w8_n32_rejected,
    30 => m_bp_w8_n128_accepted,
    31 => m_bp_w8_n128_rejected,
    32 => m_bp_w16_n8_accepted,
    33 => m_bp_w16_n8_rejected,
    34 => m_bp_w16_n32_accepted,
    35 => m_bp_w16_n32_rejected,
    36 => m_bp_w16_n128_accepted,
    37 => m_bp_w16_n128_rejected,
}

#[cfg(feature = "latest-block-matrix")]
markers! {
    0 => m_end,
    1 => m_overhead,
    // D3 composition rows: sample width in bytes, then complete Block shape.
    50 => m_lc_sample_w2_publish_first,
    51 => m_lc_sample_w2_publish_replace,
    52 => m_lc_sample_w2_take_pending,
    53 => m_lc_sample_w2_take_empty,
    54 => m_lc_sample_w8_publish_first,
    55 => m_lc_sample_w8_publish_replace,
    56 => m_lc_sample_w8_take_pending,
    57 => m_lc_sample_w8_take_empty,
    58 => m_lc_sample_w16_publish_first,
    59 => m_lc_sample_w16_publish_replace,
    60 => m_lc_sample_w16_take_pending,
    61 => m_lc_sample_w16_take_empty,
    62 => m_lc_block_w2_n8_publish_first,
    63 => m_lc_block_w2_n8_publish_replace,
    64 => m_lc_block_w2_n8_take_pending,
    65 => m_lc_block_w2_n8_take_empty,
    66 => m_lc_block_w2_n32_publish_first,
    67 => m_lc_block_w2_n32_publish_replace,
    68 => m_lc_block_w2_n32_take_pending,
    69 => m_lc_block_w2_n32_take_empty,
    70 => m_lc_block_w2_n128_publish_first,
    71 => m_lc_block_w2_n128_publish_replace,
    72 => m_lc_block_w2_n128_take_pending,
    73 => m_lc_block_w2_n128_take_empty,
    74 => m_lc_block_w8_n8_publish_first,
    75 => m_lc_block_w8_n8_publish_replace,
    76 => m_lc_block_w8_n8_take_pending,
    77 => m_lc_block_w8_n8_take_empty,
    78 => m_lc_block_w8_n32_publish_first,
    79 => m_lc_block_w8_n32_publish_replace,
    80 => m_lc_block_w8_n32_take_pending,
    81 => m_lc_block_w8_n32_take_empty,
    82 => m_lc_block_w8_n128_publish_first,
    83 => m_lc_block_w8_n128_publish_replace,
    84 => m_lc_block_w8_n128_take_pending,
    85 => m_lc_block_w8_n128_take_empty,
    86 => m_lc_block_w16_n8_publish_first,
    87 => m_lc_block_w16_n8_publish_replace,
    88 => m_lc_block_w16_n8_take_pending,
    89 => m_lc_block_w16_n8_take_empty,
    90 => m_lc_block_w16_n32_publish_first,
    91 => m_lc_block_w16_n32_publish_replace,
    92 => m_lc_block_w16_n32_take_pending,
    93 => m_lc_block_w16_n32_take_empty,
    94 => m_lc_block_w16_n128_publish_first,
    95 => m_lc_block_w16_n128_publish_replace,
    96 => m_lc_block_w16_n128_take_pending,
    97 => m_lc_block_w16_n128_take_empty,
    // Final BlockBuilder push + LatestBuf publication, matching #28's boundary.
    100 => m_lc_complete_w2_n8_publish_first,
    101 => m_lc_complete_w2_n8_publish_replace,
    102 => m_lc_complete_w2_n32_publish_first,
    103 => m_lc_complete_w2_n32_publish_replace,
    104 => m_lc_complete_w2_n128_publish_first,
    105 => m_lc_complete_w2_n128_publish_replace,
    106 => m_lc_complete_w8_n8_publish_first,
    107 => m_lc_complete_w8_n8_publish_replace,
    108 => m_lc_complete_w8_n32_publish_first,
    109 => m_lc_complete_w8_n32_publish_replace,
    110 => m_lc_complete_w8_n128_publish_first,
    111 => m_lc_complete_w8_n128_publish_replace,
    112 => m_lc_complete_w16_n8_publish_first,
    113 => m_lc_complete_w16_n8_publish_replace,
    114 => m_lc_complete_w16_n32_publish_first,
    115 => m_lc_complete_w16_n32_publish_replace,
    116 => m_lc_complete_w16_n128_publish_first,
    117 => m_lc_complete_w16_n128_publish_replace,
}

#[cfg(feature = "latest-matrix")]
markers! {
    0 => m_end,
    1 => m_overhead,
    // LatestBuf -- payload width is part of each label.
    20 => m_lb_u32_publish_first,
    21 => m_lb_u32_publish_replace,
    22 => m_lb_u32_take_pending,
    23 => m_lb_u32_take_empty,
    24 => m_lb_w16_publish_first,
    25 => m_lb_w16_publish_replace,
    26 => m_lb_w16_take_pending,
    27 => m_lb_w16_take_empty,
    28 => m_lb_block128_publish_first,
    29 => m_lb_block128_publish_replace,
    30 => m_lb_block128_take_pending,
    31 => m_lb_block128_take_empty,
    // Channel-resident role state / stateless handle costs (A.3 evidence).
    32 => m_lb_producer_drop,
    33 => m_lb_producer_reacquire,
    34 => m_lb_consumer_drop,
    35 => m_lb_consumer_reacquire,
}

#[cfg(not(any(
    feature = "block-matrix",
    feature = "latest-matrix",
    feature = "latest-block-matrix"
)))]
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

#[cfg(not(any(
    feature = "block-matrix",
    feature = "latest-matrix",
    feature = "latest-block-matrix"
)))]
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

#[cfg(not(any(
    feature = "block-matrix",
    feature = "latest-matrix",
    feature = "latest-block-matrix"
)))]
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

#[cfg(feature = "block-matrix")]
macro_rules! measure_block_shape {
    ($sample:ty, $n:literal, $value:expr, $accepted:ident, $rejected:ident) => {{
        let mut accepted_fill = BlockBuilder::<$sample, $n>::new();
        for sequence in 1..$n {
            let _ = accepted_fill.push(sequence as u32, black_box($value));
        }

        let queue = EventBuf::<Block<$sample, $n>, 1>::new();
        let tx = queue.try_producer().expect("producer");

        $accepted();
        let block = accepted_fill
            .push($n as u32, black_box($value))
            .expect("contiguous")
            .expect("complete");
        let _ = black_box(tx.push(block));
        m_end();

        let mut rejected_fill = BlockBuilder::<$sample, $n>::new();
        for sequence in 1..$n {
            let _ = rejected_fill.push(sequence as u32, black_box($value));
        }

        $rejected();
        let block = rejected_fill
            .push($n as u32, black_box($value))
            .expect("contiguous")
            .expect("complete");
        let _ = black_box(tx.push(block));
        m_end();
    }};
}

#[cfg(any(feature = "latest-matrix", feature = "latest-block-matrix"))]
macro_rules! measure_latest_payload {
    (
        $payload:ty,
        $first:expr,
        $second:expr,
        $publish_first:ident,
        $publish_replace:ident,
        $take_pending:ident,
        $take_empty:ident
    ) => {{
        let channel = LatestBuf::<$payload>::new();
        let producer = channel.try_producer().expect("producer");
        let consumer = channel.try_consumer().expect("consumer");

        $publish_first();
        let _ = black_box(producer.publish(black_box($first)));
        m_end();

        $publish_replace();
        let _ = black_box(producer.publish(black_box($second)));
        m_end();

        $take_pending();
        let _ = black_box(consumer.take_latest());
        m_end();

        $take_empty();
        let _ = black_box(consumer.take_latest());
        m_end();
    }};
}

#[cfg(feature = "block-matrix")]
fn block_publication_costs() {
    measure_block_shape!(u16, 8, 1_u16, m_bp_w2_n8_accepted, m_bp_w2_n8_rejected);
    measure_block_shape!(u16, 32, 1_u16, m_bp_w2_n32_accepted, m_bp_w2_n32_rejected);
    measure_block_shape!(
        u16,
        128,
        1_u16,
        m_bp_w2_n128_accepted,
        m_bp_w2_n128_rejected
    );
    measure_block_shape!(u64, 8, 1_u64, m_bp_w8_n8_accepted, m_bp_w8_n8_rejected);
    measure_block_shape!(u64, 32, 1_u64, m_bp_w8_n32_accepted, m_bp_w8_n32_rejected);
    measure_block_shape!(
        u64,
        128,
        1_u64,
        m_bp_w8_n128_accepted,
        m_bp_w8_n128_rejected
    );
    measure_block_shape!(
        [u64; 2],
        8,
        [1_u64; 2],
        m_bp_w16_n8_accepted,
        m_bp_w16_n8_rejected
    );
    measure_block_shape!(
        [u64; 2],
        32,
        [1_u64; 2],
        m_bp_w16_n32_accepted,
        m_bp_w16_n32_rejected
    );
    measure_block_shape!(
        [u64; 2],
        128,
        [1_u64; 2],
        m_bp_w16_n128_accepted,
        m_bp_w16_n128_rejected
    );
}

#[cfg(feature = "latest-block-matrix")]
macro_rules! measure_complete_publication {
    (
        $sample:ty,
        $n:literal,
        $value:expr,
        $publish_first:ident,
        $publish_replace:ident
    ) => {{
        let channel = LatestBuf::<BlockShape<$sample, $n>>::new();
        let producer = channel.try_producer().expect("producer");

        let mut first_fill = BlockBuilderShape::<$sample, $n>::new();
        for sequence in 1..$n {
            let _ = first_fill.push(sequence as u32, black_box($value));
        }
        $publish_first();
        let block = first_fill
            .push($n as u32, black_box($value))
            .expect("contiguous")
            .expect("complete");
        let _ = black_box(producer.publish(block));
        m_end();

        let mut replacement_fill = BlockBuilderShape::<$sample, $n>::new();
        for sequence in 1..$n {
            let _ = replacement_fill.push(sequence as u32, black_box($value));
        }
        $publish_replace();
        let block = replacement_fill
            .push($n as u32, black_box($value))
            .expect("contiguous")
            .expect("complete");
        let _ = black_box(producer.publish(block));
        m_end();
    }};
}

#[cfg(feature = "latest-block-matrix")]
fn latest_block_composition_costs() {
    measure_latest_payload!(
        u16,
        1_u16,
        2_u16,
        m_lc_sample_w2_publish_first,
        m_lc_sample_w2_publish_replace,
        m_lc_sample_w2_take_pending,
        m_lc_sample_w2_take_empty
    );
    measure_latest_payload!(
        u64,
        1_u64,
        2_u64,
        m_lc_sample_w8_publish_first,
        m_lc_sample_w8_publish_replace,
        m_lc_sample_w8_take_pending,
        m_lc_sample_w8_take_empty
    );
    measure_latest_payload!(
        [u64; 2],
        [1_u64; 2],
        [2_u64; 2],
        m_lc_sample_w16_publish_first,
        m_lc_sample_w16_publish_replace,
        m_lc_sample_w16_take_pending,
        m_lc_sample_w16_take_empty
    );
    measure_latest_payload!(
        BlockShape<u16, 8>,
        BlockShape::filled(1_u16),
        BlockShape::filled(2_u16),
        m_lc_block_w2_n8_publish_first,
        m_lc_block_w2_n8_publish_replace,
        m_lc_block_w2_n8_take_pending,
        m_lc_block_w2_n8_take_empty
    );
    measure_latest_payload!(
        BlockShape<u16, 32>,
        BlockShape::filled(1_u16),
        BlockShape::filled(2_u16),
        m_lc_block_w2_n32_publish_first,
        m_lc_block_w2_n32_publish_replace,
        m_lc_block_w2_n32_take_pending,
        m_lc_block_w2_n32_take_empty
    );
    measure_latest_payload!(
        BlockShape<u16, 128>,
        BlockShape::filled(1_u16),
        BlockShape::filled(2_u16),
        m_lc_block_w2_n128_publish_first,
        m_lc_block_w2_n128_publish_replace,
        m_lc_block_w2_n128_take_pending,
        m_lc_block_w2_n128_take_empty
    );
    measure_latest_payload!(
        BlockShape<u64, 8>,
        BlockShape::filled(1_u64),
        BlockShape::filled(2_u64),
        m_lc_block_w8_n8_publish_first,
        m_lc_block_w8_n8_publish_replace,
        m_lc_block_w8_n8_take_pending,
        m_lc_block_w8_n8_take_empty
    );
    measure_latest_payload!(
        BlockShape<u64, 32>,
        BlockShape::filled(1_u64),
        BlockShape::filled(2_u64),
        m_lc_block_w8_n32_publish_first,
        m_lc_block_w8_n32_publish_replace,
        m_lc_block_w8_n32_take_pending,
        m_lc_block_w8_n32_take_empty
    );
    measure_latest_payload!(
        BlockShape<u64, 128>,
        BlockShape::filled(1_u64),
        BlockShape::filled(2_u64),
        m_lc_block_w8_n128_publish_first,
        m_lc_block_w8_n128_publish_replace,
        m_lc_block_w8_n128_take_pending,
        m_lc_block_w8_n128_take_empty
    );
    measure_latest_payload!(
        BlockShape<[u64; 2], 8>,
        BlockShape::filled([1_u64; 2]),
        BlockShape::filled([2_u64; 2]),
        m_lc_block_w16_n8_publish_first,
        m_lc_block_w16_n8_publish_replace,
        m_lc_block_w16_n8_take_pending,
        m_lc_block_w16_n8_take_empty
    );
    measure_latest_payload!(
        BlockShape<[u64; 2], 32>,
        BlockShape::filled([1_u64; 2]),
        BlockShape::filled([2_u64; 2]),
        m_lc_block_w16_n32_publish_first,
        m_lc_block_w16_n32_publish_replace,
        m_lc_block_w16_n32_take_pending,
        m_lc_block_w16_n32_take_empty
    );
    measure_latest_payload!(
        BlockShape<[u64; 2], 128>,
        BlockShape::filled([1_u64; 2]),
        BlockShape::filled([2_u64; 2]),
        m_lc_block_w16_n128_publish_first,
        m_lc_block_w16_n128_publish_replace,
        m_lc_block_w16_n128_take_pending,
        m_lc_block_w16_n128_take_empty
    );

    measure_complete_publication!(
        u16,
        8,
        1_u16,
        m_lc_complete_w2_n8_publish_first,
        m_lc_complete_w2_n8_publish_replace
    );
    measure_complete_publication!(
        u16,
        32,
        1_u16,
        m_lc_complete_w2_n32_publish_first,
        m_lc_complete_w2_n32_publish_replace
    );
    measure_complete_publication!(
        u16,
        128,
        1_u16,
        m_lc_complete_w2_n128_publish_first,
        m_lc_complete_w2_n128_publish_replace
    );
    measure_complete_publication!(
        u64,
        8,
        1_u64,
        m_lc_complete_w8_n8_publish_first,
        m_lc_complete_w8_n8_publish_replace
    );
    measure_complete_publication!(
        u64,
        32,
        1_u64,
        m_lc_complete_w8_n32_publish_first,
        m_lc_complete_w8_n32_publish_replace
    );
    measure_complete_publication!(
        u64,
        128,
        1_u64,
        m_lc_complete_w8_n128_publish_first,
        m_lc_complete_w8_n128_publish_replace
    );
    measure_complete_publication!(
        [u64; 2],
        8,
        [1_u64; 2],
        m_lc_complete_w16_n8_publish_first,
        m_lc_complete_w16_n8_publish_replace
    );
    measure_complete_publication!(
        [u64; 2],
        32,
        [1_u64; 2],
        m_lc_complete_w16_n32_publish_first,
        m_lc_complete_w16_n32_publish_replace
    );
    measure_complete_publication!(
        [u64; 2],
        128,
        [1_u64; 2],
        m_lc_complete_w16_n128_publish_first,
        m_lc_complete_w16_n128_publish_replace
    );
}

#[cfg(feature = "latest-matrix")]
fn latest_buf_costs() {
    measure_latest_payload!(
        u32,
        1_u32,
        2_u32,
        m_lb_u32_publish_first,
        m_lb_u32_publish_replace,
        m_lb_u32_take_pending,
        m_lb_u32_take_empty
    );
    measure_latest_payload!(
        [u32; 4],
        [1_u32; 4],
        [2_u32; 4],
        m_lb_w16_publish_first,
        m_lb_w16_publish_replace,
        m_lb_w16_take_pending,
        m_lb_w16_take_empty
    );
    measure_latest_payload!(
        [u32; 32],
        [1_u32; 32],
        [2_u32; 32],
        m_lb_block128_publish_first,
        m_lb_block128_publish_replace,
        m_lb_block128_take_pending,
        m_lb_block128_take_empty
    );

    let channel = LatestBuf::<u32>::new();
    let producer = channel.try_producer().expect("producer");
    let consumer = channel.try_consumer().expect("consumer");

    m_lb_producer_drop();
    drop(black_box(producer));
    m_end();

    m_lb_producer_reacquire();
    let producer = black_box(channel.try_producer());
    m_end();
    drop(producer.expect("reacquired producer"));

    m_lb_consumer_drop();
    drop(black_box(consumer));
    m_end();

    m_lb_consumer_reacquire();
    let consumer = black_box(channel.try_consumer());
    m_end();
    drop(consumer.expect("reacquired consumer"));
}

// Semihosting exit terminates QEMU, but its signature is not `!`; the fallback
// must remain side-effect-free so it cannot contaminate any measured region.
#[allow(clippy::empty_loop)]
#[entry]
fn main() -> ! {
    // Two adjacent markers: the cost of the markers themselves, subtracted
    // from every other region by the runner.
    m_overhead();
    m_end();

    #[cfg(not(any(
        feature = "block-matrix",
        feature = "latest-matrix",
        feature = "latest-block-matrix"
    )))]
    {
        event_buf_costs();
        seq_ring_costs();
        ring_buf_costs();
    }
    #[cfg(feature = "block-matrix")]
    block_publication_costs();
    #[cfg(feature = "latest-matrix")]
    latest_buf_costs();
    #[cfg(feature = "latest-block-matrix")]
    latest_block_composition_costs();

    debug::exit(debug::EXIT_SUCCESS);
    loop {}
}
