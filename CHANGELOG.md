# Changelog

All notable changes to this project will be documented in this file.

## Unreleased
### Added
- `static_spsc!` — declarative bring-up for a `static` SPSC buffer. Expands to a `static`, two
  handle type aliases, and an all-or-nothing `take()`. This is the crate's one concession to
  ergonomics and it is made at **compile time**: no allocation, no indirection, no instruction that
  would not be there if written out by hand.

  It exists because handles are `Send + !Sync` — which is exactly what makes moving a producer into
  an ISR sound — and a `static` requires `Sync`, so handles can **never** live in a `static`
  whatever the constructor looks like. Taking them stays a runtime step permanently. What the macro
  removes is the boilerplate: naming
  `ph_eventing::event_buf::Producer<'static, u32, 64>` in every signature that accepts a handle.

  ```rust
  ph_eventing::static_spsc! {
      pub mod telemetry: EventBuf<u32, 64>;
  }
  fn on_sample(tx: &telemetry::Tx, v: u32) { let _ = tx.push(v); }
  let (tx, rx) = telemetry::take().expect("first take");
  ```

  It generates a module rather than prefixed names because `macro_rules!` cannot concatenate
  identifiers; a proc-macro dependency would have bought nicer names at a cost this crate does not
  pay.
### Added
- `scripts/cycles.sh` now covers all three types, and measures the claim rather than restating it.
  Every `push` is constant with respect to occupancy — `EventBuf` 25/25, `SeqRing` 34/33,
  `RingBuf` 19/20 for empty vs loaded. `SeqRing` lag recovery is **O(1) in the lag**: a consumer
  2×N behind costs 115 instructions, one ~2000 behind costs 114. Deliberately not wired into
  `ci.sh` — it needs a system package (`qemu-system-arm`) that the pinned toolchain does not
  supply, and a check most contributors cannot run would make a green `ci.sh` mean less.
### Added
- `scripts/cycles.sh` — instruction-cost measurement under QEMU, covering the half of the
  determinism claim that code size cannot reach. `push` costs **25 instructions into an empty
  buffer and 25 into a nearly-full one**; the rejected push is 19, `pop` 20 and 14, `len()` 14.
  Constant with respect to occupancy and `N`, which is the property the crate advertises and had
  never actually measured. Requires `qemu-system-arm` (a system package, not supplied by
  `rust-toolchain.toml`); skips cleanly without it.
### Added
- Code-size baseline gate. `scripts/codesize.sh` compares against a committed `baseline.tsv` and
  `ci.sh` runs it as a check, so flash cost is pinned rather than merely measurable. Growth beyond
  +16 bytes fails; shrinkage only reports; a toolchain mismatch `SKIP`s rather than failing, since
  comparing codegen across compilers is noise. Xtensa is measured but never gated, because gating
  it would make the esp-rs fork mandatory. The baseline is committable because it is
  host-independent — verified byte-identical on Windows and Linux for the same pinned rustc across
  all eight gated targets.
### Added
- `const fn new()` for `SeqRing` and `EventBuf` on the normal build, so
  `static BUF: EventBuf<u32, 64> = EventBuf::new();` works. Under `--cfg loom`, both remain
  non-const because Loom's atomics are not const-constructible.

### Changed
- **Breaking:** `SeqRing::new` and `EventBuf::new` reject `N == 0` with a const assertion on the
  host path instead of a runtime `assert!`. `SeqRing::<T, 0>::new()` no longer compiles where it
  previously panicked.

## 0.1.4 - 2026-08-10
### Added
- `try_producer` / `try_consumer` on `SeqRing` and `EventBuf`, returning `Option` when a handle of
  that kind is already active. The panicking `producer` / `consumer` APIs are unchanged.
- `SeqRing` value-returning poll helpers: `Consumer::poll_one_value() -> Option<(u32, T)>` and
  `Consumer::latest_value() -> Option<(u32, T)>`. Hook-based `poll_*` / `latest` are unchanged and
  still maintain the `read + dropped` invariant.
- `EventBuf::Consumer::peek() -> Option<T>` — copy the oldest item without advancing the cursor.

### Changed
- GitHub Actions CI runs again on every push to `master` or a `release/**` branch and on every
  pull request, rather than by manual dispatch only. Added `fmt` and `doc` jobs so the remote
  matrix matches the cheap half of `scripts/ci.*`, and a `concurrency` group that cancels
  superseded PR runs but never cancels a run on `master` or a release branch. Coverage, Miri, and
  Loom stay local-only: coverage is non-deterministic here, and the other two are slow and are the
  only real evidence for the lock-free types. Every document that described CI as manual-dispatch
  was updated with it — a green check is now necessary but still not sufficient, and saying so in
  one place while another claims nothing runs remotely would be worse than either alone.
- The `scripts/*.ps1` twins are gone; `ci.sh`, `miri.sh`, and `loom.sh` are the only runners. On
  Windows they run under the Git Bash that ships with Git for Windows. Keeping two
  implementations of one gate in step is work that only ever gets done on the one you happen to be
  running, so the other drifts silently — and it is the one the next contributor uses. The
  PowerShell versions were at parity when removed (same checks, same `SKIP_EMBEDDED` / `FAIL_FAST`
  / `COVERAGE_FLOOR` knobs), so nothing was lost with them.
- All three scripts now set `CARGO_INCREMENTAL=0`. On Windows, rustc intermittently fails to
  finalize `target/*/incremental` with "Access is denied (os error 5)", and the check it lands on
  moves between runs, so a green matrix reads as a moving defect. A gate that goes red at random
  teaches you to re-run it rather than read it. A fresh CI build gains nothing from incremental.

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
- README / crate docs: static-init guidance updated for SPSC `const fn new`.

### Known issues
- `SeqRing` is a seqlock and has a formal data race that Miri reports as undefined behaviour.
  **This affects downstream tooling:** running `cargo miri test` over a test that drives the ring
  from two threads reports UB inside this crate. It is a deliberate trade — a ring restricted to a
  word-sized payload could hold it in an atomic and be race-free; accepting any `T: Copy` is what
  rules that out. The `seq_ring` module docs give the alternatives and why each was rejected.
  `EventBuf` is unaffected and passes Miri with the detector on, but applies backpressure rather
  than overwriting, so it is not a drop-in replacement.
- `SeqRing` drops a bounded number of extra entries once per `u32` sequence wrap, newly documented
  above. This is a data-loss bound, not a soundness issue, and it is unchanged from 0.1.3 — only
  its documentation is new.

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
