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
#![no_std]

pub mod seq_ring;

pub use seq_ring::{Consumer, Producer, SeqRing};

#[cfg(test)]
extern crate std;
