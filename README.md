# ph-eventing

[![Crates.io](https://img.shields.io/crates/v/ph-eventing)](https://crates.io/crates/ph-eventing)
[![docs.rs](https://img.shields.io/docsrs/ph-eventing)](https://docs.rs/ph-eventing)
[![CI: local](https://img.shields.io/badge/CI-local-blue)](scripts/ci.sh)
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
- Those two are **mutually exclusive** — they select different portable-atomic backends, and
  enabling both fails inside portable-atomic. Cargo features are additive, so this cannot be
  expressed in the manifest; `build.rs` detects the combination and explains it.
- Consequently **`--all-features` does not work for this crate** and cannot be made to. Check
  combinations individually; `scripts/ci.*` enumerates the supported set.

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
- `EventBuf` is race-free by construction: its producer and consumer never touch the same slot,
  and it passes Miri with the data-race detector enabled.
- `SeqRing` is a seqlock and carries a **known formal data race** — the consumer may copy a slot
  the producer is overwriting, then discard the copy when the sequence re-check fails. The copy is
  never returned and never becomes an invalid value, but the access is undefined behaviour by the
  letter of the memory model.
  - **This affects your tooling, not just ours:** if you run `cargo miri test` over a test that
    drives `SeqRing` from two threads, Miri will report UB pointing into this crate. That is the
    known deviation, not a new bug.
  - It is a deliberate trade. A ring restricted to a word-sized payload could store it in an
    atomic and be fully race-free; accepting any `T: Copy` is what rules that out. Generality was
    chosen over formal soundness.
  - `EventBuf` has no such caveat and passes Miri with the detector on — but it is **not a
    drop-in**, since it applies backpressure instead of overwriting.
  - Full reasoning, including the alternatives and why each was rejected, is in the `seq_ring`
    module docs.

## Testing

54 unit tests and 7 doctests covering all three buffer types plus the trait
system. Host tests require `std`:

```
cargo test
```

| Module | Tests |
|--------|------:|
| `event_buf` | 14 |
| `ring` | 10 |
| `seq_ring` | 19 |
| `traits` | 11 |
| doctests | 7 |
| **Total** | **61** |

### Concurrency checking

`SeqRing` and `EventBuf` are lock-free, so a green `cargo test` on x86 proves
very little — a strongly-ordered host hides exactly the ordering bugs that
matter on ARM and RISC-V. `scripts/miri.ps1` / `scripts/miri.sh` run the suite
under [Miri](https://github.com/rust-lang/miri), which interprets MIR against
the C++20 weak-memory model:

```
./scripts/miri.sh
```

It makes three host passes — full checking, the `SeqRing` overwrite test with
the data-race detector off (see below), and a multi-seed scheduler sweep —
then repeats the full pass on 32-bit and big-endian targets.

Miri needs `std` for the test harness, so the bare-metal targets cannot be run
directly. `i686-unknown-linux-gnu` and `armv7-unknown-linux-gnueabihf` stand in
for them: same 32-bit pointer width and `usize` range, which is what the
sequence and cursor arithmetic actually depends on. `s390x-unknown-linux-gnu`
covers big-endian.

**Known deviation:** `SeqRing` is a seqlock, and seqlocks are formally racy —
the consumer may copy a slot while the producer overwrites it, then discard the
copy after the sequence re-check fails. Miri reports that copy as UB, correctly;
this cannot be fixed in stable Rust for an arbitrary `T: Copy`. See the
`seq_ring` module docs for the full reasoning. `EventBuf` has no such deviation
and passes Miri with the race detector on.

### Model checking

Miri samples schedules; [Loom](https://github.com/tokio-rs/loom) is exhaustive.
It enumerates every legal execution of a model — every interleaving, and every
value each relaxed load may return under the C11 memory model — so a clean run
proves the absence of ordering bugs at the modelled size:

```
./scripts/loom.sh
```

Five models in `src/loom_tests.rs` cover `EventBuf` (lossless FIFO delivery,
the `len` capacity bound, and never reading unpublished data) and the `SeqRing`
sequence protocol (the consumer never outruns the producer; `latest` never
reports an unpublished sequence). They use capacity-2 buffers and two or three
operations per thread, because the state space grows exponentially and bugs
that need four items are vanishingly rare.

**Loom does not affect the shipped crate.** It is a dev-dependency gated on
`--cfg loom`, which only the script sets, so `cargo build`, `cargo test`, and
`cargo package` never resolve it — `cargo tree` still shows zero dependencies.

### Local CI

This project runs its CI locally. `scripts/ci.ps1` (Windows) and
`scripts/ci.sh` (POSIX) run the full matrix — formatting, clippy, host tests,
docs, and the `thumbv6m` / `thumbv7em` / `riscv32imac` cross-compilation
checks:

```
./scripts/ci.sh
```

Every check runs even if an earlier one fails, and a summary is printed at the
end. Pass `-SkipEmbedded` / `SKIP_EMBEDDED=1` to skip the cross-compilation
checks. The GitHub Actions workflow mirrors the same jobs but is
manual-dispatch only.

### Supply chain

[cargo-deny](https://github.com/EmbarkStudios/cargo-deny) enforces the
zero-dependency stance rather than leaving it to good intentions — it fails on
a new dependency's licence, a security advisory, a yanked crate, a duplicate
version, a wildcard requirement, or a non-crates.io source:

```
cargo install cargo-deny
cargo deny check
```

The policy is in [deny.toml](deny.toml), and the local CI runner includes it
when the tool is installed (and reports it as skipped when it is not). The
licence allow-list is deliberately just `MIT` and `Apache-2.0`; anything else
requires a deliberate edit.

### Coverage

Local CI enforces a **90% line floor** via `cargo llvm-cov`, so this is a gate
rather than a number nobody reads:

```
cargo install cargo-llvm-cov
cargo llvm-cov --summary-only
```

Measured over the 54 unit tests with `cargo llvm-cov 0.8.7` at commit
`45aaeb9` — approximately **93% lines, 94% regions, 91% functions**. Doctests
are not instrumented by default.

The figures are given as approximate on purpose. Three consecutive runs on the
same commit produced 93.18%, 93.46%, and 93.18% lines: the threaded stress
tests take different paths each run, so coverage is not deterministic here. An
exact table would imply a precision the measurement does not have. The 90%
floor sits comfortably below that spread.

Read the number with the right expectations. Coverage cannot see the properties
that actually matter for this crate — ordering and interleaving — so it is a
regression alarm for untested branches, not evidence of concurrency
correctness. The Miri and Loom runs above are that evidence. Some uncovered
lines are deliberately hard to reach single-threaded: the `EventBuf::len` clamp
fallback, for instance, needs the consumer to move during every retry.

## License
MIT. See `LICENSE`.
