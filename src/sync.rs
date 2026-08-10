//! Synchronisation primitives, swappable for Loom's instrumented versions.
//!
//! Under `--cfg loom` this re-exports Loom's atomics and cell, so the model
//! checker can explore every legal interleaving and every value a relaxed load
//! is permitted to return. Otherwise it re-exports the real primitives —
//! `core::sync::atomic` where the target has 32-bit atomics, `portable-atomic`
//! where it does not.
//!
//! Nothing here changes the shipped library: the Loom branch is compile-gated
//! behind a cfg that only the model-checking script sets, and `loom` is a
//! dev-dependency of that cfg alone.
//!
//! # Why [`TrackedCell`]
//! Loom's `UnsafeCell` requires closure-based access so it can detect
//! concurrent access to the same slot. Its API differs from `core`'s, so
//! [`TrackedCell`] presents one `with`/`with_mut` interface over both.
//!
//! [`crate::EventBuf`] uses it: its producer and consumer never touch the same
//! slot, so Loom can verify that claim. [`crate::SeqRing`] deliberately does
//! not — it is a seqlock, its slot access is racy by construction (see the
//! `seq_ring` module docs), and wrapping its slots in a tracked cell would only
//! re-report a race that is already known and documented. Its *sequence
//! protocol* is what Loom checks, and that is built entirely from the atomics
//! below.

#[cfg(loom)]
pub(crate) use loom::sync::atomic::{AtomicBool, AtomicU8, AtomicU32, fence};

#[cfg(all(not(loom), target_has_atomic = "32"))]
pub(crate) use core::sync::atomic::{AtomicBool, AtomicU8, AtomicU32, fence};

#[cfg(all(not(loom), not(target_has_atomic = "32"), feature = "portable-atomic"))]
pub(crate) use portable_atomic::{AtomicBool, AtomicU8, AtomicU32, fence};

pub(crate) use core::sync::atomic::Ordering;

/// An `UnsafeCell` that Loom can instrument.
///
/// Under `--cfg loom` this is Loom's cell, which flags concurrent access to
/// the same slot. Otherwise it is a zero-cost wrapper over `core`'s.
#[cfg(loom)]
#[derive(Debug)]
pub(crate) struct TrackedCell<T>(loom::cell::UnsafeCell<T>);

#[cfg(not(loom))]
#[derive(Debug)]
pub(crate) struct TrackedCell<T>(core::cell::UnsafeCell<T>);

impl<T> TrackedCell<T> {
    /// Construct a cell. Const on the host path; Loom's cell is not
    /// const-constructible, so the Loom build keeps a non-const `new`.
    #[cfg(not(loom))]
    #[inline(always)]
    pub(crate) const fn new(value: T) -> Self {
        Self(core::cell::UnsafeCell::new(value))
    }

    #[cfg(loom)]
    #[inline(always)]
    pub(crate) fn new(value: T) -> Self {
        Self(loom::cell::UnsafeCell::new(value))
    }

    /// Access the contents through a shared pointer.
    #[inline(always)]
    pub(crate) fn with<R>(&self, f: impl FnOnce(*const T) -> R) -> R {
        #[cfg(loom)]
        {
            self.0.with(f)
        }
        #[cfg(not(loom))]
        {
            f(self.0.get())
        }
    }

    /// Access the contents through a mutable pointer.
    #[inline(always)]
    pub(crate) fn with_mut<R>(&self, f: impl FnOnce(*mut T) -> R) -> R {
        #[cfg(loom)]
        {
            self.0.with_mut(f)
        }
        #[cfg(not(loom))]
        {
            f(self.0.get())
        }
    }
}
