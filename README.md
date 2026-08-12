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
| [`Block<T, N>` / `BlockBuilder<T, N>`](#complete-blocks) | Build complete contiguous sample windows, then compose them with a transport. |
| [`RingBuf<T, N>`](#ringbuf) | Single-owner ring buffer — simple, no atomics, `&mut` access. |
| [`SeqRing<T, N>`](#seqring) | Lock-free SPSC ring that **overwrites** old entries (lossy, high-throughput). |
| [`EventBuf<T, N>`](#eventbuf) | Lock-free SPSC ring with **backpressure** — rejects pushes when full. |
| [`CountedSignal`](#countedsignal) | Saturating SPSC count for identical, payload-free events. |
| [`LatestBuf<T>`](#latestbuf) | Freshness-first SPSC snapshot — retains one newest unread value. |

All types are fixed-size, `#![no_std]`, and zero-allocation. The buffers are
generic over `T: Copy`; `CountedSignal` carries no payload.

## What this optimises for

`no_std` and no-alloc are the entry fee. What this crate offers past that is
**behaviour you can predict and cost you can measure**:

- **Predictability first.** No unbounded loops, no hidden allocation, and no
  panic reachable from a hot path. For the two SPSC types, no data loss that
  cannot be observed either — every drop is reported (`SeqRing`) or prevented
  (`EventBuf`). `RingBuf` is the deliberate exception: it is a single-owner
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
- Three ring buffer flavours plus a freshness-first SPSC snapshot channel.
- Complete contiguous sample blocks with an explicit fill-side builder.
- Common `Sink`/`Source`/`Link` traits for writing generic event-processing code.
- `forward(src, snk, max)` utility to bridge any `Source` → `Sink`.
- No heap, no dynamic dispatch, no required dependencies.
- Optional `portable-atomic` support for targets without native 32-bit atomics.
- Designed for `#![no_std]` environments (std only for tests).

## Compatibility
- MSRV: Rust 1.92.0.
- `SeqRing::new()` and `EventBuf::new()` assert `N > 0`.
- `SeqRing` and `EventBuf` require 32-bit atomics by default.
- `LatestBuf` also requires 32-bit atomics and stores exactly three payload slots.
- For `thumbv6m-none-eabi` (and other no-atomic targets), enable one of:
  - `portable-atomic-unsafe-assume-single-core`
  - `portable-atomic-critical-section` (requires a critical-section implementation in the binary)
- Those two are **mutually exclusive** — they select different portable-atomic backends, and
  enabling both fails inside portable-atomic. Cargo features are additive, so this cannot be
  expressed in the manifest; `build.rs` detects the combination and explains it.
- Consequently **`--all-features` does not work for this crate** and cannot be made to. Check
  combinations individually; `scripts/ci.sh` enumerates the supported set.

## Usage

### Complete blocks

`BlockBuilder<T, N>` privately accumulates sequenced samples and yields a
`Block<T, N>` only when all `N` contiguous samples are present. A gap is
returned to the caller without changing the partial block, and clearing or
dropping a partial builder publishes nothing. Timestamping is payload policy:
use a timestamped type for `T` when required.

`Block` is deliberately not another queue. Compose it with the overload policy
you need: `EventBuf<Block<T, N>, Q>` queues complete blocks and rejects the
newest when full; the proposed `LatestBuf<Block<T, N>>` will retain only the
latest complete block.

**Budget the composition before choosing it.** Publication copies the
complete block, so cost scales with block bytes (159–8,658 reference
instructions across the measured 2/8/16-byte × N = 8/32/128 grid), a
rejected push costs nearly as much as an accepted one (the complete block
is preserved and returned, within 5–31 instructions), and RAM is multiple
complete blocks — `Q` slots plus the private builder. Small windows can
invert the economics (per-sample publication beats blocks at the
8/16-byte `N = 8` corners), and DMA integrations currently cannot avoid
the double copy in ISR context — the builder's storage is deliberately
private, so either budget both copies or publish from task context. The
`block` module docs carry the full measured disclosure.

```rust
use ph_eventing::{BlockBuilder, EventBuf};

let mut fill = BlockBuilder::<i16, 4>::new();
for (sequence, sample) in [(10, 1), (11, 2), (12, 3)] {
    assert!(fill.push(sequence, sample).unwrap().is_none());
}
let block = fill.push(13, 4).unwrap().unwrap();

let queue = EventBuf::<_, 2>::new();
let producer = queue.try_producer().unwrap();
let consumer = queue.try_consumer().unwrap();
producer.push(block).unwrap();
assert_eq!(consumer.pop().unwrap().samples(), &[1, 2, 3, 4]);
```

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

### LatestBuf

A three-slot SPSC snapshot channel for state where freshness dominates FIFO
delivery. Publishing never rejects; it reports whether an unread value was
replaced. Taking returns the newest complete value with generation and skipped
counts. Exact skipped counts are guaranteed within one non-zero `u32` wrap
span; beyond it the count **under-counts, and a gap of exactly one or more
whole cycles reports `skipped = 0`** — silence there is not evidence that
nothing was lost. The boundary is a rate × take-interval property (~49.7
days between takes at 1 kHz publishing, ~72 minutes at 1 MHz); if the
count itself is your requirement, carry a wider producer-assigned sequence
in `T`, and if consumer liveness is, use a watchdog — the
`LatestItem::skipped` docs carry the full disclosure. The
consumer intentionally implements `LatestSource`, not `Source`, so gap evidence
is not silently discarded. `T` may be one sample or a complete block. Empty
polls use an Acquire load rather than an atomic RMW; pending polls transfer
ownership with one `AcqRel` swap. The all-zero initial representation keeps a
const-initialized channel in `.bss` with no payload-proportional flash or
startup-copy cost.

```rust
use ph_eventing::LatestBuf;

let channel = LatestBuf::<u32>::new();
let producer = channel.try_producer().expect("producer");
let consumer = channel.try_consumer().expect("consumer");

let _ = producer.publish(10);
assert!(producer.publish(20).replaced_unread);
let item = consumer.take_latest().expect("latest");
assert_eq!((item.value, item.generation, item.skipped), (20, 2, 1));
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

### CountedSignal

A saturating count for repeated events whose payload and ordering do not
matter. The sole producer is load-bearing: it permits exact saturation with a
fixed source-level sequence that treats observed `u32::MAX` as maybe-stale and
confirms it through a no-op RMW re-read — an RMW observes the latest value in
modification order, so there is no compare-exchange and no algorithmic retry;
the contract discloses how each single RMW is realised per ISA.

```rust
use ph_eventing::CountedSignal;

let signal = CountedSignal::new();
let producer = signal.try_producer().expect("no producer taken yet");
let consumer = signal.try_consumer().expect("no consumer taken yet");

producer.increment();
producer.increment();
let snapshot = consumer.take_count();
assert_eq!(snapshot.count(), 2);
assert!(!snapshot.is_saturated());
```

### Common Traits

The ring-buffer producers implement `Sink<T>` and their consumers
implement `Source<T>`, so generic code works with any combination of the
listed handles. Signal types such as `CountedSignal` are outside that
stream vocabulary (no `T` payload). `LatestBuf` deliberately stands
outside it as well: its consumer implements `LatestSource<T>` (and its
producer `LatestSink<T>`), because `try_pop` cannot report the
displacement that is this channel's designed overload behaviour — a
generic `Source` bound will not compile against it, by decision D2:

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
| `LatestSink<T>` | Publish latest | `latest_buf::Producer` (reports replacement) |
| `LatestSource<T>` | Take latest | `latest_buf::Consumer` (reports generation + skipped) |

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

### CountedSignal
- Counts below `u32::MAX` are exact; the counter saturates rather than wrapping.
- `take_count` atomically clears the counter and reports whether it saturated.
- A concurrent increment belongs wholly to the current take or the next one.
- The sole `Send + !Sync` producer handle is part of the correctness contract.
- Count operations do not publish unrelated application memory; payload data
  needs a separate synchronization mechanism.
- The reference Cortex-M3 probe measures `increment` at 8 retired
  instructions on the below-`MAX` hot path and 9 on the saturated sentinel
  arm, and `take_count` at 7 (rustc 1.92.0, QEMU 10.0.11). The third arm — a
  stale `MAX` re-read below `MAX` after a take — is the saturated arm plus
  one `fetch_add` by construction; all rows are uncontended single-pass
  counts (the contract discloses the per-ISA RMW realisation).

## Safety and Concurrency
- `RingBuf` has no atomics and no interior mutability — standard Rust borrow rules apply. It stores slots as `MaybeUninit<T>` and reads only live entries, so it does contain `unsafe`.
- `SeqRing`, `EventBuf`, and `CountedSignal` are SPSC by design: exactly one producer and one consumer may be
  active. Use `try_producer()`/`try_consumer()`, which return `None` rather than panicking —
  on a microcontroller a panic is a reset, and the panic machinery costs flash you may not have.
  The panicking `producer()`/`consumer()`, deprecated since 0.2.0, **were removed in
  0.3.0**. Using unsafe to bypass `SeqRing`/`EventBuf` ownership can be undefined
  behavior. Forging or concurrently sharing a `CountedSignal` producer breaks
  its bounded no-wrap contract; the handle is `!Sync` to prevent that in safe Rust.
- `T: Copy` is required by all payload-carrying types to avoid allocation and return values by copy.
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

- **The primitive is shared; the handles are owned.** `SeqRing<T, N>` and
  `EventBuf<T, N>` are `Sync` when `T: Send`, and `CountedSignal` is `Sync`, so the primitive can be handed to both
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

`SeqRing` and `EventBuf` are lock-free, so a green test run on x86 is weak
evidence — a strongly-ordered host cannot exhibit the ordering bugs that appear
on ARM and RISC-V. What backs this crate, in descending order of strength:

| Evidence | What it establishes |
|----------|---------------------|
| [Loom](https://github.com/tokio-rs/loom) models | Exhaustive: every interleaving and every legal relaxed-load value, for the modelled size |
| [Miri](https://github.com/rust-lang/miri) | UB, data races, and weak-memory behaviour; also run on 32-bit and big-endian targets |
| 91 unit + 13 doctests + 9 compile-fail | Behaviour, including threaded stress tests for all SPSC types; `N == 0` rejected at compile time on the three buffers and `BlockBuilder`; `LatestBuf`'s absent `Source` impl and handle `!Sync` pinned (D2/H2); CountedSignal handle `!Sync` pinned |
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
