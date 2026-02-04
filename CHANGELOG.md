# Changelog

All notable changes to this project will be documented in this file.

## 0.1.1 - 2026-02-04
### Added
- Optional `portable-atomic` support for targets without 32-bit atomics (e.g. `thumbv6m-none-eabi`).
- `Default` implementation for `SeqRing<T, N>` (equivalent to `SeqRing::new()`).

### Changed
- CI now uses `dtolnay/rust-toolchain` and pins clippy to the MSRV toolchain.
- Embedded CI checks include `thumbv6m-none-eabi` with the portable-atomic feature enabled.
