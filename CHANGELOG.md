# Changelog

All notable changes to this project will be documented in this file.

## 0.1.2 - 2026-02-08
### Added
- `Sink<T>` trait for event producers, implemented by `RingBuf`, `seq_ring::Producer`, `event_buf::Producer`.
- `Source<T>` trait for event consumers, implemented by `seq_ring::Consumer`, `event_buf::Consumer`.
- `Link<In, Out>` blanket trait for types that are both `Sink` and `Source`.
- `forward(src, snk, max)` utility to bridge any `Source` into any `Sink`.
- `EventBuf<T, N>` — bounded SPSC event buffer with backpressure (`push` returns `Err` when full).
- `event_buf::Producer` and `event_buf::Consumer` handles.
- `RingBuf<T, N>` — single-owner, stack-allocated ring buffer (no atomics, no unsafe).
- `RingIter` iterator with `ExactSizeIterator` support.
- `Default` implementation for `RingBuf` and `EventBuf`.
- `Debug` implementations for all public types (`RingBuf`, `RingIter`, `SeqRing`, `EventBuf`, all `Producer`/`Consumer` handles).
- `IntoIterator` implementation for `&RingBuf`.
- `capacity()` method on `RingBuf`, `SeqRing`, and `EventBuf`.
- Re-export `RingBuf`, `EventBuf`, `Sink`, `Source`, `Link` from crate root.

### Fixed
- `event_buf::Producer` and `event_buf::Consumer` are now `Send` (was accidentally `!Send` due to `PhantomData<*mut ()>`).
- `RingBuf::new()` now panics on `N == 0` with a clear message (matches `SeqRing` and `EventBuf` behaviour).

### Changed
- Removed `Producer` and `Consumer` re-exports from crate root to avoid ambiguity between `seq_ring` and `event_buf` types. Access them via `seq_ring::Producer` / `event_buf::Producer` etc.

## 0.1.1 - 2026-02-04
### Added
- Optional `portable-atomic` support for targets without 32-bit atomics (e.g. `thumbv6m-none-eabi`).
- `Default` implementation for `SeqRing<T, N>` (equivalent to `SeqRing::new()`).

### Changed
- CI now uses `dtolnay/rust-toolchain` and pins clippy to the MSRV toolchain.
- Embedded CI checks include `thumbv6m-none-eabi` with the portable-atomic feature enabled.
