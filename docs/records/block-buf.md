# BlockBuf — engineering record

- **Status:** candidate, EVALUATION READY — complete development package on
  `candidate/block-buf` (draft PR #34); D3 type-identity confirmed
  (composition, no `LatestBlockBuf`); **promotion waits on cycle decision
  P** (named instruction/time budget and RAM envelope); SlotPool path
  (S) gated behind P.
- **Normative sources:** [proposal](../proposals/block-buf.md)
  (§5–§9 cited below) · LatestBuf
  [contract §9 D3](../proposals/latest-buf-contract.md) ·
  [publication matrix](../proposals/block-buf-measurements.md)
  (`bc54a9a`) · joint D3 matrix on the LatestBuf lane
  (`latest-block-composition-measurements.md`, `8593b4a`, PR #35).

## 1. Value statement

`Block<T, N>` and `BlockBuilder<T, N>` are the fill-side half of complete
window handoff: the producer privately accumulates `N` contiguous samples
and yields a finished block only when every slot is initialized; the
consumer never sees a partial window. Overload policy is composition, not
a new synchronization primitive — latest is `LatestBuf<Block<T, N>>`,
queued is `EventBuf<Block<T, N>, Q>`, and drop-new is call-site handling
of the complete block `EventBuf::push` returns on rejection. The headline
property is observable interruption *before* publication: discontinuities
preserve the partial builder and return the rejected sample; teardown of
an incomplete builder publishes nothing. It refuses to be: a separate
`LatestBlockBuf`/`QueuedBlockBuf`/`DropBlockBuf` type family, a
timestamp authority (stamps live inside `T`), a free constant-cost
publication path across block shapes, or a SlotPool grant transport until
cycle decision P authorizes that branch.

## 2. Risks and integration concerns

Ordered by how likely they are to surprise an integrator. Proposal
section and contract IDs in parentheses; those texts are normative.

- **Publication cost scales with block bytes, not with the O(1)
  synchronization step (proposal §6, §6.1).** Accepted
  completion-plus-publication on the reference Cortex-M3 runs 159–8,658
  retired instructions across the measured grid (2/8/16-byte samples ×
  `N = 8/32/128`) and grows roughly linearly with logical payload
  traffic (48–4,112 B per path). Synchronization being constant does
  **not** make publication cheap; the acquisition budget must be checked
  against the accepted row for the chosen shape, not against a scalar
  handoff myth.
- **Rejection is nearly as expensive as acceptance (§6.1).** Preserving
  and returning the complete rejected block keeps the rejected path
  within 5–31 instructions of the accepted path on every measured row —
  not a cheap scalar `Err(())`. Callers that treat rejection as free
  error plumbing will mis-budget the ISR/task.
- **Small-N publication can lose to per-sample LatestBuf (D3
  obligation).** The joint composition matrix (`8593b4a`) shows block
  publication beats per-sample for every 2-byte row and for 8/16-byte
  samples at `N ≥ 32`, but costs 54%/31% *more* at the 8/16-byte
  `N = 8` corners. Composition is not automatically the cheaper shape.
- **RAM is multiple complete blocks, always (D3 docs obligation, §6).**
  Latest composition holds three payload slots plus the private builder;
  the joint matrix states 136–8,280 B of combined channel + builder RAM
  across the measured grid. Queued composition is `Q * B` plus atomics,
  on top of fill-side storage. State the per-shape number; the docs are
  required to.
- **No partial-block freshness (F1, D3).** Composition never exposes an
  incomplete window — sample-level freshness inside a filling block is
  unobtainable by design. A discontinuity returns the rejected sample
  without mutating the partial builder; the caller must `clear` or
  retry explicitly (F3).
- **Copy composition vs SlotPool is undecided (P open; S gated).** The
  publication matrix (`bc54a9a`) supplies the numbers but does **not**
  close P: no named ISR/task instruction/time budget or RAM envelope
  exists in the record. S (representative BlockBuf-over-SlotPool
  integration) remains unauthorized until P chooses. Treating Copy as
  selected, or SlotPool as selected, would invent a decision the
  maintainer deferred.
- **Drop-new is an action, not a type (§5.1).** There is no
  `DropBlockBuf`; discarding a newly completed block is what the caller
  does with `Err(block)`. Loss after completion is reported by the
  selected transport (`PublishReport::replaced_unread` or the returned
  block), distinct from pre-publication filtered/incomplete windows.

## 3. Technical claims and validation

| Claim | Evidence | Status |
|---|---|---|
| Type identity is composition — no `LatestBlockBuf` (D3, §5.1) | Maintainer closure 2026-08-11 (contract §9, PR #37); candidate implements `Block`/`BlockBuilder` only | Closed |
| Complete-only yield; teardown publishes nothing (F1, F4) | Unit pins on completion boundary; drop/clear of partial builder; Miri on `MaybeUninit` completion copy | Proven |
| Contiguous wrap-aware sequences; reserved `0` rejected (F2) | Fill-error unit set incl. wrap; `T: Copy` without `Default` | Pinned |
| Discontinuity preserves partial block and returns sample (F3) | Explicit interruption tests; no silent mutate-or-count | Proven |
| Accepted/rejected publication cost measured per shape (§6.1) | Matrix `bc54a9a`: 18 instruction rows (159–8,658 accepted; rejection within 5–31); reproduced on QEMU 10.0.11 and 10.2.1 | Measured |
| Flash cost bounded across gated targets | 8-target codesize matrix: 110–312 B per completion-plus-publication shape | Measured |
| Joint D3 composition cost vs per-sample (D3) | Matrix `8593b4a`: 171–6,958 instructions; 136–8,280 B combined channel+builder RAM; small-N inversion documented | Measured |
| No block-layer atomics; transport evidence unchanged (§8) | Block path: unit + Miri; LatestBuf/EventBuf: detector-on Miri and Loom on selected transport | Proven (transport) |
| Copy vs SlotPool foundation (P / S) | Matrix complete; named budget/envelope absent; maintainer deferred holistic reading | **Open — blocks promotion** |

Full CI for the lane: 78 unit tests, 12 doctests, 4 compile-fail, 8
gated codesize targets, 3 embedded checks, 6 Miri passes, 5 Loom
models.

## 4. The record

**Decision history (D3 closed 2026-08-11; P deferred — canonical text in
contract §9 and proposal §5–§9):**

- **D3 — payload-agnostic `LatestBuf<T>`; blocks are payloads
  (composition).** Closed jointly with the LatestBuf lane: latest =
  `LatestBuf<Block<T, N>>`, queued = `EventBuf<Block<T, N>, Q>`,
  drop-new = handle `Err(block)` at the call site. No named
  `LatestBlockBuf` / `QueuedBlockBuf` / `DropBlockBuf`. Reopening
  condition (contract §9): a separate block transport must enforce a
  guarantee composition cannot (direct-to-granted-slot filling), and
  that question lives behind cycle decisions P/S.
- **Fill-side contract F1–F5 (§8) — selected as the block-only delta.**
  Complete-only yield, contiguous sequences, explicit interruption,
  teardown publishes nothing, reuse after completion. No concurrency
  contract is added by composition; LatestBuf / EventBuf clauses apply
  with `T = Block<…>`.
- **Payload and stamps (§5.3) — stamps inside `T`.** `Block<T, N>`
  carries `[T; N]` plus inclusive first/last sequences; zero-stamp
  applications pay zero timestamp overhead. The seed sketch's
  `Block<T, Stamp, N>` parameterisation is superseded.
- **P — Copy composition vs SlotPool: open.** Measurement package
  complete (`bc54a9a` publication matrix; joint D3 matrix `8593b4a`);
  maintainer deferred naming the ISR/task instruction/time budget and
  RAM envelope until a holistic reading of the cycle. This record does
  **not** select Copy and does **not** authorize SlotPool.
- **S — SlotPool path: gated behind P.** No representative
  BlockBuf-over-SlotPool integration is authorized while P is unset;
  choosing a library-wide `N` threshold in lieu of an application budget
  was explicitly refused (§6.1).

**Review history:** D3 closure and the composition identity were
confirmed in the LatestBuf contract arbitration (PR #37) as the
convergent answer both coupled lanes reached independently. BlockBuf's
remaining gate is not further type-identity review — it is the budget
decision P.

**Where the numbers live:** `block-buf-measurements.md` (publication
matrix `bc54a9a`: 18 cycle rows, 8-target flash); joint D3 matrix on
the LatestBuf lane (`latest-block-composition-measurements.md`,
`8593b4a`, PR #35); proposal §6 / §6.1 (cost model and decision rule);
tracking: issue #26 decision P, draft PR #34, contract PR #37.
