# CLAUDE.md

This document provides guidance for AI assistants working with the ph-eventing codebase.

## Project Overview

**ph-eventing** is a lock-free SPSC (Single Producer Single Consumer) event ring library for high-throughput telemetry in embedded systems. It is designed for no-std environments where allocation is not available.

**Key characteristics:**
- Zero external dependencies
- `#![no_std]` by default (std only for testing)
- Producer never blocks; consumer drops old events when lagging
- Sequence-based tracking with torn-read prevention

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
    └── seq_ring.rs         # Core SeqRing implementation and tests
```

## Architecture

### Core Types

| Type | Purpose |
|------|---------|
| `SeqRing<T: Copy, N>` | Main ring buffer with atomic sequence tracking |
| `Producer<'a, T, N>` | Write handle; `push(T) -> u32` returns sequence number |
| `Consumer<'a, T, N>` | Read handle with multiple polling modes |
| `PollStats` | Statistics returned from poll operations |

### Memory Ordering Strategy

The implementation uses careful atomic ordering for thread safety:
- **Producer writes:** `Ordering::Release` to publish slot and sequence
- **Consumer reads:** `Ordering::Acquire` to validate sequences
- **Torn-read prevention:** Double-check slot sequence before/after value read

### Consumer Polling Modes

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

Tests are in `src/seq_ring.rs` in the `tests` module. They require std and use the standard Rust test framework.

**Run tests:**
```bash
cargo test
```

**Current test coverage:**
- `poll_one_empty_returns_false` - Empty ring behavior
- `polls_in_order` - Sequential consumption
- `drops_when_consumer_lags` - Overwrite/drop semantics
- `latest_reads_newest` - Out-of-order read
- `skip_to_latest_makes_next_poll_latest` - Cursor fast-forward

**Doctest:** There is one doctest in `src/lib.rs` demonstrating basic usage.

## Code Conventions

### Rust Patterns

- **`#[inline]` / `#[inline(always)]`** on hot-path methods
- **`#[must_use]`** on types that shouldn't be silently discarded (e.g., `PollStats`)
- **`const fn`** for compile-time calculations (`idx_for`)
- **Internal methods** suffixed with `_inner` (e.g., `push_inner`, `read_seq_inner`)
- **Accumulator fields** suffixed with `_accum` (e.g., `dropped_accum`)

### Safety Requirements

- `T: Copy` constraint required for lock-free value returns
- `T: Send` required for `SeqRing` to be `Sync`
- Unsafe code is confined to `UnsafeCell` and `MaybeUninit` operations
- No panics in hot paths; only assertion is in `SeqRing::new()` for `N > 0`

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

- Sequence 0 is reserved for "empty" state
- Producer never blocks or returns errors
- Consumer tracks dropped items accurately
- Torn reads are impossible (double-sequence-check pattern)
- Ring capacity `N` must be > 0

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
