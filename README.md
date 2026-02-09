# ph-eventing

[![Crates.io](https://img.shields.io/crates/v/ph-eventing)](https://crates.io/crates/ph-eventing)
[![docs.rs](https://img.shields.io/docsrs/ph-eventing)](https://docs.rs/ph-eventing)
[![CI](https://github.com/photon-circus/ph-eventing/actions/workflows/ci.yml/badge.svg)](https://github.com/photon-circus/ph-eventing/actions/workflows/ci.yml) 
[![License: MIT](https://img.shields.io/crates/l/ph-eventing)](LICENSE)
[![MSRV](https://img.shields.io/badge/MSRV-1.92.0-blue)](rust-toolchain.toml)
[![no_std](https://img.shields.io/badge/no__std-yes-green)](src/lib.rs)

Stack-allocated ring buffers for no-std embedded targets.

## What's in the box

| Type | Use case |
|------|----------|
| [`RingBuf<T, N>`](#ringbuf) | Single-owner ring buffer — simple, no atomics, `&mut` access. |
| [`SeqRing<T, N>`](#seqring) | Lock-free SPSC ring that **overwrites** old entries (lossy, high-throughput). |
| [`EventBuf<T, N>`](#eventbuf) | Lock-free SPSC ring with **backpressure** — rejects pushes when full. |

All three are fixed-size, `#![no_std]`, zero-allocation, and generic over `T: Copy`.

## Features
- Three ring buffer flavours: single-owner, lossy SPSC, and backpressure SPSC.
- Common `Sink`/`Source`/`Link` traits for writing generic event-processing code.
- `forward(src, snk, max)` utility to bridge any `Source` → `Sink`.
- No heap, no dynamic dispatch, no required dependencies.
- Optional `portable-atomic` support for targets without native 32-bit atomics.
- Designed for `#![no_std]` environments (std only for tests).

## Compatibility
- MSRV: Rust 1.92.0.
- `SeqRing::new()` and `EventBuf::new()` assert `N > 0`.
- `SeqRing` and `EventBuf` require 32-bit atomics by default.
- For `thumbv6m-none-eabi` (and other no-atomic targets), enable one of:
  - `portable-atomic-unsafe-assume-single-core`
  - `portable-atomic-critical-section` (requires a critical-section implementation in the binary)

## Usage

### RingBuf

A straightforward, single-owner ring buffer for collecting values when you
don't need cross-thread access. When full, new pushes silently overwrite the
oldest entry.

```rust
use ph_eventing::RingBuf;

let mut ring = RingBuf::<u32, 4>::new();
ring.push(1);
ring.push(2);
ring.push(3);
assert_eq!(ring.latest(), Some(3));
assert_eq!(ring.get(0),  Some(1)); // oldest

// iterate oldest → newest
for val in ring.iter() {
    // 1, 2, 3
}
```

### SeqRing

A lock-free SPSC ring for high-rate telemetry. The producer never blocks;
the consumer reports drops when it lags behind by more than `N`.

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

### EventBuf

A bounded SPSC queue with backpressure. When the buffer is full, `push`
returns `Err(val)` so the producer can decide what to do — no data is
silently lost.

```rust
use ph_eventing::EventBuf;

let buf = EventBuf::<u32, 2>::new();
let producer = buf.producer();
let consumer = buf.consumer();

assert!(producer.push(1).is_ok());
assert!(producer.push(2).is_ok());
assert_eq!(producer.push(3), Err(3)); // full — value returned

assert_eq!(consumer.pop(), Some(1));
assert!(producer.push(3).is_ok());     // space freed
```

### Common Traits

All producers implement `Sink<T>` and all consumers implement `Source<T>`,
so you can write generic code that works with any combination:

```rust
use ph_eventing::{SeqRing, EventBuf};
use ph_eventing::traits::{Source, Sink, forward};

// bridge a SeqRing producer → EventBuf consumer
let seq = SeqRing::<u32, 8>::new();
let sp = seq.producer();
let mut sc = seq.consumer();

sp.push(1); sp.push(2);

let eb = EventBuf::<u32, 8>::new();
let mut ep = eb.producer();

let (n, err) = forward(&mut sc, &mut ep, 10);
assert_eq!(n, 2);
assert!(err.is_none());
```

| Trait | Role | Implementors |
|-------|------|--------------|
| `Sink<T>` | Accept events | `RingBuf`, `seq_ring::Producer`, `event_buf::Producer` |
| `Source<T>` | Yield events | `seq_ring::Consumer`, `event_buf::Consumer` |
| `Link<In,Out>` | Both | Blanket impl for `Sink<In> + Source<Out>` |

## Semantics

### RingBuf
- Single-owner (`&mut self` to push).
- `get(i)` returns the `i`-th element where `0` is the oldest.
- `latest()` returns the most recently pushed element.
- `iter()` yields elements oldest → newest.

### SeqRing
- Sequence numbers are monotonically increasing `u32` values; `0` is reserved for "empty".
- When the producer wraps the ring, old values are overwritten.
- `poll_one` and `poll_up_to` drain in-order and return `PollStats` (`read`, `dropped`, `newest`).
- `latest` reads the newest value without advancing the consumer cursor.
- If the consumer lags by more than `N`, it skips ahead and reports drops via `PollStats`.

### EventBuf
- FIFO order: `pop` always returns the oldest item.
- `push` returns `Ok(())` on success or `Err(val)` when the buffer is full.
- `drain(max, hook)` consumes up to `max` items through a callback and returns the count.
- No data is silently lost — the producer always knows when the buffer cannot accept more.

## Safety and Concurrency
- `RingBuf` is a plain struct with no interior mutability — standard Rust borrow rules apply.
- `SeqRing` and `EventBuf` are SPSC by design: exactly one producer and one consumer may be
  active. `producer()`/`consumer()` will panic if called while another handle of the same kind
  is active. Using unsafe to bypass these constraints (or sharing handles concurrently) is
  undefined behavior.
- `T: Copy` is required by all types to avoid allocation and return values by copy.

## Testing

46 unit tests and 7 doctests covering all three buffer types plus the trait
system. Host tests require `std`:

```
cargo test
```

| Module | Tests |
|--------|------:|
| `event_buf` | 12 |
| `ring` | 10 |
| `seq_ring` | 13 |
| `traits` | 11 |
| doctests | 7 |
| **Total** | **53** |

Coverage snapshot (2026-02-08, via `cargo llvm-cov`):

| Metric | Covered | Total | % |
|--------|--------:|------:|--:|
| Lines | 784 | 857 | 91.5 |
| Functions | 115 | 130 | 88.5 |
| Regions | 1392 | 1490 | 93.4 |
| Instantiations | 220 | 237 | 92.8 |

To regenerate:
```
cargo llvm-cov --json --summary-only --output-path target/llvm-cov/summary.json
```

## License
MIT. See `LICENSE`.
