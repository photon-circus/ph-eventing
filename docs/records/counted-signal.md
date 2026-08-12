# CountedSignal — engineering record

- **Status:** candidate, PROPOSED — complete admission package on
  `candidate/counted-signal` (PR #33); accepted contract; shared
  handle decision H closed on this lane's evidence; MAX short-circuit
  exactness fix landed; awaiting acceptance review and release assembly.
- **Normative sources:** [contract](../proposals/counted-signal-contract.md)
  (clause IDs cited below) · [proposal](../proposals/counted-signal.md) ·
  measurements inline in proposal §3.1 (eight-target code size; pinned
  Cortex-M3 cycles via `./scripts/verify.sh cycles`: 8 hot path, 9
  saturated sentinel arm, 9 take on the assembled 0.3.0 tree — the
  merged probe binary carries the take two instructions higher than the
  per-lane tree's 7; the increment arms are unchanged).

## 1. Value statement

`CountedSignal` is a saturating SPSC counter for the multiplicity-without-
payload niche: the producer records identical, payload-free occurrences
with a statically bounded increment; the consumer atomically takes the
count for one interval and clears it. Every completed increment
linearizes exactly once into a take interval; when the exact range is
exhausted the signal saturates observably rather than wrapping — the
same `dropped_accum` honesty the crate already requires of overload
accounting. Its soundness argument for exact, wrap-free saturation is
**sole-producer ownership**: between the producer's reads and RMWs the
consumer can only reset, so the RMW cannot wrap — and every path is a fixed
instruction sequence (the saturation sentinel is re-read via a no-op
`fetch_or(0)` RMW, never a compare-exchange, so no LR/SC retry on RISC-V).
It complements `EventFlags` (presence) rather than replacing it, and it
refuses to be: a payload channel, an ordering authority, a multi-producer
bus, or an unbounded exact total past saturation.

## 2. Risks and integration concerns

Ordered by how likely they are to surprise an integrator. Contract clause
IDs in parentheses; the clauses are the normative statements.

- **Saturation loses exact excess (A2, X3).** A saturated snapshot
  reports at least `u32::MAX` increments in the interval and that
  exactness was lost; the excess itself is deliberately unrecoverable.
  If your mission needs an unbounded exact total, this type is wrong —
  widen the accounting in the application, or take often enough that
  saturation is a fault rather than a design point.
- **Sole producer is load-bearing, not style (H1–H2, X4, H-decision).**
  The no-wrap / B1 proof depends on one active producer: two raisers
  could both observe `u32::MAX - 1` and the second commit would wrap.
  Shareable or multi-producer signalling needs a **separate** type,
  contract, algorithm, and evidence case — not a weakened reading of
  this one. `&self` on the hot path is a receiver choice; it does not
  permit sharing a handle (`Send + !Sync` remains).
- **No payload, no per-occurrence identity (X1, X2).** Only the count in
  a take interval is retained. Ordering between individual increments
  is intentionally absent; unrelated data gains no publication fence
  from an increment. Callers that need payload or FIFO delivery want
  `EventBuf` / `SeqRing`, not this niche.
- **Role recovery ends at `Drop` (H1–H3).** Reacquisition after a dropped
  handle continues from channel-resident state without reset — I/T/A
  clauses keep holding. An execution context destroyed without running
  destructors, or a forgotten handle, leaves the role held. There is
  deliberately no out-of-band forced release.
- **Algorithmic bounds are not wall-clock claims (B1–B3, X5).**
  `increment` is a fixed sequence of source-level atomics with no
  algorithmic retry; `take_count` is one source-level `swap`. On
  exclusive-monitor Arm each single RMW is an LDREX/STREX pair that can
  repeat only when an intervening event claims the word (B1's per-ISA
  disclosure); the measured rows are uncontended. Neither waits or
  panics on the hot path. Instruction and latency numbers are
  environment-specific measurements; cite them with the pinned
  toolchain and QEMU stamp, never as universal cycle truths.
- **Constrained-target atomics are measured, not free.** On thumbv6m /
  thumbv8m.base the RMW path uses the portable-atomic single-core
  backend; the gated code-size rows (proposal §3.1) are the honest
  cost basis for those cores.

## 3. Technical claims and validation

| Claim | Evidence | Status |
|---|---|---|
| Exact accumulate + take clears; no fabricated/duplicated counts (I1, T1, T4, A1) | `increments_accumulate_and_take_clears`; threaded `concurrent_takes_do_not_lose_increments` sum check (100,000 increments) | Proven |
| Saturates at `u32::MAX`, never wraps; sticky until take (I2–I3, T2, A2–A3) | `saturates_instead_of_wrapping`; Loom `counted_signal_saturation_boundary_is_linearizable` | Proven |
| Concurrent increment belongs to exactly one take interval (T1–T3, A1) | Loom `counted_signal_take_partitions_increments`; threaded take stress | Proven |
| Post-take increment after saturated take is not dropped by a stale MAX observe (T3, A1) | Loom `counted_signal_post_take_increment_observes_reset_epoch` (Relaxed gate only) | Proven |
| Bounded source-level hot paths, no algorithmic retry: load + `fetch_add`; sentinel no-op RMW re-read + ≤1 follow-up `fetch_add`; `swap(0)` take — per-ISA realisation per B1 (LDREX/STREX pairs on exclusive-monitor Arm may repeat under contention, so this is not a wait-free or consumer-independent-latency claim) (B1–B2) | Source review; riscv32imac disassembly shows lw/amoor.w/amoadd.w with **no lr.w/sc.w**; eight-target code size re-blessed post-RMW; Cortex-M3 cycles 8 (hot) / 9 (saturated arm, probe-seeded region) / 9 (take, assembled 0.3.0 tree; 7 on the per-lane tree — merged-binary codegen, not an algorithm change); the stale-MAX third arm is the saturated arm + one `fetch_add` by construction | Measured |
| No panic reachable from hot paths (B3) | Source review; normal, Miri, Loom, and embedded-target executions | Proven |
| Sole-role exclusive handles; reacquisition continues state (H1, H3) | `handles_are_exclusive_and_reusable_after_drop` | Proven |
| Handles are `Send + !Sync` (H2) | `handles_are_send`; compile-fail doctests pin `!Sync` on both roles | Pinned |
| Constructible in static storage (H4) | `const_new_works_in_static_context` | Proven |
| Cost claims per target | Eight-target gated code-size rows (proposal §3.1), re-blessed after the sentinel-RMW change | Measured |

Full CI at lane acceptance (per-lane tree; the assembled 0.3.0 release matrix supersedes these totals) (`0a22ada` admission package; `f26d4c3` H
finalization): complete pinned `./scripts/verify.sh` matrix with zero
skips — unit tests, 11 doctests, 5 compile-fail, coverage, Miri host
and proxy targets, Loom models, eight-target code size, embedded
checks, QEMU cycles. Cycles on the assembled 0.3.0 tree: 8 / 9, with the
saturated sentinel arm added as its own probe-seeded region at 9;
full matrix not re-run for this cycles-doc follow-up.

## 4. The record

**Decision history (closed on `candidate/counted-signal`, canonical text
in contract §8 and proposal §3.1–§3.2):**

- **Bounded exact saturation: sole-producer `fetch_add` with stale-MAX
  confirmation.** Below `MAX`, one Relaxed load and one Relaxed
  `fetch_add`. Observed `MAX` is confirmed with a no-op RMW re-read
  (`fetch_or(0)`) — an RMW observes the latest value in modification
  order, so `MAX` proves saturation and anything else falls through to
  one `fetch_add` into the post-take epoch (fixed source-level
  sequence, no algorithmic retry). The first accepted form confirmed
  with a bounded `compare_exchange(MAX, MAX)`; review showed a strong
  CAS lowers to an unbounded LR/SC loop on RISC-V, and the no-op RMW
  replaced it.
  Rejected for this type: a load-and-skip-on-`MAX` short-circuit (fails
  T3/A1 after a completed take under Relaxed observation), an unbounded
  CAS loop (fails B1), silent wrap (fails A2/I3 and the crate's
  saturate-don't-wrap convention), and shareable/multiple raisers
  (invalidates the intervening-writer argument). Closure set the
  disclosure standard: the no-wrap proof is an ownership proof — state
  it so a multi-producer caller can decide *against* the type.
- **H — sole-role `Send + !Sync` handles with `&self` hot-path ops
  (`f26d4c3`).** Accepted for CountedSignal and EventFlags on this
  lane's evidence. For CountedSignal the choice is correctness, not API
  taste: producer exclusivity is part of the no-wrap / B1 proof. A
  future multi-producer signal must be a separate type. LatestBuf's
  A.3 later closed on the same sole-role doctrine (PR #37), so the
  crate now states the handle model once.
- **`u32::MAX` sentinel in a one-word `CountSnapshot`.** Full exact
  range below the sentinel; `is_saturated()` observes loss of precision
  in the take that clears it; no second sticky atomic.
- **`swap(0)` take, relaxed ordering.** Counts only — no payload
  publication to order. Atomic modification order partitions each
  increment into exactly one take epoch (T3).
- **Width and shape: one `u32` counter.** Matches the atomic shim and
  every shipped 32-bit target. Multi-class use composes a fixed array
  of signals; a dedicated multi-counter or dynamic registry needs its
  own caller and evidence. Width genericity would create
  target-dependent contracts.

**Review history:** issue #30 closed with the lane PROPOSED; PR #33
carries the admission package. Bugbot high finding (stale MAX load drops
increments) agreed and fixed with a sentinel re-read + Loom litmus — first
as a bounded CAS, then (Codex P1) as the no-op RMW after review showed the
CAS lowers to an unbounded LR/SC loop on RISC-V. Codex
P2 (qualify README Sink/Source claim) agreed. Cross-lane handle
decision recorded against #29; cycle decision matrix remains #26.
Contract clause IDs (I/T/A/B/H/X) are load-bearing for tests and models
— do not renumber.

**Where the numbers live:** proposal §3.1 (eight-target `increment` /
`take_count` code-size table; pinned Cortex-M3 instruction regions under
rustc 1.92.0 `ded5c06cf`, LLVM 21.1.3, QEMU 10.0.11 — 8 / 9 / 9 on the assembled 0.3.0 tree
after the sentinel-RMW fix); contract §9 evidence map; tracking: issue #30,
PR #33; completion
commits `0a22ada` (clause-numbered contract + pinned costs) and
`f26d4c3` (H closed, promotion finalized).
