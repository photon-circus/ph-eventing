//! Deterministic handoff primitives for no-std embedded targets.
//!
//! # Primitives
//!
//! | Type | When to reach for it |
//! |------|----------------------|
//! | [`Block`] / [`BlockBuilder`] | Complete contiguous sample windows; compose with a transport. |
//! | [`RingBuf`] | Single-owner ring — simple, no atomics, `&mut` access. |
//! | [`SeqRing`] | Lock-free SPSC ring that **overwrites** old entries (lossy, high-throughput). |
//! | [`EventBuf`] | Lock-free SPSC ring with **backpressure** — rejects pushes when full. |
//! | [`CountedSignal`] | Saturating SPSC count for identical, payload-free events. |
//! | [`EventFlags`] | Coalesced SPSC condition set — one bit per condition, one atomic operation per hot path. |
//! | [`LatestBuf`] | Freshness-first SPSC snapshot — retains one newest unread value. |
//!
//! All are fixed-size and zero-allocation. The buffer types are generic
//! over `T: Copy`; [`CountedSignal`] carries no payload and [`EventFlags`]
//! provides exactly 32 payload-free conditions.
//!
//! # Common traits
//!
//! | Trait | Role | Implementors |
//! |-------|------|--------------|
//! | [`Sink<T>`](traits::Sink) | Accept events | `RingBuf`, `seq_ring::Producer`, `event_buf::Producer` |
//! | [`Source<T>`](traits::Source) | Yield events | `seq_ring::Consumer`, `event_buf::Consumer` |
//! | [`Link<In,Out>`](traits::Link) | Both | Blanket impl for any `Sink<In> + Source<Out>` |
//!
//! The [`traits::forward`] function transfers items from any `Source` to any
//! `Sink`, making it easy to bridge different buffer types.
//!
//! # Static bring-up
//!
//! [`static_spsc!`](crate::static_spsc) declares a `static` buffer together with
//! named handle types, so a signature need not spell out
//! `event_buf::Producer<'static, T, N>`:
//!
//! ```
//! ph_eventing::static_spsc! {
//!     pub mod telemetry: EventBuf<u32, 64>;
//! }
//!
//! fn on_sample(tx: &telemetry::Tx, v: u32) { let _ = tx.push(v); }
//!
//! let (tx, rx) = telemetry::take().expect("first take");
//! on_sample(&tx, 1);
//! assert_eq!(rx.pop(), Some(1));
//! ```
//!
//! # Quick start — `RingBuf`
//! ```
//! use ph_eventing::RingBuf;
//!
//! let mut ring = RingBuf::<u32, 4>::new();
//! ring.push(1);
//! ring.push(2);
//! ring.push(3);
//! assert_eq!(ring.latest(), Some(3));
//! ```
//!
//! # Quick start — `SeqRing`
//! ```
//! use ph_eventing::SeqRing;
//!
//! let ring = SeqRing::<u32, 64>::new();
//! let producer = ring.try_producer().expect("producer");
//! let mut consumer = ring.try_consumer().expect("consumer");
//!
//! producer.push(42);
//! consumer.poll_one(|seq, v| {
//!     assert_eq!(seq, 1);
//!     assert_eq!(*v, 42);
//! });
//! ```
//!
//! # Quick start — `EventBuf`
//! ```
//! use ph_eventing::EventBuf;
//!
//! let buf = EventBuf::<u32, 4>::new();
//! let producer = buf.try_producer().expect("producer");
//! let consumer = buf.try_consumer().expect("consumer");
//!
//! assert!(producer.push(1).is_ok());
//! assert!(producer.push(2).is_ok());
//! assert_eq!(consumer.pop(), Some(1));
//! ```
//!
//! # Quick start — `forward`
//! ```
//! use ph_eventing::{SeqRing, EventBuf};
//! use ph_eventing::traits::{Source, Sink, forward};
//!
//! let seq = SeqRing::<u32, 8>::new();
//! let sp = seq.try_producer().expect("producer");
//! let mut sc = seq.try_consumer().expect("consumer");
//!
//! sp.push(1); sp.push(2);
//!
//! let eb = EventBuf::<u32, 8>::new();
//! let mut ep = eb.try_producer().expect("producer");
//!
//! let (n, err) = forward(&mut sc, &mut ep, 10);
//! assert_eq!(n, 2);
//! assert!(err.is_none());
//! ```
//!
//! # No-std
//! The crate is `#![no_std]` by default. Tests require `std`.
//!
//! # Targets without atomics
//! `SeqRing`, `EventBuf`, and `EventFlags` require 32-bit atomics. For targets that lack them
//! (for example `thumbv6m-none-eabi`), enable
//! `portable-atomic-unsafe-assume-single-core` or `portable-atomic-critical-section`.
//! The crate always compiles those modules, so no-atomic targets need one of
//! those features even when only [`RingBuf`] is used. `RingBuf` itself uses no
//! atomics.
//!
//! # Safety and concurrency
//! - `RingBuf` has no atomics and no interior mutability — standard Rust borrow
//!   rules apply. It stores slots as `MaybeUninit<T>` and reads only live
//!   entries, so it does contain `unsafe`.
//! - `SeqRing`, `EventBuf`, and `EventFlags` are SPSC by design: exactly one
//!   producer and one consumer must be active. Handle acquisition is
//!   `try_producer()` / `try_consumer()`, which return `None` rather than
//!   panicking — on a microcontroller a panic is a reset. (The panicking
//!   `producer()` / `consumer()`, deprecated since 0.2.0, were removed in
//!   0.3.0.) Using unsafe to bypass these constraints is undefined behavior.
//!
//!   The examples here use `.expect(...)` for brevity, which is a panic. That
//!   is fine in a doctest on a host; in firmware, branch on the `None`:
//!
//! ```
//! # use ph_eventing::EventBuf;
//! # let buf = EventBuf::<u32, 4>::new();
//! let Some(tx) = buf.try_producer() else {
//!     return; // already claimed -- report it, do not reset the device
//! };
//! # let _ = tx;
//! ```
//! - [`EventBuf`] is race-free by construction — its producer and consumer
//!   never touch the same slot — and passes Miri with the data-race detector
//!   enabled.
//! - [`SeqRing`] is a seqlock and carries a **known formal data race**. A
//!   raced copy is discarded and never becomes an invalid value — within the
//!   whole-span bound: the discard compares `u32` sequences, so a consumer
//!   stalled mid-read for a full `2^32 - 1` publications can pass both checks
//!   against a rewritten slot (the [`seq_ring`] "whole-span sequence
//!   aliasing" section carries the reachability arithmetic and escape
//!   hatches). The access itself is undefined behaviour by the letter of the
//!   memory model. Practical
//!   consequence: running Miri over a test that drives this ring from two
//!   threads reports UB inside this crate — that is the deviation, not a new
//!   bug. It is a deliberate trade of formal soundness for accepting any
//!   `T: Copy`; the [`seq_ring`] module docs give the alternatives and why each
//!   was rejected. [`EventBuf`] has no such caveat, but applies backpressure
//!   rather than overwriting, so it is not a drop-in replacement.
//!
//! # Using it across contexts
//! The typical embedded shape is a producer in an interrupt handler and a
//! consumer in a task loop.
//!
//! - [`SeqRing`] and [`EventBuf`] are `Sync` when `T: Send`, and [`EventFlags`]
//!   is `Sync`, so a shared reference can be handed to both contexts. The
//!   `Producer` and `Consumer` handles are
//!   `Send + !Sync`: move each into the context that owns it, never share one.
//! - The handles borrow the buffer, so the buffer must outlive them.
//! - **`new()` is a `const fn`** on the normal build, so
//!   `static BUF: EventBuf<u32, 64> = EventBuf::new();` works. (Under
//!   `--cfg loom` it is non-const because Loom's atomics are not
//!   const-constructible.) Handles still borrow the buffer, so an ISR /
//!   task split typically pairs the `static` with a `StaticCell` or similar
//!   for the handles themselves.
//!
//! `N` is fixed at compile time and the buffer lives inline —
//! `N * size_of::<T>()` bytes, no allocation. For [`EventBuf`] it is the
//! backpressure threshold; for [`SeqRing`] it is how far the consumer may lag
//! before entries are lost. It need not be a power of two.
//!
//! # SeqRing semantics
//! - Sequence numbers are monotonically increasing `u32` values; `0` is reserved for "empty".
//! - `poll_one`/`poll_up_to` drain in-order and return `PollStats`; `poll_one_value`
//!   returns `(seq, T)` without a hook.
//! - `latest` / `latest_value` read the newest value without advancing the consumer cursor.
//! - If the consumer lags by more than `N`, it skips ahead and reports drops via `PollStats`.
//! - Once every `2^32 - 1` pushes the sequence counter wraps, and a few extra entries are dropped
//!   there because `push` skips the reserved sequence `0`: exactly one for a power-of-two `N`,
//!   none if `N` divides `2^32 - 1`, up to `N - 1` otherwise. Reported as ordinary drops, never a
//!   stale or torn value within the whole-span bound stated in the [`seq_ring`]
//!   module docs. Prefer a power of two for `N`.
//! - `Consumer::dropped` saturates rather than wrapping; `usize` is 32 bits on
//!   the targets this crate ships to, so a long-lived lagging consumer can
//!   reach the top of the range.
//!
//! # EventBuf semantics
//! - `push` returns `Err(val)` when the buffer is full — no data is silently lost.
//! - `pop` returns the oldest item, or `None` when empty.
//! - `peek` copies the oldest item without advancing the consumer cursor.
//! - `drain(max, hook)` consumes up to `max` items through a callback.
//!
//! # EventFlags semantics
//! - [`event_flags::Producer::raise`] unions a mask into the pending set.
//! - [`event_flags::Consumer::take_all`] atomically returns and clears that set.
//! - Duplicate raises may coalesce; ordering and multiplicity are not retained.
//! - A take that observes a raise also observes memory actions sequenced before it.
#![no_std]

#[cfg(all(not(target_has_atomic = "32"), not(feature = "portable-atomic")))]
compile_error!(
    "ph-eventing requires 32-bit atomics. For thumbv6m and other no-atomic targets, \
enable either the portable-atomic-unsafe-assume-single-core or portable-atomic-critical-section feature."
);

// NOTE: `portable-atomic-unsafe-assume-single-core` and
// `portable-atomic-critical-section` select different portable-atomic backends
// and cannot both be enabled — which makes `--all-features` unsupported for
// this crate. The guard for it lives in build.rs, not here: both features
// forward straight to portable-atomic, whose own `compile_error!` fires while
// the dependency compiles, so a guard in this file would never be reached. A
// build script does not depend on portable-atomic and runs regardless.

#[macro_use]
mod macros;

pub mod block;
pub mod counted_signal;
pub mod event_buf;
pub mod event_flags;
pub mod latest_buf;
pub mod ring;
pub mod seq_ring;
pub(crate) mod sync;
pub mod traits;

pub use block::{Block, BlockBuilder, FillError};
pub use counted_signal::{CountSnapshot, CountedSignal};
pub use event_buf::EventBuf;
pub use event_flags::{EventFlags, EventMask};
pub use latest_buf::{LatestBuf, LatestItem, PublishReport};
pub use ring::RingBuf;
pub use seq_ring::{PollStats, SeqRing};
pub use traits::{LatestSink, LatestSource, Link, Sink, Source};

#[cfg(all(loom, test))]
mod loom_tests;

#[cfg(test)]
extern crate std;

/// Helpers shared by the concurrency tests.
#[cfg(test)]
pub(crate) mod test_support {
    /// Scale a stress-test loop count for the current interpreter.
    ///
    /// Miri executes MIR rather than machine code, so a native iteration count
    /// would take hours. Miri's value is schedule exploration, not volume — a
    /// few hundred interleavings under its weak-memory model catch far more
    /// than millions of native iterations on a strongly-ordered x86 host.
    pub(crate) fn iterations(native: u32) -> u32 {
        if cfg!(miri) { 200 } else { native }
    }
}
