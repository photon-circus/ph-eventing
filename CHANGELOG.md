# Changelog

All notable changes to this project will be documented in this file.

## Unreleased
### Fixed
- `CountedSignal::increment`: replace the load-and-skip-on-`MAX` short-circuit
  with a no-op RMW (`fetch_or(0)`) re-read that treats observed `MAX` as
  maybe-stale — an RMW observes the latest value in modification order, so a
  `MAX` re-read confirms saturation and anything else proceeds into the
  post-take epoch, with no compare-exchange and no LR/SC retry loop on
  RISC-V. A completed `take_count` followed by a later `increment` can no
  longer lose the occurrence under Relaxed observation (contract T3/A1). Loom
  litmus: `counted_signal_post_take_increment_observes_reset_epoch`.
### Documentation
- CountedSignal engineering record (`docs/records/counted-signal.md`) — Track 1 acceptance package: value statement, integrator risks (saturation, sole-producer exclusivity as load-bearing for no-wrap), claims×evidence including the pinned Cortex-M3 rows (confirmed 8 / 7 after the sentinel-RMW fix), and the H closure record.
- CountedSignal Cortex-M3 cycle rows re-confirmed at 8 / 7 after the sentinel-RMW
  fix (`./scripts/verify.sh cycles`, QEMU 10.0.11); pending-remeasure markers cleared.
- CountedSignal contract B1: fixed instruction sequence with a no-op RMW
  sentinel re-read — no compare-exchange, no retry; proposal §3.1
  linearization claim corrected; README Sink/Source claim qualified to
  payload-buffer handles.
### Added
- `CountedSignal`: a payload-free SPSC counter with a bounded
  `increment`, atomic `take_count`, exact `u32` saturation, and observable
  saturation. Loom models pin take partitioning, the saturation-boundary
  interleaving, and the post-take stale-`MAX` litmus; its frozen contract maps
  citable clauses to unit, threaded, Loom, Miri, code-size, and QEMU evidence.
  Exact bounded saturation depends on retaining a sole `Send + !Sync` producer
  handle. Cortex-M3 instruction counts remain 8 / 7 after the sentinel-RMW
  path (`./scripts/verify.sh cycles`).
### Removed
- **Breaking:** the panicking `SeqRing::{producer, consumer}` and
  `EventBuf::{producer, consumer}`, deprecated since 0.2.0 with removal scheduled for 0.3.0.
  `try_producer()` / `try_consumer()` (added in 0.1.4) are now the only handle-acquisition
  API: on the targets this crate exists for a panic is a reset, and the panic machinery costs
  flash — a 0.2.0 code-size probe showed no panic strings reach the binary when only the
  `try_*` constructors are used. The `double_producer_panics` / `double_consumer_panics` tests
  are gone with the behaviour they pinned; SPSC enforcement (a second acquisition is refused
  while a handle is live) remains pinned by `try_producer_and_try_consumer` in both modules,
  and the Loom models now drive the `try_*` path directly, so the proven orderings are the
  shipped orderings.

### Documentation
- Cycle decisions **P** and **S** are closed (2026-08-11). **P**: BlockBuf's publication
  foundation is **Copy composition** for the supported shapes — no library-wide threshold is
  claimed; the per-shape measured rows are the budget statement, the nine-shape matrix rides
  into the release baselines at promotion, and the docs owe integrators the double-copy
  hazard guidance (make the builder the DMA target, or publish from task context). **S**:
  SlotPool is **deferred**, not rejected — full evaluation evidence banked on its branch,
  draft PR closed, and an adopter-gated reopening trigger registered (a measured budget
  breach, a direct-to-granted-slot requirement, or a standalone zero-copy adopter). Every
  decision in the 0.3.0 cycle is now settled.
- **Engineering records established** (`docs/records/`, maintainer decision 2026-08-11): one
  enduring document per shipped type — a short value statement, risks and integration
  concerns in integrator terms, technical claims mapped to their validating evidence, then
  the working record (decisions, measurements, rejected alternatives). Not rustdoc
  duplication: rustdoc says how to use a type; the record says why it is trustworthy and
  what it cost. A candidate lane owes its record as part of its acceptance package;
  [`records/latest-buf.md`](docs/records/latest-buf.md) is the exemplar. The 0.2.0 types
  receive theirs when next materially touched.
- `LatestBuf` contract: decision **D1** (wrap-ambiguity policy) is closed as options
  (a) + (c) — `skipped` is one formula (wrap-aware distance minus one, saturated at zero):
  exact within one wrap span, a documented under-count beyond it, and callers whose
  requirement is the count itself are pointed at a wider producer-assigned payload sequence
  or a saturating counter. The wrap family (C3, C5, O2) now carries its beyond-span text,
  and a new non-promise **X6** states the limitation for adopters: the boundary is a
  rate × take-interval property crossed exactly when a full span of publications separates
  two takes, the channel deliberately does not detect the crossing, and a full-cycle gap
  reports zero. Option (b) — an explicit
  "wrapped/unknown" state — is rejected on the record: it prices wrap detection into every
  hot-path operation and still cannot recover the lost count.
- `LatestBuf` contract: decision **D2** (`Source<T>` policy) is closed as the proposal's
  option 1 — the consumer does not implement `Source<T>`; `LatestSink`/`LatestSource` were
  designed as the type's contract surface, solving what the existing traits structurally
  could not (`try_pop` cannot report displacement, and displacement is the type's *designed*
  overload behaviour, making the `RingBuf::pop` rejection apply with more force). New
  non-promise **X7** states the substitution limitation honestly: no generic pipeline
  composition without a caller-written adapter — discarding loss evidence is an application
  decision, never the transport's. A convenience `Source` impl remains an additive,
  adopter-evidence-gated future decision, and a compile-fail pin keeps it from arriving
  silently.
- `LatestBuf` contract: decision **D3** (first deliverable form) is closed as the convergent
  answer from both coupled lanes — payload-agnostic `LatestBuf<T>`, with a complete block as
  a payload (`LatestBuf<Block<T, N>>` latest / `EventBuf<Block<T, N>, Q>` queued / caller
  policy for drop-new) and no separate `LatestBlockBuf`. The joint composition matrix is the
  acceptance measurement set for both shapes; the closure binds a documentation obligation
  on the block-payload surfaces (per-shape RAM, the small-`N` cost inversion, and the
  no-partial-block limitation) and registers the only reopening condition: a separate block
  transport must enforce a guarantee composition cannot, behind cycle decisions P/S. All
  three contract decision points are now closed.
- `LatestBuf` proposal: review caveat **A.3** (handle-state continuation) is closed as
  channel-resident role state with stateless handles — the crate's stateless-handle
  precedent and the sole-role doctrine of decision H, validated by drop-and-reacquire
  continuation tests, both cross-context role-handoff Loom models, detector-on Miri, and
  all four role-handoff ordering-mutation detections. Persist-on-drop is considered and
  not selected (Drop-time state copy is a permanent failure surface; its L3 model does not
  isolate the taken-flag handoff; register residency unmeasured), and narrowing H2's
  registered condition was not met. New contract non-promise **X8** states the
  role-recovery boundary for integrators: reacquisition requires the previous handle's
  drop, handle lifetime is an application property, and there is deliberately no
  out-of-band role reset because a forced release would break the exclusivity soundness
  rests on — the facts of the exchange, informing downstream design without prescribing it.
- `RingBuf::new`'s docs no longer mention a `pop` method the type does not have — `pop` was
  deliberately rejected (its data loss would be unreportable under overwrite; see the worked
  rejection in AGENTS.md). Found by review on the release PR just after `0.2.0` published, so
  the `0.2.0` docs on docs.rs carry the sentence; the fix rides out with the next publish.

## 0.2.0 - 2026-08-10

**What this release delivers.** 0.1.x was correct: verified lock-free buffers with no way to
show what they cost. 0.2.0 makes the costs part of the contract. Buffers now const-construct
into `.bss` — 268 bytes, zero flash, zero startup copy, measured on 11 targets across 4 ISA
families — and the panicking constructors are deprecated in favour of `try_*`, so no panic
machinery reaches the binary. The hot paths carry measured instruction counts instead of
adjectives: every `push` costs the same empty or full, and a `SeqRing` consumer 2000 sequences
behind recovers in the same 115 instructions as one 16 behind. What was true before is now
*checked*: a code-size baseline gates every `ci.sh` run, the instruction counts are
reproducible by anyone via one pinned Docker environment, and the one ergonomic gap that
couldn't be closed at runtime got a compile-time answer (`static_spsc!`) instead of a runtime
cost. Two features were built, measured, and rejected for this release because the numbers said
no — that reasoning ships in `AGENTS.md`, because on this crate the measurement discipline is
the feature.

### Added
- `scripts/verify/Dockerfile` + `./scripts/verify.sh` — the **reference verification
  environment**. The crate's claims are only claims if someone else can reproduce the evidence,
  and three tools sit outside what `rust-toolchain.toml` can pin: QEMU (instruction counts are
  deterministic per build, not across builds), the Miri nightly (a bare `+nightly` drifts
  daily), and the optional `ci.sh` gates (cargo-deny, cargo-llvm-cov, stable). The image pins
  all three; `./scripts/verify.sh` runs the full local matrix — `ci.sh` with zero SKIPs,
  Miri, Loom, cycles — inside it. Crate sources are preloaded, so only the cargo-deny advisory
  refresh needs network. A prebuilt copy is published as
  [`stevegiacomelli/ph-eventing-verify`](https://hub.docker.com/r/stevegiacomelli/ph-eventing-verify),
  each `0.x.y` tag frozen as that release's evidence environment.
  Every run stamps its versions. Deliberately guarded against
  running in hosted CI: the expensive checks were taken off the remote pipeline on purpose, and
  reversing that should be an explicit, reviewed decision.
- `scripts/miri.sh` accepts `MIRI_TOOLCHAIN` (e.g. `nightly-2026-08-08`) and prints the exact
  toolchain every run, so a Miri verdict is always paired with the nightly that produced it.

### Changed
- **Crate metadata now states what the crate is actually for.** The description led with
  "stack-allocated ring buffers for no-std embedded targets", which is the entry fee rather than
  the value — many crates clear that bar. It now leads with determinism and measured cost.
  `realtime` replaces `embedded` in the keywords (`embedded` is already a category and appears in
  the description, while nothing signalled the determinism guarantee), and `data-structures` is
  added to the categories.

### Documentation
- `AGENTS.md` and `CONTRIBUTING.md` record the design priority order — predictability, then
  efficiency, then regression resistance, then ergonomics — and the rule that follows from it:
  when the correct design is awkward, recover the ergonomics at **compile time** (macros,
  type-state, `const fn`) rather than paying for them at runtime. Includes the worked rejections
  of `RingBuf::pop`/`Source` and `try_split`, since the reasoning is the guidance.
### Added
- `RingBuf::new()` is a `const fn`, so a ring can be const-initialised inside an interior-mutability
  wrapper — `static LOG: Mutex<RefCell<RingBuf<u32, 64>>> = Mutex::new(RefCell::new(RingBuf::new()));`
  — which previously needed a `StaticCell` and a runtime init step. A bare `static RingBuf` is of
  little use on its own, since every mutator takes `&mut self`.

### Added
- `compile_fail` doctest covering `RingBuf`'s `N == 0` rejection, restoring the coverage that
  `zero_capacity_panics` provided before the check moved to compile time. The expected error code
  is pinned (`compile_fail,E0080`) so it cannot pass for the wrong reason.
### Fixed
- `RingBuf` accessors no longer overflow at very large `N`. The slot index formed an intermediate
  up to `3N`; a zero-sized `T` makes `RingBuf<(), { usize::MAX }>` constructible, so `get`/`latest`
  panicked there in an overflow-checking build, breaking the no-panic guarantee. The index now
  steps back from `head` and nothing exceeds `N`. Found by automated review on the PR.
- `RingBuf::latest` now routes through the same private `index` helper as every other read. The
  open-coded calculation it replaced was correct, but it was a second independent slot computation,
  so a future change to the cursor representation could fix `get` and silently invalidate `latest`.

### Changed
- **Breaking:** `RingBuf::new()` rejects `N == 0` with a const assertion instead of a runtime
  `assert!`. `RingBuf::<u32, 0>::new()` no longer compiles where it previously panicked, and the
  `zero_capacity_panics` test is gone because the case can no longer be written.
- **Breaking:** `RingBuf` no longer requires `T: Default`. Slots are stored as `MaybeUninit<T>`
  and only live entries are read. Relaxing a bound is compatible for callers, but `RingBuf` is no
  longer free of `unsafe`, which was a documented property of the type — so it is recorded as
  breaking rather than as a quiet improvement.
- The code-size gate could report a pass without gating anything. Three defects found by automated
  review, all now demonstrated fixed: a skipped gate exited 0 and `ci.sh` recorded **PASS** despite
  printing SKIP; a baseline row with no matching measurement printed `MISSING` and still passed;
  and `--bless` would overwrite the baseline after a partial run, silently deleting the rows it
  could not measure. The script now exits `2` for "could not run", `ci.sh` maps that to a real
  `SKIP` row with a summary warning, missing rows fail, and `--bless` refuses after any skip or
  failure.
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
- `compile_fail` doctests covering the `N == 0` rejection on `SeqRing::new` and `EventBuf::new`.
  The check is a const assertion, so there is no runtime panic to catch and the case cannot be
  written as a `#[test]` — this restores the coverage that `zero_capacity_panics` used to provide.
  The expected error code is pinned (`compile_fail,E0080`) so the test cannot pass for the wrong
  reason. No dev-dependency was added; rustdoc does this natively.
- `scripts/cycles.sh` now covers all three types, and measures the claim rather than restating it.
  Every `push` is constant with respect to occupancy — `EventBuf` 25/25, `SeqRing` 34/33,
  `RingBuf` 20/20 for empty vs loaded. `SeqRing` lag recovery is **O(1) in the lag**: a consumer
  2×N behind and one ~2000 behind both cost 115 instructions. (Measured in the reference
  environment — see `scripts/verify/Dockerfile`; a different QEMU build was observed to shift two
  regions by ±1 instruction.) Deliberately not wired into `ci.sh` — it needs a system package
  (`qemu-system-arm`) that the pinned toolchain does not supply, and a check most contributors
  cannot run would make a green `ci.sh` mean less.
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

### Deprecated
- **`SeqRing::producer` / `consumer` and `EventBuf::producer` / `consumer`**, in favour of the
  `try_*` variants added in 0.1.4. Scheduled for removal in 0.3.0. On a microcontroller a panic is
  a reset, and the panic machinery costs flash: a code-size probe shows no panic strings reach the
  binary when only the `try_*` constructors are used. The shorter, more discoverable name being
  the hazardous one is the inversion this corrects. The deprecated methods are unchanged and still
  tested; only the recommendation moves.
### Added
- `const fn new()` for `SeqRing` and `EventBuf` on the normal build, so
  `static BUF: EventBuf<u32, 64> = EventBuf::new();` works. Under `--cfg loom`, both remain
  non-const because Loom's atomics are not const-constructible.

### Changed
- **Breaking:** `SeqRing::new` and `EventBuf::new` reject `N == 0` with a const assertion on the
  host path instead of a runtime `assert!`. `SeqRing::<T, 0>::new()` no longer compiles where it
  previously panicked.

### Known issues
- `SeqRing` is a seqlock and has a formal data race that Miri reports as undefined behaviour.
  **This affects downstream tooling:** running `cargo miri test` over a test that drives the ring
  from two threads reports UB inside this crate. It is a deliberate trade — a ring restricted to a
  word-sized payload could hold it in an atomic and be race-free; accepting any `T: Copy` is what
  rules that out. The `seq_ring` module docs give the alternatives and why each was rejected.
  `EventBuf` is unaffected and passes Miri with the detector on, but applies backpressure rather
  than overwriting, so it is not a drop-in replacement. Unchanged from 0.1.x.
- `SeqRing` drops a bounded number of extra entries once per `u32` sequence wrap (documented since
  0.1.4). A data-loss bound, not a soundness issue; unchanged.

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
