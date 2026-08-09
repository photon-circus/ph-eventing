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

### Formatting and lints

```bash
cargo fmt-check
cargo lint               # cargo clippy --all-targets -- -D warnings
```

### Everything at once

```bash
./scripts/ci.sh          # or ./scripts/ci.ps1 on Windows
```

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
and it never enters a normal build. `SeqRing` has one known, documented Miri
deviation; see its module docs before assuming a report is new.

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
- Atomic orderings in `SeqRing` must not be weakened.

## Pull Request Process

1. Create a feature branch from `main`.
2. Make your changes in small, focused commits.
3. Add or update tests as appropriate.
4. Run the full check suite. **This project's CI runs locally** — the GitHub
   Actions workflow is manual-dispatch only, so nothing will check your branch
   for you:
   ```bash
   ./scripts/ci.sh      # POSIX
   ./scripts/ci.ps1     # Windows
   ```
   That runs formatting, clippy, host tests, docs, and the `thumbv6m` /
   `thumbv7em` / `riscv32imac` cross-compilation checks, then prints a
   pass/fail summary.
5. Open a pull request against `main`. Describe what the change does and why.
6. The maintainer will review and may request changes before merging.

## Code Style

- Follow existing patterns (see [CLAUDE.md](CLAUDE.md) § Code Conventions).
- Use `#[inline]` / `#[inline(always)]` on hot-path methods.
- Use `#[must_use]` on types that should not be silently discarded.
- Prefer `const fn` where possible.
- Public APIs must have doc comments.

## License

By contributing, you agree that your contributions will be licensed under the
[MIT License](LICENSE).
