# SlotPool — engineering record

- **Status:** **DEFERRED** (decision S, 2026-08-11) — evaluation
  evidence complete and banked on `candidate/slot-pool`; not admitted
  this cycle. **P closed as Copy composition** (no in-release consumer);
  standalone admission refused without a real adopter to shape the
  grant/claim API. **Reopening trigger** (any one): a measured budget
  breach at a supported shape; a direct-to-granted-slot (DMA-class)
  requirement composition cannot satisfy; a standalone zero-copy
  adopter. Draft PR #32 closed as deferred; branch preserved and
  archive-tagged.
- **Normative sources:** [proposal](../proposals/slot-pool.md)
  (SP clause IDs cited below) ·
  [target-cost record](../proposals/slot-pool-measurements.md) ·
  [taxonomy](../proposals/exploratory-primitives.md) §3 · triage in
  [`0.3.0-candidates.md`](../0.3.0-candidates.md) §4.

## 1. Value statement

`SlotPool<T, N>` is a bounded SPSC ownership-transfer pool for the
zero-copy niche: free slots become producer-owned grants, committed
slots become consumer-owned claims, and release returns storage to the
pool — `Free → ProducerOwned → Published → ConsumerOwned → Free`. The
producer never scans or retries; a full pool rejects the new
reservation explicitly. Initialization is a type-state witness
(`VacantGrant::write` before any type that can `commit`), so a claim
never observes uninitialized bytes. It is the crate's first deliberate
departure from all-`Copy` payloads: RAII grants and claims carry drop
obligations that every shipped primitive today refuses. It owns the
`ReservableSink`/`ClaimSource` (GAT) sketches, refuses to be a DMA
cache authority, and never claimed this cycle should admit it — a
posture the closures vindicated: P closed as Copy, and S deferred the
pool with its evidence banked and an adopter-gated trigger registered.

## 2. Risks and integration concerns

Ordered by how likely they are to surprise an integrator. Proposal
clause IDs in parentheses; the clauses are the exploratory contract,
not an admission decision.

- **Non-`Copy` RAII is the sharp edge (SP-O4, SP-O5, SP-B4).** Every
  current primitive is `T: Copy` with no drop obligations on the hot
  path. Grants and claims are not: cancellation and release are
  panic-free and O(1), and user `T::drop` runs only at pool teardown —
  but the integrator must treat handle lifetime and initialized residue
  as part of the contract. If that obligation is unacceptable, decide
  *against* the type and stay on `Copy` composition; the measurements
  exist so that choice is informed, not deferred.
- **A claim must never read uninitialized storage (SP-O4, SP-P1–P2).**
  `VacantGrant` has no `commit`; only `write(T)` produces a `Grant`.
  Breaking that witness — treating `commit` as proof of arbitrary
  in-place initialization — would let a claim form `&T` over raw
  bytes. The compile-fail pin and Miri drop/init suite exist because
  this failure mode is silent UB, not a logic bug.
- **Grant cancellation must stay panic-free and bounded (SP-C3,
  SP-B1, SP-B4).** Dropping or cancelling an uncommitted reservation
  performs no atomic and invokes no user drop: vacant cancel leaves
  the slot uninitialized; cancel-after-write leaves reusable
  initialized residue. Any future API that runs `T::drop` or retries
  on the cancel path breaks the hot-path bound the cycle is measuring.
- **Stale handles are excluded by API shape, not generation tags
  (SP-O1–O3).** One reservation and one claim in flight, each borrowing
  its sole endpoint mutably, make detached tokens and generation tags
  unnecessary *inside this restricted surface*. Detached FFI handles,
  multi-reservation, or MPSC invalidate the cursor-only representation;
  they are not silent optimizations of this design.
- **Reservation is bounded and reject-new (SP-B2, SP-B3).** Occupancy
  never exceeds `N`; a full pool returns `None` with no state change.
  There is no overwrite policy and no unbounded scan for a free slot.
- **Single-reservation SPSC first (SP-O1, SP-O2).** Mutable endpoint
  borrows enforce one outstanding grant and one outstanding claim. A
  requirement for more is a different primitive, not a flag on this
  one.
- **DMA cache maintenance stays outside the crate.** The pool
  transfers Rust ownership only; target-specific visibility
  (clean/invalidate, barrier pairing with a DMA engine) remains caller
  policy, per the taxonomy out-of-scope list.

## 3. Technical claims and validation

| Claim | Evidence | Status |
|---|---|---|
| Exclusive ownership cycle; producer/consumer never share a slot (SP-O1–O3) | Cursor representation; Loom tracked-slot commit/claim and grant-drop models (capacity 1) | Proven (modelled) |
| No uninitialized claim; type-state init witness (SP-O4, SP-P1) | `VacantGrant`/`Grant` split; `compile_fail` doctest; detector-on Miri init/cancel/teardown | Proven |
| Initialized `T` dropped exactly once at pool teardown (SP-O5) | Non-`Copy` drop-counter tests + Miri | Proven |
| Commit publishes only after construction/mutation; FIFO claims (SP-P2, SP-C1–C2) | Release/Acquire cursor pairing; Loom exclusivity model; unit FIFO suite | Proven |
| Cancellation publishes nothing; hot path invokes no user drop (SP-C3, SP-B4) | Cancel/drop unit paths; grant-drop Loom interleavings; teardown-only drop tests | Proven |
| Wait-free state paths; reject-new when full; occupancy ≤ N (SP-B1–B3) | Algorithm review (no CAS retry / no scan); pinned QEMU regions | Measured |
| Representative zero-copy block handoff without publication copy | `representative_block_handoff_fills_and_claims_the_same_storage` (pointer identity) | Pinned |
| Cost claims per target (`d3f252e`) | 8 gated code-size targets: 30–84 B; pinned QEMU 10.0.11 Cortex-M3: 3–16 instructions per state path | Measured |

Evaluation evidence is complete for the exploratory bar. P and S are
closed (Copy; deferred), so this table claims exactly what survives the
closures: measured costs and proven properties, banked for the trigger —
not admission, and not superiority to `Copy` composition on any named
budget.

## 4. The record

**Decision history (evaluation complete; P and S closed 2026-08-11):**

- **P — CLOSED as Copy composition** (planning-record P/S closure).
  This pool's measured rows (reserve 14 / full 13 / commit 4 / claim
  16 / release 3 / init+commit 13 instructions; 30–84 B across the
  eight gated targets, `d3f252e`) sit banked beside the Copy rows so
  the eventual trigger reading needs no new measurement. Copy's one
  unserved corner (large-block × slow-core × ISR × high-rate) is the
  registered road back here.
- **S — CLOSED as DEFERRED.** Not standalone (no adopter to shape the
  grant/claim API), not foundation (no in-release consumer with P =
  Copy), not rejected (the evidence is banked, the branch preserved
  and archive-tagged, draft PR #32 closed as deferred). The trigger is
  adopter-gated — a measured budget breach, a direct-to-granted-slot
  requirement, or a standalone zero-copy adopter — and on trigger,
  BlockBuf-over-SlotPool is the demonstration path.
- **Initialization witness: type-state, not runtime flag.**
  `VacantGrant::write(T) → Grant → commit` is the evaluated shape;
  prefix-count tracking (one pool-wide `u32`) replaces a per-slot
  bitmap under FIFO commit order. Reopening condition: a requirement
  for field-by-field `MaybeUninit` observation that `write(T)` does
  not claim.
- **Scope: one reservation / one claim, SPSC, reject-new.** Cursor-only
  representation depends on that restriction; generation tags,
  detached tokens, MPSC/MPMC, and overwrite are explicitly out.
- **Traits: `ReservableSink` / `ClaimSource` remain sketches.** Per the
  crate's trait rule they wait for admission *and* a second concrete
  implementation; admission alone does not publish them.
- **Does not claim:** cycle admission, PROPOSED status, or that
  SlotPool beats `Copy` composition on any named budget — P and S
  closed against admission this cycle, and any future claim rides the
  registered trigger with fresh comparison evidence.

**Review history:** evaluation implementation and evidence landed on
`candidate/slot-pool` (draft PR #32), culminating in the target-cost
and Loom/Miri completion commit `d3f252e`. Issue #31's development
follow-ons are complete; remaining work is review and admission
policy. Issue #26 holds the deferred P budget that gates S.

**Where the numbers live:** `slot-pool-measurements.md` (8-target code
size, pinned QEMU cycles, RAM shape); proposal §9–§13 (SP clauses,
BlockBuf probe, evidence map, remaining decisions); tracking: issues
#31/#26, PR #32.
