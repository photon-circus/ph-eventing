# AGENTS.md

Guidance for coding agents working in the ph-eventing codebase. This is the
canonical agent reference; tool-specific entry points (`CLAUDE.md` and friends)
are thin pointers to this file.

Everything here is meant to be verifiable. Where a claim depends on running
something, the command is given — prefer running it over trusting the prose,
and correct the prose when it disagrees.

## Project Overview

**ph-eventing** provides stack-allocated ring buffers for no-std embedded targets.

It ships three primitives:
- **`RingBuf<T, N>`** — a single-owner ring buffer (no atomics, `&mut` access). Ideal for local event logs, sample windows, and single-context collection.
- **`SeqRing<T, N>`** — a lock-free SPSC ring that **overwrites** old entries. Designed for high-rate telemetry where a fast producer and a potentially slower consumer run on different contexts.
- **`EventBuf<T, N>`** — a lock-free SPSC ring with **backpressure**. `push` returns `Err(val)` when full, so the producer always knows when delivery fails.

**Key characteristics:**
- Zero runtime dependencies (`portable-atomic` is optional; `loom` is a dev-dependency gated on `--cfg loom`). Verify with `cargo tree` — it must print the crate alone.
- `#![no_std]` by default (std only for testing)
- Fixed-size, zero-allocation, `T: Copy`
- `SeqRing`: producer never blocks; consumer drops old events when lagging
- `SeqRing`: sequence-based tracking; a raced read is detected and discarded rather than returned
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
├── scripts/
│   ├── ci.ps1              # Local CI matrix (Windows)
│   ├── ci.sh               # Local CI matrix (POSIX)
│   ├── miri.ps1            # Miri UB/concurrency checks (Windows)
│   ├── miri.sh             # Miri UB/concurrency checks (POSIX)
│   ├── loom.ps1            # Loom model checking (Windows)
│   └── loom.sh             # Loom model checking (POSIX)
└── src/
    ├── lib.rs              # Crate root, public exports, doctests
    ├── event_buf.rs        # Bounded SPSC event buffer with backpressure
    ├── ring.rs             # Single-owner stack-allocated ring buffer
    ├── seq_ring.rs         # Lock-free SPSC overwrite ring with sequence tracking
    ├── sync.rs             # Atomic/cell shim — swaps in Loom's primitives under --cfg loom
    ├── loom_tests.rs       # Exhaustive concurrency models (--cfg loom only)
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
- **Producer writes:** invalidate `slot_seq` to `0`, `fence(Release)`, write the value, then publish `slot_seq` and `published_seq` with `Ordering::Release`
- **Consumer reads:** validate `slot_seq` with `Ordering::Acquire`, copy the value, `fence(Acquire)`, re-check `slot_seq`
- **Torn-read prevention:** double-check slot sequence before/after the value read
- **Fences, not just ordered accesses:** the `Release`/`Acquire` *fences* bracket the value access. Ordering on the `slot_seq` store/load alone is one-way and would let the value access drift across the guard it is meant to sit inside.
- **Volatile slot access:** the value is read and written with `read_volatile`/`write_volatile`, and the consumer holds its copy as `MaybeUninit<T>` until the re-check passes, so a raced copy is discarded as raw bytes and never becomes an invalid `T`.
- **Sequence arithmetic:** use `seq_distance`, not raw `wrapping_sub` — the latter over-counts by one across the wrap because `push` skips the reserved `0`.

`RingBuf` uses no atomics — it is a plain struct with `&mut self` mutation.

### Memory Ordering Strategy (EventBuf)

`EventBuf` uses a classic Lamport SPSC queue pattern:
- **Producer:** owns `head` (Relaxed load, Release store), reads `tail` with Acquire.
- **Consumer:** owns `tail` (Relaxed load, Release store), reads `head` with Acquire.
- The Release/Acquire on the cursor stores/loads act as the publication fence for slot data.
- Producer and consumer never touch the same slot, so the slots themselves are race-free; only the cursors are shared.
- `len()` is the one observer reading both cursors. It brackets its `head` load between two `tail` samples (Acquire load, then `fence(Acquire)`), retries a bounded number of times, then falls back to a clamped estimate so it is always wait-free.

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

### Local CI

CI for this project runs locally; the GitHub Actions workflow mirrors the same
jobs but is `workflow_dispatch` only, so nothing runs automatically on push or
PR. Run the full matrix — fmt, clippy, test, doc, and the `thumbv6m` /
`thumbv7em` / `riscv32imac` cross-compilation checks — before every commit:

```bash
./scripts/ci.sh          # POSIX
```

```powershell
./scripts/ci.ps1         # Windows
```

Every check runs even if an earlier one fails; a pass/fail summary is printed
at the end and the exit code is non-zero if anything failed. Skip the
cross-compilation checks with `SKIP_EMBEDDED=1` / `-SkipEmbedded`, and stop at
the first failure with `FAIL_FAST=1` / `-FailFast`.

### Concurrency checking (Miri)

`cargo test` passing on x86 proves little about `SeqRing` and `EventBuf` — a
strongly-ordered host hides the ordering bugs that appear on ARM and RISC-V.
Run Miri after **any** change to atomics, orderings, fences, or unsafe blocks:

```bash
./scripts/miri.sh        # POSIX; SEEDS=64 for deeper schedule exploration
```

```powershell
./scripts/miri.ps1       # Windows; -Seeds 64
```

Requires the nightly toolchain and the `miri` component:
`rustup component add --toolchain nightly miri`.

Three host passes plus cross-target passes:

| Pass | Purpose |
|------|---------|
| full checking (skips the `SeqRing` overwrite test) | UB, data races, provenance, leaks |
| `SeqRing` overwrite test, race detector off | seqlock logic under Miri's scheduler |
| multi-seed sweep | schedule exploration across interleavings |
| `i686` / `armv7` / `s390x` | 32-bit pointer width and big-endian |

**Miri cannot run the bare-metal targets** — it needs `std` for the test
harness, and `thumbv*` / `riscv32imac` have none. The 32-bit std targets are the
stand-in: same pointer width and `usize` range, which is what the sequence and
cursor arithmetic depends on. This is not academic — the 32-bit pass caught a
real `dropped_accum` overflow that the 64-bit host run could not.

**`SeqRing` has a known, documented data race** (it is a seqlock). Do not
"fix" it by weakening the sequence guards, and do not silence it by disabling
the race detector globally — the split-pass structure exists so everything else
stays fully checked. See the `seq_ring` module docs.

### Model checking (Loom)

Miri samples schedules; Loom is exhaustive. It enumerates every legal execution
of a model — every interleaving, and every value a relaxed load may return —
so a clean run proves the absence of ordering bugs at the modelled size.

```bash
./scripts/loom.sh                     # POSIX
./scripts/loom.sh event_buf           # filter by name
```

```powershell
./scripts/loom.ps1 -Filter event_buf -MaxPreemptions 3
```

Models live in [src/loom_tests.rs](src/loom_tests.rs). **Keep them tiny** —
capacity 2, two or three operations per thread. The state space grows
exponentially; raising `MaxPreemptions` or adding a fourth push can turn a
5-second run into an unbounded one.

Loom substitutes its own instrumented atomics and cells via
[src/sync.rs](src/sync.rs). Rules when touching that shim:

- **Never import `core::sync::atomic` directly in `event_buf.rs` or
  `seq_ring.rs`** — go through `crate::sync`, or Loom silently stops modelling
  that access and the models become worthless while still passing.
- Loom's atomics are **not const-constructible**. Anything in a `static` must
  use `core` atomics explicitly (see the test hook in `seq_ring.rs`).
- `EventBuf` slots use `sync::TrackedCell` so Loom can verify that the producer
  and consumer never touch the same slot. `SeqRing` slots deliberately use
  `core::cell::UnsafeCell` — its slot access is racy by construction, and
  tracking it would only re-report the documented deviation.

**Loom must never become a runtime dependency.** It is declared under
`[target.'cfg(loom)'.dev-dependencies]`, so a normal build never resolves it.
After changing `Cargo.toml`, confirm with `cargo tree` — it must print the
crate alone, with no dependencies.

Individual checks are also available as cargo aliases (see
[.cargo/config.toml](.cargo/config.toml)):

| Alias | Runs |
|-------|------|
| `cargo f` | `fmt --all` |
| `cargo fmt-check` | `fmt --all -- --check` |
| `cargo t` | `test` |
| `cargo lint` | `clippy --all-targets -- -D warnings` |
| `cargo docs` | `doc --no-deps` |
| `cargo emb-thumbv6m` | `check --target thumbv6m-none-eabi --features portable-atomic-unsafe-assume-single-core` |
| `cargo emb-thumbv7em` | `check --target thumbv7em-none-eabi` |
| `cargo emb-riscv32imac` | `check --target riscv32imac-unknown-none-elf` |

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
- `len_stays_within_capacity_while_consumer_drains` — `len()` bound under concurrency
- `concurrent_spsc_preserves_fifo_and_loses_nothing` — threaded FIFO/no-loss stress
- `handles_are_send` — Producer/Consumer are Send

**`seq_ring::tests`:**
- `poll_one_empty_returns_false` — Empty ring behavior
- `polls_in_order` — Sequential consumption
- `drops_when_consumer_lags` — Overwrite/drop semantics
- `latest_reads_newest` — Out-of-order read
- `skip_to_latest_makes_next_poll_latest` — Cursor fast-forward
- `read_seq_inner_rejects_invalidated_slot` — Slot invalidated mid-overwrite reads as absent
- `consumer_skips_reserved_seq_zero_on_wrap` — Sequence wrap skips reserved `0`
- `lag_across_wrap_counts_drops_exactly` — Drop accounting across the `u32` wrap
- `seq_distance_skips_the_reserved_zero` — Sequence distance excludes reserved `0`
- `dropped_accum_saturates_instead_of_overflowing` — 32-bit drop-counter saturation
- `concurrent_overwrite_never_yields_a_mismatched_value` — Threaded overwrite stress; no stale or torn payload
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

**Doctests:** Four doctests in `src/lib.rs` demonstrating `RingBuf`, `SeqRing`, `EventBuf`, and `forward` usage, plus one in `src/ring.rs`, one in `src/event_buf.rs`, and one in `src/traits.rs`. Total: 54 unit tests + 7 doctests.

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
5. **Run local CI before committing:** `./scripts/ci.sh` (or `./scripts/ci.ps1`) — nothing runs remotely on push, so this is the only gate
6. **Run Miri after touching atomics or unsafe:** `./scripts/miri.sh` — native tests on x86 cannot see weak-memory or 32-bit bugs
7. **Run Loom after changing any ordering:** `./scripts/loom.sh` — exhaustive proof for the modelled size, and it catches weakened orderings that Miri may miss

### Key Invariants to Preserve

- `RingBuf` is fully safe — no unsafe code, no interior mutability
- `RingBuf`: Ring capacity `N` must be > 0
- `RingBuf.len` is always ≤ `N`; `head` is always < `N`
- `SeqRing`: Sequence 0 is reserved for "empty" state
- `SeqRing`: Producer never blocks or returns errors
- `SeqRing`: Consumer tracks dropped items accurately, including across the `u32` wrap — compare sequences with `seq_distance`, never raw `wrapping_sub`
- `SeqRing`: a torn or stale read is never *returned* — the double-sequence-check discards it. Note the precise claim: the read itself still races with the producer (see the known deviation below); what is guaranteed is that a raced copy is never observed and never becomes an invalid `T`
- `SeqRing`: A raced slot copy never materialises as a `T` — it stays `MaybeUninit<T>` until the re-check passes
- `SeqRing`: Ring capacity `N` must be > 0
- `EventBuf`: `push` must return `Err(val)` when full — never overwrite
- `EventBuf`: `pop`/`drain` return items in strict FIFO order
- `EventBuf`: Ring capacity `N` must be > 0
- `EventBuf`: `head.wrapping_sub(tail)` always represents the item count
- `EventBuf`: `len()` never exceeds `N` and never blocks, even while both handles are active
- `SeqRing`: `dropped_accum` saturates — it must never overflow, and `usize` is 32-bit on every shipped target
- `EventBuf`: Producer and Consumer handles are `Send + !Sync`
- `SeqRing`: Producer and Consumer handles are `Send + !Sync`
- `SeqRing`: the seqlock data race is **known and documented**, not an oversight. Do not "fix" it by weakening the sequence guards, and do not silence it by disabling Miri's race detector globally — the split-pass structure in `scripts/miri.*` exists so everything else stays fully checked
- `EventBuf`: race-free by construction — producer and consumer never touch the same slot. If a change makes them share one, that is a design break, not a tuning decision

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
- Removing or weakening atomic ordering (Loom will catch it — a `Release` downgraded to `Relaxed` fails three models immediately)
- Adding runtime dependencies, or moving `loom` out of `[target.'cfg(loom)'.dev-dependencies]`
- Importing `core::sync::atomic` directly in `event_buf.rs` or `seq_ring.rs` instead of `crate::sync`
- Breaking API compatibility without explicit request
- Adding `std`-only features to the main library path
