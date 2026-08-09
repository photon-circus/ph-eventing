//! Stack-allocated ring buffers for no-std embedded targets.
//!
//! # Primitives
//!
//! | Type | When to reach for it |
//! |------|----------------------|
//! | [`RingBuf`] | Single-owner ring — simple, no atomics, `&mut` access. |
//! | [`SeqRing`] | Lock-free SPSC ring that **overwrites** old entries (lossy, high-throughput). |
//! | [`EventBuf`] | Lock-free SPSC ring with **backpressure** — rejects pushes when full. |
//!
//! All three are fixed-size, zero-allocation, and generic over `T: Copy`.
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
//! let producer = ring.producer();
//! let mut consumer = ring.consumer();
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
//! let producer = buf.producer();
//! let consumer = buf.consumer();
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
//! let sp = seq.producer();
//! let mut sc = seq.consumer();
//!
//! sp.push(1); sp.push(2);
//!
//! let eb = EventBuf::<u32, 8>::new();
//! let mut ep = eb.producer();
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
//! `SeqRing` and `EventBuf` require 32-bit atomics. For targets that lack them
//! (for example `thumbv6m-none-eabi`), enable
//! `portable-atomic-unsafe-assume-single-core` or `portable-atomic-critical-section`.
//! The crate always compiles those modules, so no-atomic targets need one of
//! those features even when only [`RingBuf`] is used. `RingBuf` itself uses no
//! atomics.
//!
//! # Safety and concurrency
//! - `RingBuf` is a plain struct — standard Rust borrow rules apply.
//! - `SeqRing` and `EventBuf` are SPSC by design: exactly one producer and one
//!   consumer must be active. `producer()`/`consumer()` will panic if called
//!   while another handle of the same kind is active. Using unsafe to bypass
//!   these constraints is undefined behavior.
//!
//! # SeqRing semantics
//! - Sequence numbers are monotonically increasing `u32` values; `0` is reserved for "empty".
//! - `poll_one`/`poll_up_to` drain in-order and return `PollStats`.
//! - `latest` reads the newest value without advancing the consumer cursor.
//! - If the consumer lags by more than `N`, it skips ahead and reports drops via `PollStats`.
//!
//! # EventBuf semantics
//! - `push` returns `Err(val)` when the buffer is full — no data is silently lost.
//! - `pop` returns the oldest item, or `None` when empty.
//! - `drain(max, hook)` consumes up to `max` items through a callback.
#![no_std]

#[cfg(all(not(target_has_atomic = "32"), not(feature = "portable-atomic")))]
compile_error!(
    "ph-eventing requires 32-bit atomics. For thumbv6m and other no-atomic targets, \
enable either the portable-atomic-unsafe-assume-single-core or portable-atomic-critical-section feature."
);

pub mod event_buf;
pub mod ring;
pub mod seq_ring;
pub(crate) mod sync;
pub mod traits;

pub use event_buf::EventBuf;
pub use ring::RingBuf;
pub use seq_ring::{PollStats, SeqRing};
pub use traits::{Link, Sink, Source};

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
