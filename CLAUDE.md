# CLAUDE.md

This document provides guidance for AI assistants working with the ph-eventing codebase.

## Project Overview

**ph-eventing** provides stack-allocated ring buffers for no-std embedded targets.

It ships three primitives:
- **`RingBuf<T, N>`** — a single-owner ring buffer (no atomics, `&mut` access). Ideal for local event logs, sample windows, and single-context collection.
- **`SeqRing<T, N>`** — a lock-free SPSC ring that **overwrites** old entries. Designed for high-rate telemetry where a fast producer and a potentially slower consumer run on different contexts.
- **`EventBuf<T, N>`** — a lock-free SPSC ring with **backpressure**. `push` returns `Err(val)` when full, so the producer always knows when delivery fails.

**Key characteristics:**
- Zero required dependencies (`portable-atomic` is optional)
- `#![no_std]` by default (std only for testing)
- Fixed-size, zero-allocation, `T: Copy`
- `SeqRing`: producer never blocks; consumer drops old events when lagging
- `SeqRing`: sequence-based tracking with torn-read prevention
- `EventBuf`: producer gets explicit backpressure; no data silently lost
- Common `Sink`/`Source`/`Link` traits unify producers and consumers across buffer types
- `forward()` utility bridges any `Source` into any `Sink`

## Codebase Structure

```
ph-eventing/
├── Cargo.toml              # Package manifest (edition 2024, rust-version 1.92.0)
├── Cargo.lock              # Dependency lock file
├── rust-toolchain.toml     # Rust 1.92.0 with embedded targets
├── README.md               # User documentation
├── LICENSE                 # MIT license
└── src/
    ├── lib.rs              # Crate root, public exports, doctests
    ├── event_buf.rs        # Bounded SPSC event buffer with backpressure
    ├── ring.rs             # Single-owner stack-allocated ring buffer
    ├── seq_ring.rs         # Lock-free SPSC overwrite ring with sequence tracking
    └── traits.rs           # Sink, Source, Link traits and forward() utility
```

## Architecture

### Core Types

| Type | Purpose |
|------|---------|
| `RingBuf<T: Copy + Default, N>` | Single-owner, stack-allocated ring buffer (no atomics) |
| `SeqRing<T: Copy, N>` | Lock-free SPSC ring buffer with atomic sequence tracking |
| `seq_ring::Producer<'a, T, N>` | SeqRing write handle; `push(T) -> u32` returns sequence number |
| `seq_ring::Consumer<'a, T, N>` | SeqRing read handle with multiple polling modes |
| `PollStats` | Statistics returned from SeqRing poll operations |
| `EventBuf<T: Copy, N>` | Bounded SPSC ring with backpressure (push returns `Result`) |
| `event_buf::Producer<'a, T, N>` | EventBuf write handle; `push(T) -> Result<(), T>` |
| `event_buf::Consumer<'a, T, N>` | EventBuf read handle; `pop() -> Option<T>`, `drain()` |
| `Sink<T>` | Trait — accept events via `try_push(&mut self, T) -> Result<(), Error>` |
| `Source<T>` | Trait — yield events via `try_pop(&mut self) -> Option<T>` |
| `Link<In, Out>` | Trait — blanket impl for any `Sink<In> + Source<Out>` |

### Memory Ordering Strategy (SeqRing)

The `SeqRing` implementation uses careful atomic ordering for thread safety:
- **Producer writes:** `Ordering::Release` to publish slot and sequence
- **Consumer reads:** `Ordering::Acquire` to validate sequences
- **Torn-read prevention:** Double-check slot sequence before/after value read

`RingBuf` uses no atomics — it is a plain struct with `&mut self` mutation.

### Memory Ordering Strategy (EventBuf)

`EventBuf` uses a classic Lamport SPSC queue pattern:
- **Producer:** owns `head` (Relaxed load, Release store), reads `tail` with Acquire.
- **Consumer:** owns `tail` (Relaxed load, Release store), reads `head` with Acquire.
- The Release/Acquire on the cursor stores/loads act as the publication fence for slot data.

### Consumer Polling Modes (SeqRing)

- `poll_one(hook)` - Drain one item in-order
- `poll_up_to(max, hook)` - Drain up to N items in-order
- `latest(hook)` - Read newest item (not in-order, doesn't advance cursor)
- `skip_to_latest()` - Fast-forward to newest, skip backlog

## Build Commands

```bash
# Build library
cargo build

# Run all tests (requires std)
cargo test

# Check all targets without building
cargo check --all-targets

# Build documentation
cargo doc --open

# Check for specific embedded target
cargo check --target thumbv7em-none-eabi
```

## Testing

Tests are in `src/ring.rs`, `src/seq_ring.rs`, `src/event_buf.rs`, and `src/traits.rs` in their respective `tests` modules. They require std and use the standard Rust test framework.

**Run tests:**
```bash
cargo test
```

**`ring::tests`:**
- `new_ring_is_empty` — Fresh ring state
- `push_and_get` — Basic push/get/latest
- `overwrite_oldest_when_full` — Wrap-around semantics
- `clear_resets_state` — Clear behaviour
- `iter_oldest_to_newest` — Iterator ordering
- `iter_exact_size` — ExactSizeIterator
- `default_is_new` — Default impl
- `zero_capacity_panics` — N=0 assertion
- `capacity_returns_n` — capacity() API
- `into_iter_for_ref` — IntoIterator for &RingBuf

**`event_buf::tests`:**
- `new_buf_is_empty` — Fresh buffer state
- `push_and_pop_fifo` — FIFO ordering
- `push_rejects_when_full` — Backpressure behaviour
- `drain_returns_count` — Drain with callback
- `drain_on_empty_returns_zero` — Empty drain
- `producer_consumer_can_be_recreated` — Handle drop/recreate
- `double_producer_panics` — SPSC enforcement
- `double_consumer_panics` — SPSC enforcement
- `wraps_around_correctly` — Multi-round wrap exercise
- `default_is_new` — Default impl
- `len_and_full_track_state` — len/is_empty/is_full tracking
- `handles_are_send` — Producer/Consumer are Send

**`seq_ring::tests`:**
- `poll_one_empty_returns_false` — Empty ring behavior
- `polls_in_order` — Sequential consumption
- `drops_when_consumer_lags` — Overwrite/drop semantics
- `latest_reads_newest` — Out-of-order read
- `skip_to_latest_makes_next_poll_latest` — Cursor fast-forward
- `capacity_returns_n` — capacity() API

**`traits::tests`:**
- `ringbuf_as_sink` — `RingBuf` implements `Sink`
- `seq_producer_as_sink` — `seq_ring::Producer` implements `Sink`
- `event_producer_as_sink` — `event_buf::Producer` implements `Sink`
- `seq_consumer_as_source` — `seq_ring::Consumer` implements `Source`
- `event_consumer_as_source` — `event_buf::Consumer` implements `Source`
- `forward_seq_to_event` — SeqRing → EventBuf bridging
- `forward_event_to_ringbuf` — EventBuf → RingBuf bridging
- `forward_stops_when_sink_full` — Backpressure during forward
- `forward_empty_source_transfers_nothing` — No-op forward
- `generic_drain_seq` — Trait-generic code with SeqRing
- `generic_drain_event` — Trait-generic code with EventBuf

**Doctests:** Four doctests in `src/lib.rs` demonstrating `RingBuf`, `SeqRing`, `EventBuf`, and `forward` usage, plus one in `src/ring.rs`, one in `src/event_buf.rs`, and one in `src/traits.rs`. Total: 46 unit tests + 7 doctests.

## Code Conventions

### Rust Patterns

- **`#[inline]` / `#[inline(always)]`** on hot-path methods
- **`#[must_use]`** on types that shouldn't be silently discarded (e.g., `PollStats`)
- **`const fn`** for compile-time calculations (`idx_for`)
- **Internal methods** suffixed with `_inner` (e.g., `push_inner`, `read_seq_inner`)
- **Accumulator fields** suffixed with `_accum` (e.g., `dropped_accum`)

### Safety Requirements

- `T: Copy` required by `RingBuf`, `SeqRing`, and `EventBuf` for value-copy returns
- `T: Default` additionally required by `RingBuf` for array initialisation
- `T: Send` required for `SeqRing` and `EventBuf` to be `Sync`
- Unsafe code is confined to `SeqRing`'s and `EventBuf`'s `UnsafeCell` / `MaybeUninit` operations; `RingBuf` uses no unsafe
- No panics in hot paths; only assertions are in `::new()` for `N > 0`

### Documentation Style

- Module-level docs explain design rationale and memory ordering
- Public APIs have doc comments explaining behavior
- Hook parameters documented as receiving `&T` to a local copy

## Embedded Target Support

The project supports these targets (defined in `rust-toolchain.toml`):
- ARM Cortex-M: `thumbv6m`, `thumbv7m`, `thumbv7em`, `thumbv8m.main`
- ARM Cortex-R: `armv7r`
- ARM v7-A: `armv7a`
- RISC-V: `riscv32imac`
- WebAssembly: `wasm32-unknown-unknown`

## AI Assistant Guidelines

### When Making Changes

1. **Read before modifying:** Always read relevant files before suggesting changes
2. **Preserve no-std compatibility:** Never add std dependencies to library code
3. **Maintain zero-allocation guarantee:** No heap allocations in the library
4. **Test after changes:** Run `cargo test` to verify functionality
5. **Check embedded targets:** Run `cargo check --target thumbv7em-none-eabi`

### Key Invariants to Preserve

- `RingBuf` is fully safe — no unsafe code, no interior mutability
- `RingBuf`: Ring capacity `N` must be > 0
- `RingBuf.len` is always ≤ `N`; `head` is always < `N`
- `SeqRing`: Sequence 0 is reserved for "empty" state
- `SeqRing`: Producer never blocks or returns errors
- `SeqRing`: Consumer tracks dropped items accurately
- `SeqRing`: Torn reads are impossible (double-sequence-check pattern)
- `SeqRing`: Ring capacity `N` must be > 0
- `EventBuf`: `push` must return `Err(val)` when full — never overwrite
- `EventBuf`: `pop`/`drain` return items in strict FIFO order
- `EventBuf`: Ring capacity `N` must be > 0
- `EventBuf`: `head.wrapping_sub(tail)` always represents the item count
- `EventBuf`: Producer and Consumer handles are `Send + !Sync`
- `SeqRing`: Producer and Consumer handles are `Send + !Sync`

### Common Tasks

**Adding a new consumer method:**
1. Add method to `Consumer` impl in `src/seq_ring.rs`
2. Use existing `read_seq_inner` for safe slot reads
3. Update `dropped_accum` if items are skipped
4. Add test case in the `tests` module
5. Run `cargo test`

**Adding a new event type feature:**
1. Ensure `T: Copy` constraint is preserved
2. Consider if new constraints are needed
3. Document any new requirements

### What to Avoid

- Adding external dependencies
- Using heap allocation (`Box`, `Vec`, `String` in library code)
- Removing or weakening atomic ordering
- Breaking API compatibility without explicit request
- Adding `std`-only features to the main library path
