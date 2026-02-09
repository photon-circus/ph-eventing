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

The crate is `#![no_std]`. Before opening a PR, verify it compiles for at
least one embedded target:

```bash
cargo check --target thumbv7em-none-eabi
```

### Formatting and lints

```bash
cargo fmt --check
cargo clippy --all-targets -- -D warnings
```

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
4. Run the full check suite:
   ```bash
   cargo fmt --check
   cargo clippy --all-targets -- -D warnings
   cargo test
   cargo check --target thumbv7em-none-eabi
   ```
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
