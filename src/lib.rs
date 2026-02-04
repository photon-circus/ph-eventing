//! Eventing primitives for no-std embedded targets.
//!
//! # Highlights
//! - Lock-free SPSC sequence ring for high-throughput telemetry.
//! - No allocation, no dynamic dispatch.
//! - Designed for fast producers and potentially slower consumers.
//!
//! # Quick start
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
//! # No-std
//! The crate is `#![no_std]` by default. Tests require `std`.
//!
//! # Targets without atomics
//! For targets that lack 32-bit atomics (for example `thumbv6m-none-eabi`), enable
//! `portable-atomic-unsafe-assume-single-core` or `portable-atomic-critical-section`.
//!
//! # Safety and concurrency
//! This crate is SPSC by design: exactly one producer and one consumer must be active.
//! `producer()`/`consumer()` will panic if called while another handle of the same kind is active.
//! Using unsafe to bypass these constraints (or sharing handles concurrently) is undefined behavior.
//!
//! # Semantics
//! - Sequence numbers are monotonically increasing `u32` values; `0` is reserved for "empty".
//! - `poll_one`/`poll_up_to` drain in-order and return `PollStats`.
//! - `latest` reads the newest value without advancing the consumer cursor.
//! - If the consumer lags by more than `N`, it skips ahead and reports drops via `PollStats`.
#![no_std]

#[cfg(all(not(target_has_atomic = "32"), not(feature = "portable-atomic")))]
compile_error!(
    "ph-eventing requires 32-bit atomics. For thumbv6m and other no-atomic targets, \
enable either the portable-atomic-unsafe-assume-single-core or portable-atomic-critical-section feature."
);

pub mod seq_ring;

pub use seq_ring::{Consumer, PollStats, Producer, SeqRing};

#[cfg(test)]
extern crate std;
