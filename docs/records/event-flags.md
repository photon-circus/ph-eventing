# EventFlags — engineering record

- **Status:** candidate, PROPOSED — complete admission package on
  `candidate/event-flags` (draft PR #36); contract frozen
  (M/R/T/C/S/B/H/W/X); shared handle decision H closed; awaiting
  acceptance review and release assembly.
- **Normative sources:** [contract](../proposals/event-flags-contract.md)
  (clause IDs cited below) · [proposal](../proposals/event-flags.md) ·
  admission evidence embedded in the proposal (§4) and
  `scripts/event-flags-atomic-window.sh` (portable-atomic ISR windows).

## 1. Value statement

`EventFlags` is a fixed SPSC condition set for the ISR-to-task niche
where the payload is readiness, not data: the producer raises any of
exactly 32 payload-free conditions with one Release `fetch_or`; the
consumer takes the entire pending set with one Acquire `swap(0)`.
Duplicate raises of the same condition coalesce — multiplicity is not
observable — and a racing raise is returned by the current take or
remains pending for the next one, never erased between windows (C1–C3).
Observing a condition publishes application memory sequenced before its
raise (S1); the mask itself stores none of that state. It complements
`CountedSignal` (multiplicity retained, payloads still absent) rather
than replacing it, and it refuses to be: a FIFO, a stream, a
multi-producer bus, a non-clearing advisory peek, or a target-dependent
word width.

## 2. Risks and integration concerns

Ordered by how likely they are to surprise an integrator. Contract clause
IDs in parentheses; the clauses are the normative statements.

- **Coalescing is the designed behaviour, not a fault mode (R2, X1).**
  Duplicate raises of one condition before the next take collapse to a
  single bit. The type is wrong for any consumer that must count how
  many times a condition fired — that is `CountedSignal`'s niche — and
  wrong for any consumer that must observe every distinct occurrence in
  order — that is `EventBuf` / `SeqRing`. Loss of multiplicity is
  observable only as an exact take window: a bit is either in this take
  or pending for a later one (C1–C3), never silently dropped *and*
  unreported.
- **No payload, no ordering, no identity (X1, X2).** Conditions are
  unordered set members. S1 publishes separately-owned application
  state; the mask does not carry it. An integrator who needs a value,
  a timestamp, or a per-raise identity must store that outside the
  flags and use the raise only as the readiness fence.
- **Exactly 32 conditions, forever for this type (W1, W2, X3).** The
  namespace is bits 0–31 of a transparent `EventMask(u32)`. A generic
  word width was rejected because it would expose target-specific atomic
  availability and fallback cost as public API; `u64` in particular is
  not one native operation on the 32-bit MCUs this crate supports. Need
  more than 32 distinct conditions? Compose multiple instances or pick a
  different primitive — do not expect a width parameter.
- **No non-clearing peek (D4, X5).** There is deliberately no advisory
  snapshot. A peek's result would be immediately stale across a
  concurrent take or raise, inviting check-then-act code the contract
  cannot uphold. Observation *is* `take_all`: destructive, exact, and
  the linearization point.
- **No stream or signal trait surface (D5, X5).** `EventFlags` does not
  implement `Sink`/`Source`/`Link`: a destructive take plus a rejecting
  downstream sink could lose the mask, and coalescing cannot be reported
  through the stream vocabulary. The sketched
  `SignalSink<S>`/`SignalSource<S>` pair is deferred —
  `CountedSignal`'s increment/count-snapshot vocabulary disproves one
  shared generic `S` as an honest surface for both. Bridging is
  application code.
- **Sole-role handles only (H1–H4, X4).** At most one producer and one
  consumer are active; acquisition is fallible and non-panicking; handles
  are `Send + !Sync` with `&self` hot paths. `fetch_or` would be sound
  with multiple raisers, but promising that here would force
  `CountedSignal` toward a weaker algorithm — the shared decision H
  (`f26d4c3`) keeps both lanes SPSC. An execution context destroyed
  without dropping its handle leaves the role held (pending state
  intact); there is no out-of-band reset.
- **Constrained-target atomics are measured, not free (B1–B3, X6).** On
  thumbv6m and ESP32-S2 the RMW is a portable-atomic critical section —
  measured interrupt-masked windows are **4** and **5** instructions
  respectively, straight-line, one RMW, no CAS loop. ESP32-S3 is native
  `s32c1i` (window **0**), correcting the exploratory assumption that S2
  and S3 share a fallback. Algorithmic bounds (B1–B3) are not a
  universal wall-clock claim (X6).

## 3. Technical claims and validation

| Claim | Evidence | Status |
|---|---|---|
| Raise unions into pending; take returns-and-clears exactly (M1, R1–R2, T1–T2, C1, C3) | Unit set: empty/all masks, bit 31, duplicate coalesce, multi-bit round-trip, empty raise/take no-ops | Pinned |
| Racing raise partitions across take windows; never erased (C1–C3) | Loom `event_flags_raise_racing_take_is_partitioned_exactly` and `event_flags_distinct_raises_partition_across_takes`; native/Miri threaded stress | Proven |
| Observed raise publishes prior application memory (S1) | Loom `event_flags_observed_raise_publishes_payload`; fails when either Release or Acquire is independently weakened to Relaxed | Proven |
| One `fetch_or` / one `swap(0)`; no CAS loop, no history-proportional work (B1–B3) | Source review; Cortex-M3 QEMU: raise **12** / take **8** instructions whether empty or set (constant w.r.t. occupancy and set-bit count) | Measured |
| Portable-atomic ISR windows stay small on constrained targets | `event-flags-atomic-window.sh`: thumbv6m **4**, ESP32-S2 **5**, ESP32-S3 **0** (native `s32c1i`); one masked RMW, no branch/CAS loop | Measured |
| Sole-role `Send + !Sync` handles; reacquisition continues pending state (H1–H4) | Exclusivity/reuse unit test; `Send` + container-`Sync` type tests; producer/consumer `compile_fail` doctests pin `!Sync`; `const_new_works_in_static_context` | Proven |
| Transparent panic-free 32-bit mask (W1–W2) | `EventMask` four-byte transparent layout; `from_index` returns `None` outside `0..32` (no shift panic) | Pinned |
| No silent `Source`/`Sink` or peek surface (D4, D5, X5) | Absent from the public API; contract non-promises X5; stream/signal traits deferred on the record | Pinned |
| Cost claims per target | Eight gated code-size rows + three opt-in Xtensa (raise/take 24/24 B thumbv6m, 23/22 B ESP32-S2, 37/36 B ESP32-S3); object is 12 B; static in `.bss` | Measured |

Full CI for the lane (`./scripts/verify.sh`, zero skips): 78 unit tests,
11 doctests, 5 compile-fail contracts, 93.60% line coverage, all 8 Loom
models plus ordering-mutation checks, detector-on Miri host and
i686/armv7/s390x proxies, feature/embedded builds, supply-chain checks.

## 4. The record

**Decision history (all closed with the contract freeze at `e9859d4`;
canonical text in contract §9; shared H closed earlier at `f26d4c3` on
issue #30):**

- **H — sole-role `Send + !Sync` producer/consumer handles with `&self`
  operations.** Accepted once for EventFlags and CountedSignal.
  EventFlags' `fetch_or` could support multiple raisers; promising that
  would force CountedSignal toward an unbounded CAS loop or a weaker
  overflow contract. The conservative shared shape loses no SPSC use
  case and leaves an independently-evidenced MPSC type possible later.
- **D2 — transparent `EventMask(u32)`.** The exploratory enum mapping
  retained runtime range checks and could not prove that two variants
  did not alias; the transparent mask erases to the raw word operation
  within two bytes while keeping domains distinct at the API boundary.
- **D3 — exactly 32 conditions.** A generic word width would expose
  target-specific atomic availability and fallback cost as public API.
  `u64` is not one native operation on the shipped 32-bit targets.
- **D4 — no non-clearing peek.** An advisory snapshot invites
  check-then-act reasoning the concurrent take/raise pair cannot uphold;
  observation is destructive `take_all` by design.
- **D5 — no stream `Sink`/`Source`/`Link`; signal traits deferred.**
  Coalesced state is not an item stream. CountedSignal's
  increment/count-snapshot vocabulary disproves the original single
  generic `SignalSink<S>`/`SignalSource<S>` sketch as an honest shared
  surface.

**Review history:** the lane's promotion bar and admission case
(portable-atomic ISR latency on thumbv6m / ESP32-S2/S3) were sequenced
behind shared decision H; once H closed, the contract freeze, secondary
confirmations (width, no-peek, trait posture), and the interrupt-window
campaign landed together as the complete admission package. Draft PR #36
carries the package for acceptance review; issue #29 is closed as
PROPOSED — ready for candidate evaluation, not yet acceptance into the
release. Evaluation may still reject the primitive on API fit or
measured cost, with the evidence retained either way.

**Where the numbers live:** proposal §4 (behaviour/Loom/Miri, 11-target
code size, pinned Cortex-M3 cycles beside CountedSignal's 8/7 anchor,
portable-atomic interrupt windows); `scripts/event-flags-atomic-window.sh`
(disassembly probe); tracking: issues #26/#29/#30, draft PR #36;
implementation freeze commit `e9859d4`, handle decision `f26d4c3`.
