# LatestBuf — engineering record

- **Status:** candidate, PROPOSED — complete admission package on
  `candidate/latest-buf` (draft PR #35); every design decision closed
  (D1–D3, A.1–A.3); awaiting acceptance review and release assembly.
- **Normative sources:** [contract](../proposals/latest-buf-contract.md)
  (clause IDs cited below) · [proposal](../proposals/latest-buf.md) ·
  lane-resident until #35 merges (paths become repo-relative then):
  [evaluation record](https://github.com/photon-circus/ph-eventing/blob/candidate/latest-buf/docs/proposals/latest-buf-evaluation.md) ·
  [measurements](https://github.com/photon-circus/ph-eventing/blob/candidate/latest-buf/docs/proposals/latest-buf-measurements.md) ·
  [joint composition matrix](https://github.com/photon-circus/ph-eventing/blob/candidate/latest-buf/docs/proposals/latest-block-composition-measurements.md).

## 1. Value statement

`LatestBuf<T>` is a three-slot SPSC channel for the freshness-first niche:
the producer never waits, never fails, and never touches bytes the
consumer is reading; the consumer always receives the newest complete
publication, with displaced publications *counted* rather than hidden —
exactly within one wrap span, under-counted beyond it, a boundary X6
states rather than buries (§2). Its soundness argument is ownership transfer **before** access —
provable by Miri with the race detector on — instead of a seqlock's
post-hoc validation, which is the headline property: a lock-free
latest-value channel whose absence of data races is machine-checked, not
argued. It complements `SeqRing` (ordered recent window) rather than
replacing it, and it refuses to be: a FIFO, a delivery guarantee, a
timestamp authority, or a multi-producer/multi-consumer bus.

## 2. Risks and integration concerns

Ordered by how likely they are to surprise an integrator. Contract clause
IDs in parentheses; the clauses are the normative statements.

- **Loss is the designed behaviour, not a fault mode (X1).** Under any
  producer/consumer rate mismatch, intermediate publications are
  displaced. The type is wrong for any consumer that must observe every
  value — that is `EventBuf`'s niche.
- **The skipped count has a documented exactness boundary (C3, X6).**
  `skipped` is exact while fewer than one full generation span (2³²−1
  publications) separates two takes; beyond that it under-counts, and a
  gap of exactly one or more whole cycles reports **zero** — a silent
  under-count. The boundary is a rate × take-interval property (~49.7
  days between takes at 1 kHz; ~72 minutes at 1 MHz), reachable by fault
  *or by deliberate low-cadence design*. The channel deliberately does
  not detect the crossing. If the count is your mission (audit,
  metering), carry a wider producer-assigned sequence in `T` or use a
  saturating counter; if consumer liveness is, use a watchdog.
- **No generic `Source<T>` (D2, X7).** The consumer does not plug into
  generic `Source` pipelines: `try_pop` cannot report displacement, so
  the impl would silently discard loss evidence in normal operation.
  `LatestSource<T>` is the designed contract surface; a caller whose
  domain permits discarding the count writes its own adapter, putting
  that signature in application code. A compile-fail test pins the
  absent impl.
- **Role recovery ends at `Drop` (H1–H4, X8).** Reacquisition after a
  dropped handle continues seamlessly — state is channel-resident, so a
  restarted task resumes with honest gap accounting. But the role is
  held until the handle is *dropped*: an execution context destroyed
  without running destructors, or a forgotten handle, leaves the role
  held (state intact, all clauses holding). There is deliberately no
  out-of-band reset — a forced release could free a role while a live
  handle exists, defeating the exclusive-ownership soundness argument.
  Handle lifetime is the application's property.
- **Memory is three payload slots, always (B3).** Fixed and in `.bss`,
  but real: measured channels are 48/84/420 B for the shipped payload
  matrix, and the block-payload shapes run 136–8,280 B of combined
  channel + builder RAM across the measured grid. State the per-shape
  number for your payload; the docs are required to.
- **Block payloads invert at small N (D3 obligation).** Publishing
  complete blocks through `LatestBuf<Block<T, N>>` beats per-sample
  publication for every 2-byte row and for 8/16-byte samples at N ≥ 32,
  but costs 54%/31% *more* at the 8/16-byte N = 8 corners. Composition
  never exposes a partial block — sample-level freshness inside an
  incomplete window is unobtainable by design.
- **Constrained-target atomics are measured, not free.** On thumbv6m and
  ESP32-S2 the atomic exchange is a portable-atomic critical section;
  the A.1 fast path keeps the *idle* poll out of it (one `Acquire`
  load), and the measured rows (192–276 B thumbv6m, 215–354 B ESP32-S2
  complete-publication code size) are the honest cost basis.

## 3. Technical claims and validation

| Claim | Evidence | Status |
|---|---|---|
| No data race; ownership transfers before access (P6/C6) | 9 focused Miri tests with the race detector **on** (the headline pass, pinned in `scripts/miri.sh`); 5 Loom models incl. slot reuse | Proven |
| No torn or mixed value — one-publication Rust value semantics (P6/C6) | Threaded patterned-payload stress; detector-on Miri | Proven |
| Wait-free producer, bounded consumer, no CAS loop (B1–B3) | Algorithm review (no loops); pinned QEMU cycle regions constant w.r.t. lag and occupancy | Measured |
| Orderings are necessary, not decorative | Mutation runs: all 6 exchange and 4 role-handoff weakenings detected (8 by Loom; 2 relinquish-only mutations by Miri, seeds 53/47) | Proven |
| Exact skipped accounting within one wrap span (C3, G1–G3) | Wrap-boundary unit set mirroring `SeqRing`'s `seq_distance` tests; full-cycle pin (`full_generation_cycle_uses_documented_approximation`) | Pinned |
| Reacquisition continues, never restarts (H4) | Drop-and-reacquire continuation tests; both cross-context role-handoff Loom models; detector-on Miri | Proven |
| Empty poll costs one `Acquire` load, no RMW (A.1) | Loom equivalence model; measured: 7–8 instructions off empty polls, +5–6 on pending Cortex-M3 paths | Measured, selected |
| No `Source<T>` impl can arrive silently (D2, X7) | `compile_fail,E0277` doctest on `Consumer` | Pinned |
| Handles stay `Send + !Sync` (H2) | `compile_fail` doctests on `Producer` and `Consumer` | Pinned |
| Cost claims per target | 11-target code-size matrix incl. ESP32-S2/S3; pinned QEMU 10.0.11 cycle regions; 66-region joint block matrix reproduced across two QEMU versions | Measured |

Full CI at lane acceptance (per-lane tree; the assembled 0.3.0 release matrix supersedes these totals): 76 unit tests, 12 doctests, 6 compile-fail, 94.12%
line coverage, zero skips.

## 4. The record

**Decision history (all closed 2026-08-11, canonical text in contract §9
and proposal Appendix A):**

- **D1 — wrap accounting: (a) documented under-count + (c) payload
  escape hatch.** Option (b), an explicit "wrapped/unknown" state, was
  rejected twice over: it prices wrap detection into every hot-path
  operation on exactly the constrained targets the primitive exists
  for, and "unknown" cannot recover a count that exceeded the counter —
  the accounting mission it gestures at is served by (c). Closure set
  the crate's disclosure standard: risks stated so a downstream user can
  decide *against* the type with full information.
- **D2 — no `Source<T>`: `LatestSink`/`LatestSource` are the designed
  contract surface** (maintainer articulation), solving what the
  existing traits structurally could not. A convenience impl remains an
  additive, adopter-evidence-gated future decision.
- **D3 — payload-agnostic `LatestBuf<T>`; blocks are payloads.** Both
  coupled lanes converged independently; the joint 66-region matrix
  (`8593b4a`) is the acceptance measurement set. Reopening condition:
  a separate block transport must enforce a guarantee composition
  cannot (direct-to-granted-slot filling), behind cycle decisions P/S.
- **A.1 — `Acquire`-load empty-poll fast path, measured and selected.**
  Revert condition registered: code-size regression or equivalence-model
  failure — neither occurred.
- **A.2 — gap arithmetic hazards (off-by-one, wrap over-count)
  validated** by the wrap-boundary unit set; the `raw − 1` skip-zero
  correction is the `SeqRing` lesson applied.
- **A.3 — channel-resident role state, stateless handles** — the same
  sole-role `Send + !Sync` doctrine decision H ratified crate-wide.
  **Persist-on-drop: considered and not selected** — its `Drop`-time
  state copy is a permanent failure surface (a missed write-back
  silently restarts accounting, the exact H4 violation A.3 exists to
  prevent), its L3 model did not isolate the taken-flag handoff, and
  its register-residency claim was never measured. The evaluation
  record §7 preserves its partial evidence as the surviving artifact.

**Review history:** the contract survived multi-round bot arbitration
(PR #24 capture review found A.2/A.3; PR #37 rounds found and fixed a
C3 formula misstatement, an unsatisfiable O2 congruence claim, and an
overclaimed stall-only trigger — every finding confirmed, none
disputed). The corrected text is *stricter* than the drafts it replaced.

**Where the numbers live** (lane-resident until #35 merges):
[`latest-buf-measurements.md`](https://github.com/photon-circus/ph-eventing/blob/candidate/latest-buf/docs/proposals/latest-buf-measurements.md)
(11-target code size, pinned cycles, A.1 comparison, RAM);
[`latest-block-composition-measurements.md`](https://github.com/photon-circus/ph-eventing/blob/candidate/latest-buf/docs/proposals/latest-block-composition-measurements.md)
(the joint D3 matrix); tracking: issues #26/#27, PRs #35/#37.
