# Contributing to ph-eventing

Thank you for your interest in contributing to ph-eventing! This document provides guidelines and information for contributors.

## Getting Started

1. Fork the repository
2. Clone your fork locally
3. Create a new branch for your changes

## Development Requirements

- Rust 1.92.0 or later (see `rust-toolchain.toml`)
- Run `cargo build` to build the library
- Run `cargo test` to run the test suite

## Code Guidelines

### Key Invariants

Please preserve these core invariants when making changes:

- **No-std compatibility:** Never add std dependencies to library code
- **Zero allocation:** No heap allocations in the library
- **Sequence 0 reserved:** Sequence 0 is reserved for "empty" state
- **Producer never blocks:** Producer never blocks or returns errors
- **Accurate drop tracking:** Consumer tracks dropped items accurately
- **Torn-read prevention:** Double-sequence-check pattern must be maintained

### Style

- Use `#[inline]` / `#[inline(always)]` on hot-path methods
- Use `#[must_use]` on types that shouldn't be silently discarded
- Use `const fn` for compile-time calculations
- Suffix internal methods with `_inner`
- Suffix accumulator fields with `_accum`

## Testing

All changes should include appropriate tests. Run the test suite with:

```bash
cargo test
```

For embedded target verification:

```bash
cargo check --target thumbv7em-none-eabi
```

## Submitting Changes

1. Ensure all tests pass
2. Ensure your code follows the existing style
3. Write clear commit messages
4. Open a pull request with a clear description of your changes

## License

By contributing to ph-eventing, you agree that your contributions will be licensed under the MIT License.
