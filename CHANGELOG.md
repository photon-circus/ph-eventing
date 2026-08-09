# Changelog

All notable changes to this project will be documented in this file.

## Unreleased
### Documentation
- `SeqRing`: documented the extra drops that occur at the `u32` sequence wrap. Slots are addressed
  by `(seq - 1) % N` while `push` skips the reserved sequence `0`, so a cycle is `2^32 - 1`
  sequences and the slot walk only lines up across the wrap when `N` divides `2^32 - 1`. Once per
  wrap this drops exactly one entry for a power-of-two `N`, none for a divisor of `2^32 - 1`, and
  up to `N - 1` otherwise (`N = 96` drops 33, `N = 121` drops 58). The affected reads fail their
  sequence check and are counted in `PollStats::dropped`, so this is a data-loss bound and not a
  soundness issue — no stale or torn value is returned, and `read + dropped` still accounts for
  every published item. Every other statement in the docs implies lag `<= N` is lossless, which
  was true apart from this one boundary. `README.md` now recommends a power-of-two `N` for
  `SeqRing` rather than only noting that one is not required.
- `.github/dependabot.yml`: corrected the guidance for validating a Dependabot PR. It pointed at
  `./scripts/ci.sh`, which never invokes an action and so cannot validate an actions bump. It now
  names the two checks that do: resolving the pinned SHA against the tag it claims, and dispatching
  the workflow after merge.

## 0.1.3 - 2026-08-09
### Fixed
- `SeqRing`: the producer now invalidates a slot's sequence before overwriting it, so the consumer
  can no longer observe a fresh value under a stale sequence number.
- `SeqRing` and `EventBuf`: the consumer's value access is now bracketed by an `Acquire` **fence**
  rather than an `Acquire` load. An acquire load is one-way and left the value access free to sink
  below the guard that validates it.
- `SeqRing`: drop accounting across the `u32` wrap was over-counted by one, because raw
  `wrapping_sub` includes the reserved sequence `0` that `push` skips. Sequence arithmetic now
  goes through `seq_distance`.
- `SeqRing`: `Consumer::dropped` saturates instead of overflowing. `usize` is 32 bits on every
  target this crate ships to, so the counter and the sequence space are the same width — overflow
  panicked in debug and wrapped silently in release.
- `EventBuf::len` returns a consistent `(tail, head)` snapshot and can no longer report a value
  above `capacity()` when the consumer advances mid-read. It is also wait-free: bounded retries
  followed by a clamped estimate, replacing an unbounded loop.
- `SeqRing`: slot copies are volatile and held as `MaybeUninit<T>` until validated, so a copy that
  raced with an overwrite is discarded as raw bytes rather than materialising as an invalid `T`.

### Added
- Threaded stress tests for both SPSC types, and Miri and Loom verification scripts
  (`scripts/miri.*`, `scripts/loom.*`). Loom is a dev-dependency gated on `--cfg loom`; the
  library keeps zero runtime dependencies.
- Local CI runner (`scripts/ci.*`) covering fmt, clippy, tests, docs, and the `thumbv6m` /
  `thumbv7em` / `riscv32imac` targets.
- `cargo-deny` policy (`deny.toml`) covering security advisories, licences, banned and duplicate
  crates, and dependency sources. Included in the local CI run when installed, reported as skipped
  when not. The licence allow-list is `MIT` / `Apache-2.0` only.
- Coverage gate in local CI via `cargo llvm-cov`, with a 90% line floor. Presence-gated, so a
  missing tool reports as skipped rather than passing silently.
- `.gitignore` entries for editor, IDE, and AI/agent tooling state. These were previously covered
  only by individual developers' global git config, leaving a fresh clone free to commit them.
- `AGENTS.md` as the canonical guidance for coding agents; `CLAUDE.md` is now a pointer to it.
- Crate-level lints enforced at build time: `unsafe_op_in_unsafe_fn = "deny"`, plus warnings for
  `missing_docs`, `missing_debug_implementations`, `unreachable_pub`,
  `clippy::undocumented_unsafe_blocks`, and `clippy::{alloc,std}_instead_of_core`. Enabling
  `missing_docs` surfaced seven undocumented public items, now documented.
- `documentation` and `homepage` fields in the manifest.
- `RELEASING.md` — release checklist covering version choice under pre-1.0 semver, the
  verification runs, what ships, and yank semantics.

### Changed
- The published crate is now defined by an `include` allowlist instead of an `exclude` denylist.
  Every new top-level file previously shipped by default until someone remembered to exclude it —
  `deny.toml`, `scripts/`, and `AGENTS.md` each had to be added after the fact. `LICENSE` and
  `README.md` are included; everything not named is not published.
- GitHub Actions runs on manual dispatch only. Local CI is the primary gate.

### Fixed (repo hygiene)
- Local CI and the workflow now exercise current stable. `rust-toolchain.toml` pins 1.92.0 and
  overrides whatever a setup action installs, so every check silently ran the MSRV — including the
  job labelled "stable". Jobs that need a different compiler now use an explicit `cargo +stable`.
- Feature combinations are checked individually in CI, and a `build.rs` guard now explains the one
  combination Cargo cannot express: `portable-atomic-unsafe-assume-single-core` and
  `portable-atomic-critical-section` are mutually exclusive, so `--all-features` cannot work. The
  guard has to be a build script — a `compile_error!` in `src/lib.rs` is unreachable, because
  portable-atomic's own guard fires before this crate is compiled.
- The GitHub Actions workflow declares `permissions: contents: read` instead of inheriting the
  repository default token scope.
- Workflow actions are pinned to commit SHAs rather than mutable tags, with Dependabot configured
  for the `github-actions` ecosystem so pinning does not turn into rot.
- `[package.metadata.docs.rs]` names `portable-atomic`, so docs.rs documents the optional path.
  The usual `all-features = true` is unusable here — it would fail the docs build.
- `src/loom_tests.rs` is excluded from the published crate. It is gated on `--cfg loom` and
  references a dev-dependency consumers do not have, so it could never compile for them.
- Added issue and pull-request templates, `CODEOWNERS`, and a Dependabot config. The PR template
  states that a `SKIP` in a local CI run is not a pass.

### Known issues
- `SeqRing` is a seqlock and has a formal data race that Miri reports as undefined behaviour.
  **This affects downstream tooling:** running `cargo miri test` over a test that drives the ring
  from two threads reports UB inside this crate. It is a deliberate trade — a ring restricted to a
  word-sized payload could hold it in an atomic and be race-free; accepting any `T: Copy` is what
  rules that out. The `seq_ring` module docs give the alternatives and why each was rejected.
  `EventBuf` is unaffected and passes Miri with the detector on, but applies backpressure rather
  than overwriting, so it is not a drop-in replacement.

## 0.1.2 - 2026-02-08
### Added
- `Sink<T>` trait for event producers, implemented by `RingBuf`, `seq_ring::Producer`, `event_buf::Producer`.
- `Source<T>` trait for event consumers, implemented by `seq_ring::Consumer`, `event_buf::Consumer`.
- `Link<In, Out>` blanket trait for types that are both `Sink` and `Source`.
- `forward(src, snk, max)` utility to bridge any `Source` into any `Sink`.
- `EventBuf<T, N>` — bounded SPSC event buffer with backpressure (`push` returns `Err` when full).
- `event_buf::Producer` and `event_buf::Consumer` handles.
- `RingBuf<T, N>` — single-owner, stack-allocated ring buffer (no atomics, no unsafe).
- `RingIter` iterator with `ExactSizeIterator` support.
- `Default` implementation for `RingBuf` and `EventBuf`.
- `Debug` implementations for all public types (`RingBuf`, `RingIter`, `SeqRing`, `EventBuf`, all `Producer`/`Consumer` handles).
- `IntoIterator` implementation for `&RingBuf`.
- `capacity()` method on `RingBuf`, `SeqRing`, and `EventBuf`.
- Re-export `RingBuf`, `EventBuf`, `Sink`, `Source`, `Link` from crate root.

### Fixed
- `event_buf::Producer` and `event_buf::Consumer` are now `Send` (was accidentally `!Send` due to `PhantomData<*mut ()>`).
- `RingBuf::new()` now panics on `N == 0` with a clear message (matches `SeqRing` and `EventBuf` behaviour).

### Changed
- Removed `Producer` and `Consumer` re-exports from crate root to avoid ambiguity between `seq_ring` and `event_buf` types. Access them via `seq_ring::Producer` / `event_buf::Producer` etc.

## 0.1.1 - 2026-02-04
### Added
- Optional `portable-atomic` support for targets without 32-bit atomics (e.g. `thumbv6m-none-eabi`).
- `Default` implementation for `SeqRing<T, N>` (equivalent to `SeqRing::new()`).

### Changed
- CI now uses `dtolnay/rust-toolchain` and pins clippy to the MSRV toolchain.
- Embedded CI checks include `thumbv6m-none-eabi` with the portable-atomic feature enabled.
