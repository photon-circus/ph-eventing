# Contributing to ph-eventing

Thanks for your interest in contributing! This document covers guidelines and
the workflow for submitting changes.

## Getting Started

1. Fork and clone the repository.
2. Ensure you have Rust **1.92.0** or later (the project's MSRV).
   The [rust-toolchain.toml](rust-toolchain.toml) file will select the correct
   toolchain automatically if you use `rustup`.
3. Run the test suite to confirm everything is green:
   ```bash
   cargo test
   ```

## Development Workflow

### Build

```bash
cargo build
```

### Test

```bash
cargo test
```

All new code should include tests. Tests live in `#[cfg(test)] mod tests`
blocks inside the source file they exercise.

### Embedded target check

The crate is `#![no_std]`. Before opening a PR, verify it compiles for the
embedded targets:

```bash
cargo emb-thumbv7em      # cargo check --target thumbv7em-none-eabi
cargo emb-thumbv6m       # thumbv6m, with portable-atomic
cargo emb-riscv32imac    # riscv32imac
```

### Feature combinations

`portable-atomic-unsafe-assume-single-core` and `portable-atomic-critical-section`
are mutually exclusive, so **`cargo test --all-features` fails** — that is
expected, not a bug you introduced. `build.rs` will say so in the error output,
alongside the noisier message from portable-atomic itself. Check the supported
set individually:

```bash
cargo check
cargo check --features portable-atomic
cargo check --features portable-atomic-critical-section
```

`./scripts/ci.sh` runs all three for you.

### Formatting and lints

```bash
cargo fmt-check
cargo lint               # cargo clippy --all-targets -- -D warnings
```

### Everything at once

```bash
./scripts/ci.sh          # Git Bash on Windows
```

### Supply chain

This crate has zero runtime dependencies and intends to keep it that way. If
your change touches `Cargo.toml`, run:

```bash
cargo install cargo-deny     # one-time
cargo deny check
```

New dependencies need a case made in the pull request. The licence allow-list
in `deny.toml` is `MIT` / `Apache-2.0` only — widening it is a reviewable
decision, not a formality.

### Concurrency changes

If you touch atomics, orderings, fences, or unsafe blocks in `SeqRing` or
`EventBuf`, `cargo test` passing is not evidence — a strongly-ordered x86 host
cannot exhibit the bugs that appear on ARM and RISC-V. Run both checkers:

```bash
./scripts/miri.sh        # UB, data races, weak memory, 32-bit and big-endian
./scripts/loom.sh        # exhaustive model checking of every interleaving
```

Miri needs `rustup component add --toolchain nightly miri`. Loom needs nothing
installed — it is a dev-dependency gated on `--cfg loom` that the script sets,
and it never enters a normal build.

A bare `+nightly` drifts daily, so a Miri verdict is only reproducible next to
the toolchain that produced it: `miri.sh` prints its exact nightly every run,
and `MIRI_TOOLCHAIN=nightly-2026-08-08 ./scripts/miri.sh` replays under a
specific one. Or skip the setup entirely and use the reference environment
below.

What each pass is for:

- **Miri** makes three host passes — full checking, the `SeqRing` overwrite test
  with the data-race detector off, and a multi-seed scheduler sweep — then
  repeats the full pass on 32-bit and big-endian targets. The bare-metal targets
  cannot be run: Miri needs `std` for the test harness. `i686` and
  `armv7-unknown-linux-gnueabihf` stand in for them, sharing the 32-bit pointer
  width and `usize` range the cursor arithmetic depends on; `s390x` covers
  big-endian. That is not academic — the 32-bit pass caught a real counter
  overflow the 64-bit host could not see.
- **Loom** is exhaustive where Miri samples. Keep models tiny (capacity 2, two
  or three operations per thread): the state space grows exponentially.

`SeqRing` has one known, documented Miri deviation — it is a seqlock, and the
race is real by the letter of the memory model. Read its module docs before
reporting a Miri finding as new.

### Code size on the embedded targets

If you change an API shape that embedded callers reach for -- especially
anything touching atomics -- measure it before arguing about it:

```bash
./scripts/codesize.sh              # 8 upstream targets
XTENSA=1 ./scripts/codesize.sh     # plus ESP32, needs the esp-rs fork
```

A design that looks cheaper on Cortex-M4 can be markedly worse on Cortex-M0+,
where portable-atomic turns each read-modify-write into an interrupt-disable
critical section, or on RISC-V, where `compare_exchange` lowers to an LR/SC
loop while `fetch_or` is a single instruction. One target is not evidence.

Nothing extra to install: `llvm-size` comes from the `llvm-tools` component
that `rust-toolchain.toml` already declares.

### Instruction cost

If you change a hot path, measure the time cost as well as the size cost:

```bash
./scripts/cycles.sh
```

Needs `qemu-system-arm` (`sudo apt-get install qemu-system-arm`); it skips
cleanly without it. It is deliberately **not** part of `./scripts/ci.sh` —
every other check is satisfied by the pinned toolchain alone, and a check most
contributors cannot run would make a green `ci.sh` mean less rather than more.

The counts are deterministic per QEMU build, not across builds (two of the
eighteen regions were observed to shift by one instruction between builds), so
when comparing against the documented numbers, run inside the reference
environment below.

Paste the numbers into your PR, as with `codesize.sh`.

### The reference environment

The documented numbers — instruction counts, Miri verdicts, the full `ci.sh`
matrix with zero SKIPs — are measured in one pinned Docker image, so that
anyone can reproduce the evidence rather than take the README's word for it:

```bash
./scripts/verify.sh            # ci + miri + loom + cycles, all inside the image
./scripts/verify.sh cycles     # just one of them
./scripts/verify.sh shell      # look around
```

Requires Docker and nothing else; `scripts/verify/Dockerfile` documents
exactly what is pinned and why. A prebuilt copy is published at
[stevegiacomelli/ph-eventing-verify](https://hub.docker.com/r/stevegiacomelli/ph-eventing-verify)
— same Dockerfile, saves you the build, and each `0.x.y` tag is frozen as the
environment that release's evidence was gathered in:

```bash
VERIFY_IMAGE=stevegiacomelli/ph-eventing-verify:0.2.0 ./scripts/verify.sh
```

The matrix is deliberately local-only — the expensive checks were removed
from hosted CI on purpose, and the script refuses to run there so that
decision cannot be reversed by accident.

### Coverage

Local CI enforces a 90% line floor via `cargo llvm-cov`. Coverage is **not
deterministic** here — the threaded stress tests take different paths per run —
so leave slack rather than setting the floor at the best observed run.

Deeper detail on all of the above, including the traps that silently invalidate
the checkers, is in [AGENTS.md](AGENTS.md).

## Design values

Before proposing an API, read **[AGENTS.md](AGENTS.md) § What this crate
optimises for**. The short version, in priority order:

1. **Predictability** — bounded behaviour, and loss that can be observed.
2. **Efficiency** — measured on every target, not inferred from one.
3. **Regression resistance** — a guarantee not pinned by tooling will drift.
4. **Ergonomics** — last, and never bought with the three above.

If the right design is awkward to use, the fix is compile-time tooling — a
macro, type-state, a `const fn` — not a friendlier API that costs something at
runtime. Ergonomics recovered at compile time is free; ergonomics bought with
cycles, flash, or predictability is not.

## What to Contribute

Contributions of all kinds are welcome:

- **Bug reports** — open an issue with a minimal reproduction.
- **Bug fixes** — include a regression test.
- **New features** — please open an issue first to discuss the design.
- **Documentation improvements** — typo fixes, clarifications, examples.

## Constraints

ph-eventing is a no-std, zero-allocation library. All contributions must
preserve these invariants:

- No `std` dependencies in library code (`std` is fine in `#[cfg(test)]`).
- No heap allocation (`Box`, `Vec`, `String`, etc.) in library code.
- `T: Copy` constraint on ring element types must be maintained.
- Atomic orderings in `SeqRing` and `EventBuf` must not be weakened. Loom will
  catch it: downgrading the `EventBuf` publication store from `Release` to
  `Relaxed` fails three models immediately.

## Pull Request Process

1. Create a feature branch from `master` (the default branch).
2. Make your changes in small, focused commits.
3. Add or update tests as appropriate.
4. Run the full check suite locally. GitHub Actions will also run on your PR,
   but it is **a subset** — no coverage, Miri, or Loom — so a green check is
   not a substitute for this:
   ```bash
   ./scripts/ci.sh
   ```
   On Windows, run it under the Git Bash that ships with Git for Windows.
   There is no PowerShell twin: two scripts that must be kept in step drift,
   and the one that drifts is the one you are not running today.
   That runs formatting, clippy, host tests, docs, cargo-deny, a 90% coverage
   floor, and the `thumbv6m` / `thumbv7em` / `riscv32imac` cross-compilation
   checks, then prints a pass/fail summary. Checks needing a separately
   installed tool report `SKIP` rather than passing silently.
5. Open a pull request against `master`. Describe what the change does and why.
6. The maintainer will review and may request changes before merging.

## Code Style

- Follow existing patterns (see [AGENTS.md](AGENTS.md) § Code Conventions).
- Use `#[inline]` / `#[inline(always)]` on hot-path methods.
- Use `#[must_use]` on types that should not be silently discarded.
- Prefer `const fn` where possible.
- Public APIs must have doc comments.

## Releasing

Maintainers: see [RELEASING.md](RELEASING.md) for the release checklist —
version choice, the verification runs, what ships, and what a yank does and
does not do.

## License

By contributing, you agree that your contributions will be licensed under the
[MIT License](LICENSE).
