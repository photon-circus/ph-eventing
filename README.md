# ph-eventing

[![codecov](https://codecov.io/github/photon-circus/ph-eventing/graph/badge.svg?token=9SA836QO2K)](https://codecov.io/github/photon-circus/ph-eventing)

Lock-free SPSC event ring for high-throughput telemetry on no-std embedded targets.

## Features
- Lock-free single-producer single-consumer sequence ring.
- Producer never blocks; consumer can be slow and will drop old items when it lags.
- No allocation, no dynamic dispatch, no required dependencies (optional `portable-atomic` for no-atomic targets).
- Designed for `#![no_std]` environments (std only for tests).

## Compatibility
- MSRV: Rust 1.92.0.
- `SeqRing::new()` asserts `N > 0`.
- Requires 32-bit atomics by default.
- For `thumbv6m-none-eabi` (and other no-atomic targets), enable one of:
  - `portable-atomic-unsafe-assume-single-core`
  - `portable-atomic-critical-section` (requires a critical-section implementation in the binary)

## Usage
```rust
use ph_eventing::SeqRing;

let ring = SeqRing::<u32, 64>::new();
let producer = ring.producer();
let mut consumer = ring.consumer();

producer.push(123);
consumer.poll_one(|seq, v| {
    assert_eq!(seq, 1);
    assert_eq!(*v, 123);
});
```

## Semantics
- Sequence numbers are monotonically increasing `u32` values; `0` is reserved for "empty".
- When the producer wraps the ring, old values are overwritten.
- `poll_one` and `poll_up_to` drain in-order and return `PollStats` (`read`, `dropped`, `newest`).
- `latest` reads the newest value without advancing the consumer cursor.
- If the consumer lags by more than `N`, it skips ahead and reports drops via `PollStats`.

## Safety and Concurrency
- This crate is SPSC by design: exactly one producer and one consumer must be active.
- `producer()`/`consumer()` will panic if called while another handle of the same kind is active.
- Using unsafe to bypass these constraints (or sharing handles concurrently) is undefined behavior.
- `T: Copy` is required to avoid allocation and return values by copy.

## Testing
Host tests require `std` and can be run with:
```
cargo test
```

Coverage snapshot (2026-02-04, via `cargo llvm-cov`):
- Lines: 279/302 (92.38%)
- Functions: 39/44 (88.64%)
- Regions: 467/483 (96.69%)
- Instantiations: 79/85 (92.94%)

To regenerate:
```
cargo llvm-cov --json --summary-only --output-path target/llvm-cov/summary.json
```

## License
MIT. See `LICENSE`.
