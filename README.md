# ph-eventing

[![Crates.io](https://img.shields.io/crates/v/ph-eventing)](https://crates.io/crates/ph-eventing)
[![docs.rs](https://img.shields.io/docsrs/ph-eventing)](https://docs.rs/ph-eventing)
[![CI: local](https://img.shields.io/badge/CI-local-blue)](scripts/ci.sh)
[![License: MIT](https://img.shields.io/crates/l/ph-eventing)](LICENSE)
[![MSRV](https://img.shields.io/badge/MSRV-1.92.0-blue)](rust-toolchain.toml)
[![no_std](https://img.shields.io/badge/no__std-yes-green)](src/lib.rs)

Deterministic zero-allocation handoff primitives for no-std embedded targets.

## What's in the box

| Type | Use case |
|------|----------|
| [`RingBuf<T, N>`](#ringbuf) | Single-owner ring buffer — simple, no atomics, `&mut` access. |
| [`SeqRing<T, N>`](#seqring) | Lock-free SPSC ring that **overwrites** old entries (lossy, high-throughput). |
| [`EventBuf<T, N>`](#eventbuf) | Lock-free SPSC ring with **backpressure** — rejects pushes when full. |
| [`EventFlags`](#eventflags) | Coalesced SPSC condition set — 32 payload-free conditions, one atomic hot-path operation. |

All four are fixed-size, `#![no_std]`, and zero-allocation. The three buffers
are generic over `T: Copy`; `EventFlags` carries an `EventMask(u32)`.

## What this optimises for

`no_std` and no-alloc are the entry fee. What this crate offers past that is
**behaviour you can predict and cost you can measure**:

- **Predictability first.** No unbounded loops, no hidden allocation, and no
  panic reachable from a hot path. For the concurrent types, no data loss that
  cannot be observed either — every drop is reported (`SeqRing`) or prevented
  (`EventBuf`), or explicitly coalesced by contract (`EventFlags`). `RingBuf` is the deliberate exception: it is a single-owner
  window that overwrites silently, with no drop counter and no backpressure.
  Reach for it when losing the oldest entry is the point, not when delivery
  matters.
- **Cost measured on every target, not one.** `scripts/codesize.sh` (added
  alongside this release) reports the
  flash cost of each API shape across 11 targets and 4 ISA families, because a
  design that wins on Cortex-M4 can cost 40–60% more on Cortex-M0+ or ESP32-S2,
  where every atomic becomes an interrupt-disable critical section.
- **Guarantees pinned by tooling.** Loom proves the orderings exhaustively at
  the modelled size, Miri checks UB on 32-bit and big-endian, and a code-size
  row keeps a cheap API from quietly becoming expensive.

**Ergonomics is ranked last, deliberately.** If an API here feels more awkward
than an equivalent `std` type, that is usually a cost being made visible rather
than hidden. Where the awkwardness is not load-bearing, the fix is compile-time
tooling that costs nothing at runtime — not a friendlier API that allocates,
panics, or hides a cost.

## Features
- Three ring buffer flavours: single-owner, lossy SPSC, and backpressure SPSC.
- `EventFlags` for coalesced ISR-to-task condition notification.
- Common `Sink`/`Source`/`Link` traits for writing generic event-processing code.
- `forward(src, snk, max)` utility to bridge any `Source` → `Sink`.
- No heap, no dynamic dispatch, no required dependencies.
- Optional `portable-atomic` support for targets without native 32-bit atomics.
- Designed for `#![no_std]` environments (std only for tests).

## Compatibility
- MSRV: Rust 1.92.0.
- `SeqRing::new()` and `EventBuf::new()` assert `N > 0`.
- `SeqRing`, `EventBuf`, and `EventFlags` require 32-bit atomics by default.
- For `thumbv6m-none-eabi` (and other no-atomic targets), enable one of:
  - `portable-atomic-unsafe-assume-single-core`
  - `portable-atomic-critical-section` (requires a critical-section implementation in the binary)
- Those two are **mutually exclusive** — they select different portable-atomic backends, and
  enabling both fails inside portable-atomic. Cargo features are additive, so this cannot be
  expressed in the manifest; `build.rs` detects the combination and explains it.
- Consequently **`--all-features` does not work for this crate** and cannot be made to. Check
  combinations individually; `scripts/ci.sh` enumerates the supported set.

## Usage

### RingBuf

A straightforward, single-owner ring buffer for collecting values when you
don't need cross-thread access. When full, new pushes silently overwrite the
oldest entry. Requires only `T: Copy` — no `Default`.

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
let producer = ring.try_producer().expect("no producer taken yet");
let mut consumer = ring.try_consumer().expect("no consumer taken yet");

producer.push(123);
assert_eq!(consumer.poll_one_value(), Some((1, 123)));
// hook form still available:
// consumer.poll_one(|seq, v| { ... });
```

### EventBuf

A bounded SPSC queue with backpressure. When the buffer is full, `push`
returns `Err(val)` so the producer can decide what to do — no data is
silently lost.

```rust
use ph_eventing::EventBuf;

let buf = EventBuf::<u32, 2>::new();
let producer = buf.try_producer().expect("no producer taken yet");
let consumer = buf.try_consumer().expect("no consumer taken yet");

assert!(producer.push(1).is_ok());
assert!(producer.push(2).is_ok());
assert_eq!(producer.push(3), Err(3)); // full — value returned

assert_eq!(consumer.peek(), Some(1));  // copy, no advance
assert_eq!(consumer.pop(), Some(1));
assert!(producer.push(3).is_ok());     // space freed
```

### EventFlags

A coalesced condition set for ISR-to-task notification. Repeated raises of one
condition may merge; a take returns and clears every condition that occurred at
least once since the preceding take.

```rust
use ph_eventing::{EventFlags, EventMask};

const DATA_READY: EventMask = EventMask::from_bits(1 << 0);
const OVERFLOW: EventMask = EventMask::from_bits(1 << 1);

let flags = EventFlags::new();
let producer = flags.try_producer().expect("no producer taken yet");
let consumer = flags.try_consumer().expect("no consumer taken yet");

producer.raise(DATA_READY);
producer.raise(DATA_READY); // coalesces
producer.raise(OVERFLOW);

assert_eq!(consumer.take_all(), DATA_READY | OVERFLOW);
assert!(consumer.take_all().is_empty());
```

`EventFlags` deliberately does not implement the stream traits below: a
coalesced condition set is not a sequence of items, and destructive take plus
a rejecting downstream sink could silently lose the mask.

### Common Traits

All producers implement `Sink<T>` and all consumers implement `Source<T>`,
so you can write generic code that works with any combination:

```rust
use ph_eventing::{SeqRing, EventBuf};
use ph_eventing::traits::{Source, Sink, forward};

// bridge a SeqRing producer → EventBuf consumer
let seq = SeqRing::<u32, 8>::new();
let sp = seq.try_producer().expect("no producer taken yet");
let mut sc = seq.try_consumer().expect("no consumer taken yet");

sp.push(1); sp.push(2);

let eb = EventBuf::<u32, 8>::new();
let mut ep = eb.try_producer().expect("no producer taken yet");

let (n, err) = forward(&mut sc, &mut ep, 10);
assert_eq!(n, 2);
assert!(err.is_none());
```

| Trait | Role | Implementors |
|-------|------|--------------|
| `Sink<T>` | Accept events | `RingBuf`, `seq_ring::Producer`, `event_buf::Producer` |
| `Source<T>` | Yield events | `seq_ring::Consumer`, `event_buf::Consumer` |
| `Link<In,Out>` | Both | Blanket impl for `Sink<In> + Source<Out>` |

### Declarative static bring-up

`static_spsc!` names the handle types for you, so a signature does not have to
spell out `ph_eventing::event_buf::Producer<'static, u32, 64>`:

```rust
ph_eventing::static_spsc! {
    pub mod telemetry: EventBuf<u32, 64>;
}

fn on_sample(tx: &telemetry::Tx, v: u32) { let _ = tx.push(v); }

let (tx, rx) = telemetry::take().expect("first take");
```

It expands to a `static`, two type aliases, and an all-or-nothing `take()` —
no allocation, no indirection, and no instruction that would not be there
written by hand. `SeqRing` works the same way. Note the handles are
`Send + !Sync` by design, so they cannot themselves live in a `static`; taking
them is a runtime step and always will be.

## Semantics

### RingBuf
- Single-owner (`&mut self` to push).
- `get(i)` returns the `i`-th element where `0` is the oldest.
- `latest()` returns the most recently pushed element.
- `iter()` yields elements oldest → newest.
- `new()` is a `const fn`; `N == 0` fails at compile time.

### SeqRing
- Sequence numbers are monotonically increasing `u32` values; `0` is reserved for "empty".
- When the producer wraps the ring, old values are overwritten.
- `poll_one` and `poll_up_to` drain in-order and return `PollStats` (`read`, `dropped`, `newest`).
- `poll_one_value` / `latest_value` return `(seq, T)` without a hook.
- `latest` reads the newest value without advancing the consumer cursor.
- `skip_to_latest` discards the backlog so the next poll returns the newest item.
- If the consumer lags by more than `N`, it skips ahead and reports drops via `PollStats`.
- Once every `2^32 - 1` pushes the sequence counter wraps and a few extra entries are dropped —
  exactly one for a power-of-two `N`, none if `N` divides `2^32 - 1`, up to `N - 1` otherwise. They
  are reported as ordinary drops; no stale or torn value is ever returned. See
  [Choosing `N`](#choosing-n).

### EventBuf
- FIFO order: `pop` always returns the oldest item.
- `peek` copies the oldest item without advancing the cursor.
- `push` returns `Ok(())` on success or `Err(val)` when the buffer is full.
- `drain(max, hook)` consumes up to `max` items through a callback and returns the count.
- No data is silently lost — the producer always knows when the buffer cannot accept more.

### EventFlags
- `raise(mask)` unions conditions into the pending set; duplicate bits may coalesce.
- `take_all()` atomically returns and clears every pending condition.
- Conditions are unordered and carry no payload or multiplicity.
- `EventMask` is exactly 32 bits; `from_index` rejects out-of-range indices without panicking.
- A take that observes a raise also observes memory writes sequenced before it.
- There is no non-clearing peek and no stream/signal trait implementation in the initial surface.

## Safety and Concurrency
- `RingBuf` has no atomics and no interior mutability — standard Rust borrow rules apply. It stores slots as `MaybeUninit<T>` and reads only live entries, so it does contain `unsafe`.
- `SeqRing`, `EventBuf`, and `EventFlags` are SPSC by design: exactly one producer and one consumer may be
  active. Use `try_producer()`/`try_consumer()`, which return `None` rather than panicking —
  on a microcontroller a panic is a reset, and the panic machinery costs flash you may not have.
  The panicking `producer()`/`consumer()` are **deprecated since 0.2.0** and will be removed in
  0.3.0. Using unsafe to bypass the SPSC constraint (or sharing handles concurrently) is
  undefined behavior.
- `T: Copy` is required by the three buffer types to avoid allocation and return values by copy.
- `EventFlags` has no unsafe slot access and passes Miri with the race detector enabled.
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

## Using it across contexts

The typical embedded shape is a producer in an interrupt handler and a consumer
in a task loop. That works, with three things to know:

- **The container is shared; the handles are owned.** `SeqRing<T, N>` and
  `EventBuf<T, N>` are `Sync` when `T: Send`, and `EventFlags` is `Sync`, so a
  shared reference can be handed to both
  contexts. `Producer` and `Consumer` are `Send + !Sync` — move each one into
  the context that owns it, and never share a single handle between contexts.
  There is no way to get a second `Producer` while one is live:
  `try_producer` / `try_consumer` return `None` rather than handing out a
  duplicate.
- **The buffer must outlive both handles.** The handles borrow it, so the usual
  answer is to own the buffer where it lives longest.
- **`new()` is a `const fn`** on the normal build, so
  `static BUF: EventBuf<u32, 64> = EventBuf::new();` works. (Under `--cfg loom`
  it is non-const because Loom's atomics are not const-constructible.) Handles
  still borrow the buffer, so an ISR / task split typically pairs the `static`
  with a `StaticCell` or similar for the handles themselves.

### Choosing `N`

`N` is the slot count, fixed at compile time, and the whole buffer lives inline
— `N * size_of::<T>()` bytes of stack or static, with no allocation.

- For `EventBuf`, `N` is your backpressure threshold: the point at which `push`
  starts returning `Err`. Size it for the largest burst you are willing to
  absorb between drains.
- For `SeqRing`, `N` is how far the consumer may lag before it starts losing
  entries. Size it for the worst-case gap between polls, not for the average.
- `N` need not be a power of two — no indexing or capacity logic requires it — but for `SeqRing`
  a power of two is still the better default. `SeqRing` addresses slots by `(seq - 1) % N` while
  `push` skips the reserved sequence `0`, so a full cycle is `2^32 - 1` sequences and the slot walk
  only lines up across the wrap when `N` divides `2^32 - 1`. What that costs, once per wrap:

  | `N` | Entries dropped at the wrap |
  |-----|-----------------------------|
  | A power of two | Exactly 1 |
  | A divisor of `2^32 - 1` (3, 5, 15, 17, 51, 85, 255, 257, 65537, …) | 0 |
  | Anything else | Up to `N - 1` — `N = 48` drops 15, `N = 96` drops 33, `N = 121` drops 58 |

  These are reported through `PollStats` like any other drop, and no stale or torn value is ever
  returned — it is a data-loss bound, not a correctness one. One lost entry per `2^32` pushes is
  beneath the noise floor for anything that already tolerates overwrite, so a power of two is
  almost always the right call. `EventBuf` has no wrap boundary of this kind.

## Quality and verification

The concurrent primitives are atomic, so a green test run on x86 is weak
evidence — a strongly-ordered host cannot exhibit the ordering bugs that appear
on ARM and RISC-V. What backs this crate, in descending order of strength:

| Evidence | What it establishes |
|----------|---------------------|
| [Loom](https://github.com/tokio-rs/loom) models | Exhaustive: every interleaving and every legal relaxed-load value, for the modelled size |
| [Miri](https://github.com/rust-lang/miri) | UB, data races, and weak-memory behaviour; also run on 32-bit and big-endian targets |
| 77 unit + 11 doctests + 5 compile-fail | Behaviour, including threaded stress for all concurrent types; `N == 0` and EventFlags handle `!Sync` contracts rejected at compile time |
| 3 embedded targets | `thumbv6m` / `thumbv7em` / `riscv32imac` compile checks |
| Code-size baseline | Flash cost gated in CI across 8 pinned targets; growth past +16 bytes fails |
| QEMU instruction counts | Hot-path cost is constant w.r.t. occupancy, measured per instruction |

All of it is reproducible: `./scripts/verify.sh` runs the full matrix inside one
pinned Docker environment (`scripts/verify/Dockerfile`), so the numbers above
can be checked rather than believed.

**One known deviation.** `SeqRing` is a seqlock and carries a formal data race —
see [Safety and Concurrency](#safety-and-concurrency) above. `EventBuf` is
race-free by construction and passes Miri with the detector enabled.

Coverage is around 94% of lines, though it is a weak signal here: what matters
is ordering and interleaving, which line coverage cannot see.

Contributors: [CONTRIBUTING.md](CONTRIBUTING.md) has the commands for running
all of the above locally. CI runs on every PR, but it covers only part of that
list — **coverage, Miri, and Loom are local-only**, so a green check is not a
clean matrix.

## License
MIT. See `LICENSE`.
