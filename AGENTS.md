# AGENTS.md

Guidance for coding agents working in the ph-eventing codebase. This is the
canonical agent reference; tool-specific entry points (`CLAUDE.md` and friends)
are thin pointers to this file.

This file is written for machines, not humans. It is not a summary of the
public API — that is on docs.rs, and duplicating it here wastes context. It
exists to carry two things an LLM cannot recover from the source alone:

1. **The internal model** — what the fields mean, which orderings are load
   bearing, and which invariants no type check enforces.
2. **Preventative guardrails** — the specific mistakes that have been made here
   before, or that are cheap to make and expensive to detect.

Length is therefore not a defect in this file. Detail that prevents one silent
concurrency bug pays for itself many times over.

Everything here is meant to be verifiable, and claims in it have been checked
rather than assumed — one "load-bearing" claim about `idx_for` was written,
tested, found false, and rewritten. Where a claim depends on running something,
the command is given. Prefer running it over trusting the prose, and correct the
prose when it disagrees.

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
│   ├── ci.sh               # Local CI matrix (Git Bash on Windows)
│   ├── miri.sh             # Miri UB/concurrency checks
│   └── loom.sh             # Loom model checking
├── build.rs                # guards the mutually exclusive portable-atomic features
├── .github/                # CI workflow (push + PR), issue/PR templates, CODEOWNERS, dependabot
├── deny.toml               # cargo-deny policy: advisories, licences, bans, sources
├── RELEASING.md            # release checklist (version choice, verification, publish, yank)
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
| `RingBuf<T: Copy, N>` | Single-owner, stack-allocated ring buffer (no atomics) |
| `SeqRing<T: Copy, N>` | Lock-free SPSC ring buffer with atomic sequence tracking |
| `seq_ring::Producer<'a, T, N>` | SeqRing write handle; `push(T) -> u32` returns sequence number |
| `seq_ring::Consumer<'a, T, N>` | SeqRing read handle with multiple polling modes |
| `PollStats` | Statistics returned from SeqRing poll operations |
| `EventBuf<T: Copy, N>` | Bounded SPSC ring with backpressure (push returns `Result`) |
| `event_buf::Producer<'a, T, N>` | EventBuf write handle; `push(T) -> Result<(), T>` |
| `event_buf::Consumer<'a, T, N>` | EventBuf read handle; `pop() -> Option<T>`, `peek()`, `drain()` |
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

#### Why `EventBuf::len` cannot exceed capacity on a successful snapshot

The tempting counterexample is a capacity-2 queue where the consumer frees one
slot, the producer publishes a third item, and `len()` appears to read
`tail = 0`, `head = 3`, `tail = 0`. That execution is forbidden; do not add a
clamp to the successful path as a substitute for preserving the ordering:

1. The consumer's `tail.store(1, Release)` synchronizes with the producer's
   `tail.load(Acquire)` that permits the third push.
2. That load is sequenced before the producer's `head.store(3, Release)`.
3. If `len()`'s Relaxed `head` load reads `3`, its following Acquire fence
   synchronizes with that Release store. An Acquire fence deliberately upgrades
   a preceding Relaxed load that reads from a Release store.
4. The second `tail` load is sequenced after the fence. By transitive
   happens-before and atomic write-read coherence, it cannot read the older
   `tail = 0`; it must see `1` or later, so the two tail samples differ and
   `len()` retries.

The proof is specific to `EventBuf`: backpressure forces the producer to
Acquire-load freed `tail` space before Release-publishing a reused `head`.
It does not apply to `SeqRing`, whose producer overwrites without observing a
consumer cursor. Both barriers in `len()` and both producer cursor orderings are
therefore load-bearing. Clamping the equality branch would keep the return value
in range after an ordering regression and could hide the broken snapshot from
tests; retain the unclamped branch so its bound continues to follow from the
memory-ordering proof.

### Internal model (SeqRing)

The public API is on docs.rs; this is the part you cannot infer from it. Three
atomics with distinct jobs, easy to conflate:

| Field | Role | Written by |
|-------|------|-----------|
| `next_seq` | Allocation counter. Holds the last sequence handed out. `push` claims the next one with `fetch_add`. | Producer only |
| `slot_seq[i]` | Which sequence currently occupies slot `i`. **`0` means "invalid / being written"** — it is not a sequence number, it is the reserved sentinel. | Producer only |
| `published_seq` | Visibility barrier. The newest sequence a consumer is allowed to look for. | Producer only |

`push_inner` commits in a fixed order, and the order *is* the correctness
argument — do not reorder it:

```
claim seq (fetch_add, skipping 0 on wrap)
slot_seq[idx] = 0          // invalidate: readers of the old seq now fail
fence(Release)             // keeps the invalidation ahead of the write
write_volatile(slot, val)
slot_seq[idx] = seq        // Release: slot is valid again, under a new seq
published_seq = seq        // Release: consumers may now look for it
```

Invariants that hold across the whole type:

- `published_seq` never runs ahead of `next_seq` — publication happens after
  allocation. A consumer never looks past `published_seq`, so it cannot observe
  a slot that is mid-claim. (Both wrap, so compare them with `seq_distance`,
  not `<`.)
- `idx_for` must be a bijection over any `N` consecutive sequences, and the
  producer and consumer must use **the same one**. The current `(seq - 1) % N`
  is convention, not correctness — it just lands sequence 1 in slot 0. Verified:
  changing it to `seq % N` passes the whole suite, because a rotation is still a
  bijection. The hazard is not the offset, it is inlining a *different* mapping
  in one place and leaving `idx_for` in the other; the sequence check would then
  reject every read and every item would count as dropped, with no compile
  error and no panic.
- In `poll_up_to`, `read + dropped` always equals the number of sequences the
  cursor advanced past. Every branch must maintain that: a successful read
  increments `read`, a missing slot increments `dropped`, and the lag-jump adds
  the whole skipped span. Breaking it makes the drop counter silently wrong,
  which no type check will catch.

All three buffers store slots as `MaybeUninit<T>` and require only `T: Copy`.
`RingBuf` used to require `T: Default` because it initialised a real `[T; N]`;
it no longer does. The trade is that `RingBuf` is no longer free of `unsafe` —
one invariant now carries every read: **the `len` slots ending at `head` have
all been written by `push`**. Every read goes through the private `index`,
which addresses only that range, and every public accessor checks `len` first.
A change that lets `len` outrun the number of writes is UB, not a logic bug.

### Conditional compilation map

Five overlapping axes. Putting code under the wrong one is a common and quiet
mistake:

| Cfg | Active when | Purpose |
|-----|-------------|---------|
| `#[cfg(test)]` | `cargo test` | Unit tests, and the `TEST_AFTER_READ_*` hook |
| `#[cfg(loom)]` | `RUSTFLAGS="--cfg loom"` | Swaps `crate::sync` to Loom's primitives |
| `#[cfg(all(loom, test))]` | both | `src/loom_tests.rs` — the models |
| `cfg!(miri)` (runtime) | under Miri | `test_support::iterations` scales stress loops down |
| `target_has_atomic = "32"` / feature cfgs | per target | `core` vs `portable-atomic` |

`TEST_AFTER_READ_TARGET` / `TEST_AFTER_READ_SEQ` are a deliberate injection
point, not dead code: they let a single-threaded test force a slot's sequence to
change *between* the consumer's copy and its re-check, which is the exact race
the double-check exists to catch. They use `core` atomics directly because
Loom's are not const-constructible in a `static`.

### Coupled edits

Changes that must land together, none of which the compiler enforces:

| If you change | Also update |
|---------------|-------------|
| Any atomic, ordering, or fence | Run Miri **and** Loom; update the Memory Ordering section above |
| Add or remove a test | Test counts in `README.md` and the Testing section below |
| Add a file under `src/` | The `include` allowlist in `Cargo.toml` |
| A public API or its behaviour | `README.md`, the module `//!` docs, and `CHANGELOG.md` |
| A user-visible guarantee | Both user surfaces — `README.md` *and* the `//!` docs, since crates.io and docs.rs show different things |
| MSRV | `Cargo.toml`, `rust-toolchain.toml`, the README badge, `CONTRIBUTING.md` — and it is a breaking change |

### Consumer Polling Modes (SeqRing)

- `poll_one(hook)` - Drain one item in-order
- `poll_one_value()` - Same, returning `Option<(u32, T)>`
- `poll_up_to(max, hook)` - Drain up to N items in-order
- `latest(hook)` / `latest_value()` - Read newest item (not in-order, doesn't advance cursor)
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

### CI

GitHub Actions runs on every push to `master` or a `release/**` branch and on
every PR. It is deliberately **a subset** of the local matrix:

| | Remote | Local |
|---|---|---|
| fmt, clippy, test (MSRV + stable), doc, deny, features, 3 embedded targets | yes | yes |
| coverage (90% floor) | no | yes |
| Miri | no | yes |
| Loom | no | yes |

The gap is not an oversight. Coverage is non-deterministic here, and Miri and
Loom are slow and are the only real evidence for the lock-free types — treating
a green x86 check as a substitute for them is the exact mistake this file exists
to prevent. Run the full matrix locally before every commit:

```bash
./scripts/ci.sh
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
./scripts/miri.sh        # SEEDS=64 for deeper schedule exploration
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

### Feature combinations and toolchains

Two traps that look like bugs but are not:

- **`--all-features` fails and cannot be made to pass.**
  `portable-atomic-unsafe-assume-single-core` and
  `portable-atomic-critical-section` select different portable-atomic backends.
  Cargo features are additive, so the manifest cannot express the exclusion.
  The guard lives in [build.rs](build.rs) — **not** a `compile_error!` in
  `src/lib.rs`, which would be unreachable: the dependency's own guard fires
  while portable-atomic compiles, before this crate is compiled at all
  (verified on host and embedded targets). A build script does not depend on
  portable-atomic, so it runs regardless and its message is actually seen.
  It does not suppress the upstream error; it adds an explanation next to it.
  Enumerate the supported set instead of sweeping; `scripts/ci.sh` does.
  `build.rs` is in the `include` allowlist — dropping it there would ship a
  crate that cannot build.
- **`rust-toolchain.toml` pins 1.92.0 and overrides whatever a CI action
  installs.** A bare `cargo` always runs the MSRV, so a job labelled "stable"
  that just runs `cargo test` is silently testing the MSRV. Use an explicit
  `cargo +stable`, which does take precedence. Local CI has a `stable:` pass
  for exactly this reason — without it nothing would catch a new stable lint.

### Releasing

Follow [RELEASING.md](RELEASING.md). Two things it exists to stop:

- Publishing on a run that reported `SKIP` for an uninstalled tool. A `SKIP` is
  not a pass, and remote CI does not cover the checks most likely to be skipped
  — coverage, Miri, and Loom are local-only.
- Treating a pre-1.0 breaking change as a patch bump. Under Cargo's semver
  rules the **minor** position is the breaking one below 1.0, so a behaviour
  change a caller could rely on means `0.1.x` → `0.2.0`.

`cargo publish` cannot be undone. A yank discourages new use but does not
delete the version or free the number.

### Supply chain (cargo-deny)

The zero-dependency stance is enforced, not merely stated. `cargo deny check`
fails on a new dependency's licence, a security advisory, a yanked crate, a
duplicate version, a wildcard requirement, or a non-crates.io source:

```bash
cargo install cargo-deny     # one-time
cargo deny check             # or: cargo supply
```

The local CI runner includes it when installed and reports `SKIP` when not, so
its absence never silently passes as a clean run.

Policy lives in [deny.toml](deny.toml). Two things to know before editing it:

- The licence allow-list is only `MIT` and `Apache-2.0`. `portable-atomic` and
  `critical-section` are `MIT OR Apache-2.0`, so both are satisfied. Widening
  the list should be a deliberate decision recorded in the diff, not a reflex
  to make a check pass.
- `[advisories] ignore` is empty. If an advisory has to be accepted, add the
  RUSTSEC id **with a comment explaining why it does not apply here**.

`[graph] all-features = true` is set, so the optional `portable-atomic` paths
are covered — verified by temporarily banning the crate and confirming the
check fires without a CLI flag.

### Coverage

`cargo llvm-cov` runs as part of local CI with a **90% line floor**, so it is a
gate rather than a number nobody reads. It is presence-gated like cargo-deny —
`SKIP` when the tool is absent, never a silent pass:

```bash
cargo install cargo-llvm-cov     # one-time
cargo llvm-cov --summary-only    # or: cargo cov
```

Coverage is **not deterministic** in this crate: the threaded stress tests take
different paths each run. Three consecutive runs on one commit gave 93.18%,
93.46%, 93.18% lines. Leave slack between the floor and the observed spread —
a floor set at the best observed run will fail intermittently for no reason.
Raise it only with evidence from several runs, and never lower it to make a run
pass.

Read the number with the right expectations. Coverage cannot see the
properties that actually matter here — ordering and interleaving — so it is a
regression alarm for untested branches, not evidence of concurrency
correctness. Miri and Loom are that evidence. Some uncovered lines are
deliberately hard to reach single-threaded, such as the `EventBuf::len` clamp
fallback, which needs the consumer to move during every retry.

### Model checking (Loom)

Miri samples schedules; Loom is exhaustive. It enumerates every legal execution
of a model — every interleaving, and every value a relaxed load may return —
so a clean run proves the absence of ordering bugs at the modelled size.

```bash
./scripts/loom.sh                     # all models
./scripts/loom.sh event_buf           # filter by name
LOOM_MAX_PREEMPTIONS=3 ./scripts/loom.sh
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
| `cargo supply` | `deny check` (needs `cargo install cargo-deny`) |
| `cargo cov` | `llvm-cov --summary-only` (needs `cargo install cargo-llvm-cov`) |
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
- `works_without_default_bound` — `T: Copy` only, no `Default`
- `const_new_works_in_const_context` — const / `static` initialiser; `N == 0` is now a build failure, not a runtime panic
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
- `try_producer_and_try_consumer` — Fallible handle creation
- `peek_copies_without_advancing` — `peek` vs `pop`

**`seq_ring::tests`:**
- `poll_one_empty_returns_false` — Empty ring behavior
- `polls_in_order` — Sequential consumption
- `poll_up_to_zero_returns_newest_only` — `max = 0` reads nothing, still reports `newest`
- `poll_up_to_counts_dropped_when_slot_missing` — A published seq whose slot never landed counts as `dropped`, not `read`; the hook must not run
- `drops_when_consumer_lags` — Overwrite/drop semantics
- `latest_reads_newest` — Out-of-order read
- `latest_empty_returns_false` — `latest` on an untouched ring reports no read
- `latest_returns_false_when_slot_missing` — `latest` rejects a published seq with no matching slot
- `skip_to_latest_makes_next_poll_latest` — Cursor fast-forward
- `read_seq_inner_rejects_invalidated_slot` — Slot invalidated mid-overwrite reads as absent
- `read_seq_inner_detects_overwrite_during_read` — The `TEST_AFTER_READ_*` hook changes the slot seq between the copy and the re-check; the read is discarded
- `consumer_skips_reserved_seq_zero_on_wrap` — Sequence wrap skips reserved `0`
- `push_wraps_seq_from_zero_to_one` — Producer side of the same wrap: `next_seq = u32::MAX` yields `1`, not `0`
- `lag_across_wrap_counts_drops_exactly` — Drop accounting across the `u32` wrap
- `seq_distance_skips_the_reserved_zero` — Sequence distance excludes reserved `0`
- `dropped_accum_saturates_instead_of_overflowing` — 32-bit drop-counter saturation
- `dropped_counter_can_reset` — `dropped()` matches `PollStats::dropped`; `reset_dropped()` clears it
- `concurrent_overwrite_never_yields_a_mismatched_value` — Threaded overwrite stress; no stale or torn payload
- `capacity_returns_n` — capacity() API
- `try_producer_and_try_consumer` — Fallible handle creation
- `poll_one_value_and_latest_value` — Value-returning poll APIs

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

**Doctests:** Four doctests in `src/lib.rs` demonstrating `RingBuf`, `SeqRing`, `EventBuf`, and `forward` usage, plus one in `src/ring.rs`, one in `src/event_buf.rs`, and one in `src/traits.rs`. Total: 59 unit tests + 7 doctests, plus 1 `compile_fail` doctest pinning the `N == 0` rejection (`E0080`) that `zero_capacity_panics` used to cover.

## Code Conventions

### Rust Patterns

- **`#[inline]` / `#[inline(always)]`** on hot-path methods
- **`#[must_use]`** on types that shouldn't be silently discarded (e.g., `PollStats`)
- **`const fn`** for compile-time calculations (`idx_for`)
- **Internal methods** suffixed with `_inner` (e.g., `push_inner`, `read_seq_inner`)
- **Accumulator fields** suffixed with `_accum` (e.g., `dropped_accum`)

### Safety Requirements

- `T: Copy` required by `RingBuf`, `SeqRing`, and `EventBuf` for value-copy returns
- `T: Send` required for `SeqRing` and `EventBuf` to be `Sync`
- Unsafe code is confined to `MaybeUninit` / `UnsafeCell` slot access in all three buffers
- No panics in hot paths. `RingBuf::new` rejects `N == 0` with a **const**
  assertion, so a zero-capacity ring fails the build rather than panicking —
  which also means no test can cover the rejection, and the const assertion is
  the only thing enforcing it

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
5. **Run local CI before committing:** `./scripts/ci.sh` — remote CI runs too, but without coverage, Miri, or Loom, so it is not the whole gate
6. **Run Miri after touching atomics or unsafe:** `./scripts/miri.sh` — native tests on x86 cannot see weak-memory or 32-bit bugs
7. **Run Loom after changing any ordering:** `./scripts/loom.sh` — exhaustive proof for the modelled size, and it catches weakened orderings that Miri may miss

### Key Invariants to Preserve

- `RingBuf` has no atomics and no interior mutability; its `unsafe` is limited to reading `MaybeUninit` slots that `push` initialised
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
- `SeqRing`: the seqlock data race is **known and documented**, not an oversight. Do not "fix" it by weakening the sequence guards, and do not silence it by disabling Miri's race detector globally — the split-pass structure in `scripts/miri.sh` exists so everything else stays fully checked
- `EventBuf`: race-free by construction — producer and consumer never touch the same slot. If a change makes them share one, that is a design break, not a tuning decision

### Common Tasks

**Adding a new consumer method:**
1. Add the method to the `Consumer` impl in `src/seq_ring.rs`
2. Read slots through `read_seq_inner` — never touch `slots[i]` directly, or you
   lose the double-sequence-check and the `MaybeUninit` discipline
3. Compare sequences with `seq_distance`, never raw `wrapping_sub`
4. If the method can skip items, update `dropped_accum` with `saturating_add`
   and preserve the `read + dropped` invariant
5. Add a test in the `tests` module, and update the counts in `README.md`
6. `./scripts/ci.sh`, then `./scripts/miri.sh` and `./scripts/loom.sh` — a
   passing `cargo test` is not evidence for a change in this file

**Adding a new event type feature:**
1. Preserve `T: Copy`; adding a bound is a breaking change
2. If it needs a new atomic or cell, take it from `crate::sync`, never `core`
3. Document the guarantee on *both* user surfaces — `README.md` and the `//!`
   docs — since crates.io and docs.rs show different things

### What to Avoid

- Adding external dependencies
- Using heap allocation (`Box`, `Vec`, `String` in library code)
- Removing or weakening atomic ordering (Loom will catch it — a `Release` downgraded to `Relaxed` fails three models immediately)
- Adding runtime dependencies, or moving `loom` out of `[target.'cfg(loom)'.dev-dependencies]`
- Widening `deny.toml`'s licence allow-list or adding an `[advisories] ignore` entry to make a check pass, rather than to record a decision
- Lowering the coverage floor in `scripts/ci.sh` to make a run pass
- Committing editor, IDE, or AI/agent settings. `.gitignore` covers the common ones; shared project config belongs at the repo root
- Adding `--all-features` to any script or workflow — it cannot work here (see above)
- Replacing a SHA-pinned action in `.github/workflows/` with a tag. Tags are mutable; that pinning is the point. Dependabot proposes the bumps
- Removing `!src/loom_tests.rs` from the `include` allowlist, or adding `build.rs` back out of it — the first ships dead weight, the second ships a crate that cannot build
- Assuming a CI job labelled "stable" runs stable. Without an explicit `cargo +stable`, the pin in `rust-toolchain.toml` wins
- Importing `core::sync::atomic` directly in `event_buf.rs` or `seq_ring.rs` instead of `crate::sync`
- Breaking API compatibility without explicit request
- Adding `std`-only features to the main library path
