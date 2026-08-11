# LatestBuf semantic contract (draft)

- **Status:** Draft for review — step 1 of the adoption sequence in
  [`latest-buf.md`](latest-buf.md) §21.
- **Rule of this document:** the contract is written against an **abstract
  channel**, independently of any implementation. Nothing here may mention
  slots, atomics, orderings, or ownership transfer — those belong to the
  design and its proof, which must *satisfy* this contract, not define it.
- **Clause IDs are load-bearing.** Tests, Loom models, and documentation
  cite clauses by ID (`P3`, `O2`, …). Renumbering is a breaking change to
  the evidence map; add new clauses at the end of their group.

## 1. Abstract model

A **channel** carries values of a `Copy` type `T` from exactly one producer
handle to exactly one consumer handle.

Abstract state:

- `pending : Option<(generation, value)>` — at most one publication is
  pending at any instant.
- `last_published : generation` — the newest generation ever published.
- `last_taken : generation` — the newest generation the consumer has
  observed (initially the reserved zero, meaning "none").

A **generation** is drawn from a finite, wrap-around sequence in which the
value `0` is reserved to mean "none". The successor function skips `0`.

Operations (their observable behaviour only):

- `publish(value) -> PublishReport` — producer.
- `take_latest() -> Option<LatestItem>` — consumer.

Both operations are **linearizable**: every call takes effect atomically at
some instant (its *linearization point*) between its invocation and return,
and all clauses below are stated against the sequence of those instants.

## 2. Generation clauses (G)

- **G1.** Generations assigned by successive `publish` calls are strictly
  increasing in wrap-aware order. The reserved value `0` is never assigned.
- **G2.** The successor of the maximum generation value is the minimum
  non-zero generation (the sequence wraps, skipping `0`).
- **G3.** Wrap-aware distance between two generations is defined excluding
  the reserved `0`, so a span that crosses the wrap counts exactly the
  generations that were actually assignable within it.

## 3. Producer clauses (P)

- **P1.** `publish` always succeeds. There is no full state, no rejection,
  and no error path.
- **P2.** `publish` never waits for the consumer and never invokes it, and
  its cost bound (B1) is independent of whether the consumer exists, is
  running, or has ever run. Its *report* is allowed — and by P4 required —
  to reflect consumer progress: `replaced_unread` observes whether the
  previous publication was taken. That observation is part of the
  operation's own bounded work, never a wait on the consumer.
- **P3.** At the linearization point of `publish(v)`: the channel's
  `pending` becomes `Some((g, v))` where `g` is the newly assigned
  generation, and `last_published` becomes `g`.
- **P4.** The returned `PublishReport` carries `generation = g` and
  `replaced_unread = true` **iff** `pending` was `Some(_)` immediately
  before the linearization point (i.e. an unread publication was
  displaced). Otherwise `replaced_unread = false`.
- **P5.** A publication is displaced only by a *newer* publication. Nothing
  else removes a pending publication except the consumer taking it.
- **P6.** `publish` publishes the complete value or, if the operation never
  linearizes (e.g. the producer is torn down mid-call by the environment),
  nothing. No partial publication is ever observable. ("Complete" means:
  the value later observed under `g` is bit-for-bit the `v` passed to this
  call.)

## 4. Consumer clauses (C)

- **C1.** If `pending = Some((g, v))` at the linearization point,
  `take_latest` returns `Some(LatestItem { value: v, generation: g,
  skipped })`, `pending` becomes `None`, and `last_taken` becomes `g`.
- **C2.** If `pending = None` at the linearization point, `take_latest`
  returns `None` and changes no observable state.
- **C3.** `skipped` is one formula in every case, fixed by D1's closure:
  the wrap-aware G3 distance from `last_taken` to `g`, **minus one**
  (the returned publication), saturated at zero. Within one wrap span
  (the same scope as C5 and O2 — see D1) this equals exactly the count of
  generations assigned after `last_taken` and before `g` — publications
  displaced before this consumer observed them: taking the immediate
  successor of `last_taken` yields `skipped = 0`; last-taken 101 and
  taken 104 yields `skipped = 2` (G3 distance 3, minus one). Beyond one
  wrap span the exact count is inherently unrecoverable from two endpoint
  values, and the same formula **under-counts**: by an exact multiple of
  the span size while the endpoints differ, and at coincident endpoints —
  a gap of exactly one or more whole cycles — it reports `skipped = 0`,
  a documented silent under-count (X6).
- **C4.** Each publication is observed **at most once** — the same
  publication *instance* is never returned twice, unconditionally. Stated
  per generation **value**, the claim carries C5's one-wrap-span scope: a
  taken generation value does not reappear unless a full generation cycle
  has passed and a genuinely newer publication reuses it, which is the
  beyond-span case D1's closure documents (§9, X6).
- **C5.** Observed generations are strictly increasing in wrap-aware order
  **within one wrap span** (the same scope as O2): while fewer than a full
  generation cycle of publications separates two takes, `take_latest`
  never returns a generation older than or equal to a previously returned
  one. Beyond that span a genuinely newer publication can carry a
  previously seen generation value, so the comparison is inherently
  ambiguous there. Per D1's closure, **no ordering claim is made beyond one
  wrap span**: an observed generation may repeat or appear to regress
  relative to one taken a full cycle earlier, and consumers must not treat
  generation comparison as a total order across such a gap (X6).
- **C6.** The value returned under generation `g` is exactly and completely
  the value published under `g` (with P6: no torn, mixed, or partially
  initialized value is ever returned).
- **C7.** `take_latest` never waits for the producer, never invokes it,
  and never delays it: its cost bound (B2) is independent of producer
  activity, and the consumer may hold a returned value indefinitely
  without affecting any producer clause. Its *result* is allowed — and by
  C1/C2 required — to reflect published state; observing `pending` is part
  of the operation's own bounded work, never a wait on the producer.

## 5. Ordering and accounting clauses (O)

- **O1.** *(Freshness)* The publication returned by `take_latest` is the
  newest publication that existed at its linearization point. (This follows
  from P3 + C1 — `pending` always holds the newest — and is stated
  separately because it is the property the primitive exists for.)
- **O2.** *(Conservation)* Within one wrap span (see D1), every assigned
  generation is at every instant in exactly one of four states: returned by
  some `take_latest` (C4/C5), already counted in exactly one `skipped`
  (C3), still pending, or **displaced and awaiting report** — a
  publication displaced before any take is not lost from the accounting:
  it lands in the `skipped` of the next successful take. No generation is
  double-counted and none disappears; immediately after a successful take
  the awaiting-report set is empty, so at those observation points the
  first three states account for everything. Beyond one wrap span,
  counting conservation is **not claimed**: the four-state partition
  still describes every observed value, but `skipped` under-counts the
  assigned generations exactly as C3 states — conservation in the
  counting sense holds within one wrap span only (C3, X6).
- **O3.** *(No fabrication)* Every generation returned or counted was
  assigned by a real `publish` call. `skipped` never includes generations
  that were never assigned (in particular, never the reserved `0` — G3).

## 6. Boundedness clauses (B)

Algorithmic bounds at the contract level; instruction-count claims live with
the measured implementation, per environment.

- **B1.** `publish` performs a statically bounded amount of work: no
  unbounded loop, no retry whose iteration count depends on consumer
  behaviour, no dynamic allocation, no user code invoked.
- **B2.** `take_latest` performs a statically bounded amount of work,
  independent of how many publications were displaced since the last take:
  no unbounded loop, no allocation, no user code invoked.
- **B3.** Neither operation's bound depends on channel occupancy, lag, or
  history. (Both bounds are permitted to depend on `size_of::<T>()`, which
  is fixed at compile time.)
- **B4.** No panic is reachable from either operation. (By analogy with
  the ring types' `N == 0` convention: if the design ever grows a
  constructor-time rejection, it must be a compile-time one. The abstract
  channel has no capacity parameter, so today there is nothing to reject —
  the clause exists so that never changes silently.)

## 7. Handle clauses (H)

- **H1.** At most one producer handle and at most one consumer handle exist
  per channel at any time. Acquisition is fallible and non-panicking
  (`try_producer` / `try_consumer` returning `Option`, matching the crate's
  0.2.0+ convention); a second concurrent acquisition of the same role
  fails.
- **H2.** Handles are `Send + !Sync`, and dropping a handle makes its role
  re-acquirable. (Same model as `SeqRing`/`EventBuf`; the `!Sync` is what
  makes moving a handle into an ISR sound.)
- **H3.** A channel is constructible in a `static` (const construction on
  the normal build), matching the existing primitives.
- **H4.** Reacquisition continues, never restarts: after a handle is
  dropped and its role reacquired, the channel's observable behaviour is
  indistinguishable from the original handle having continued. The
  generation sequence resumes (G1 holds across the drop), skipped
  accounting continues to satisfy C3 within C3's scope, and every other
  clause keeps holding. A
  design that cannot honour this must not offer reacquisition for that
  role (narrowing H2 explicitly rather than weakening H4). *(How
  continuation state survives the drop is an implementation concern,
  deliberately not specified here — the proposal's Appendix A.3 records
  the closed mechanism decision, 2026-08-11. X8 states the boundary
  facts of role recovery for integrators.)*

## 8. What the contract does not promise (X)

- **X1.** Delivery of every publication. Displacement is the designed
  overload behaviour (P4/P5), not a defect.
- **X2.** FIFO access to a backlog. There is no backlog; `SeqRing` is the
  ordered-recent-window primitive.
- **X3.** Any timestamp semantics. Time belongs to the payload and its
  producer (proposal §10).
- **X4.** Multi-producer or multi-consumer operation (H1).
- **X5.** Real-time guarantees. B-clauses are algorithmic bounds; wall-clock
  and instruction costs are measured claims, stated per target and
  toolchain.
- **X6.** Exact loss accounting beyond one wrap span. `skipped` is exact
  while fewer than a full generation cycle of publications separates two
  consecutive takes; beyond that the C3 formula under-counts — by whole
  spans while the endpoints differ, and a gap of exactly one or more full
  cycles reports zero (C3) — a silent under-count. Three facts frame this
  honestly for adopters:
  - The boundary is a property of the **publication-rate × take-interval
    product**: it is crossed exactly when a full span of publications
    occurs between two successful takes. The contract sets no rate and no
    cadence (X5), so both roads to it are real — a consumer that stops
    taking (fault), and a deployment that deliberately takes rarely
    against a fast producer (design). For a 32-bit generation the
    arithmetic is ~49.7 days between takes at continuous 1 kHz
    publishing, ~72 minutes at 1 MHz; at millisecond-scale take cadences
    the required rate is unattainable, which is why the common road is
    the take interval — but the contract bounds neither, and adopters
    own this arithmetic for their rates.
  - The channel deliberately does **not detect** the crossing. Detection
    would require widening hot-path state (a wider generation or a wrap
    epoch), and even a detected "unknown" cannot recover the lost count —
    it converts a silent under-count into a named one at a per-operation
    cost on exactly the constrained targets this primitive exists for.
  - Callers whose **requirement is the count itself** — audit, metering,
    regulatory loss accounting — must carry a wider producer-assigned
    sequence inside `T` (time and identity belong to the payload, X3) or
    use a saturating-counter primitive; a stalled-consumer *liveness*
    requirement belongs to the supervision layer (watchdog/heartbeat),
    which detects a dead consumer orders of magnitude sooner than any
    generation wrap.
- **X7.** Substitution into generic `Source<T>` pipelines. The consumer
  does not implement `Source<T>` (D2): `try_pop`'s shape cannot report
  displacement, and displacement is this channel's designed overload
  behaviour (X1), so a generic pipeline would silently discard loss
  evidence during *normal* operation. `LatestSource<T>` is the consumer's
  designed contract surface. A caller whose domain genuinely permits
  discarding the skipped count writes its own adapter — the decision to
  discard loss evidence is an application decision, signed in application
  code, never the transport's. Adding a convenience `Source` impl later
  remains an additive, adopter-evidence-gated decision (§9 D2); it must
  never arrive as a silent convenience.
- **X8.** Role recovery beyond `Drop`. Reacquisition is promised only
  after the previous handle is dropped (H2/H4): the drop is the exchange's
  handoff point, and a successful reacquisition orders against it. Whether
  and when a handle is dropped is a property of the application and its
  execution environment, never of this contract — an environment that
  destroys an execution context without running destructors, or code that
  forgets a handle, leaves the role held, with channel state intact and
  every other clause still holding. The contract deliberately offers no
  out-of-band role reset: a forced release could clear a role while a
  live handle still exists, and double acquisition is exactly what the
  exclusive-ownership soundness argument exists to prevent. These are the
  facts of the exchange, stated so an integrator can design against them
  with full information; how an application manages handle lifetime —
  supervision, task teardown, restart strategy — is beyond this
  contract's scope, in the same posture as X3 and X5.

## 9. Decision points (D)

All three decision points are **closed** (2026-08-11, below).

- **D1. Wrap-ambiguity policy — CLOSED (maintainer, 2026-08-11): options
  (a) + (c) together.** `skipped` is exact within one wrap span and reports
  the documented under-counting C3 formula beyond it (a); callers whose
  requirement is the count itself are pointed at a wider producer-assigned
  sequence carried in the payload or a saturating-counter primitive (c).
  The whole wrap family closes with it: C3 (beyond-span gap reporting), C5
  (no beyond-span ordering claim), O2 (in-span-only conservation), and the new
  X6 (the non-promise, stated so a downstream user can decide against the
  type with full information). Option (b) — an explicit "unknown/wrapped"
  state — was rejected on two independent grounds recorded in X6: it
  prices wrap detection into every hot-path operation on the targets the
  primitive exists for, and it still cannot serve the accounting mission,
  because "unknown" does not recover a count that exceeded the counter.
  **Documentation obligation, binding on the implementation:** the concrete
  type's public documentation must carry the X6 disclosure prominently —
  the exactness span, the silent-zero full-cycle case, the
  rate × take-interval reachability arithmetic at representative rates,
  and the (c) escape hatch. "Unreachable in practice" is a measured claim to be shown, never
  a reason for silence.
- **D2. `Source<T>` policy — CLOSED (maintainer, 2026-08-11): the
  proposal's option 1.** The consumer does not implement `Source<T>`;
  `LatestSource<T>` is the consumer's contract surface. The maintainer's
  articulation, recorded as the rationale: **the `LatestSink`/`LatestSource`
  traits were designed to solve the problem the existing traits could not —
  they are the contract surface for the type**, not a parallel convenience
  beside a missing `Source` impl. `Source::try_pop`'s shape structurally
  cannot report displacement, and displacement is this channel's *designed*
  overload behaviour (X1) — so a generic `Source` impl would discard loss
  evidence during normal operation, recreating the `RingBuf::pop`
  rejection in a case where the loss is routine rather than a corner.
  The proposal's option 2 (a documented-convenience `Source` impl) remains
  the pre-registered **additive** upgrade path, to be reconsidered only on
  adopter evidence per §13 — never retrofitted by convenience; option 3
  (a sequenced-payload `Source`) stays subsumed by the second-
  implementation trait gate on payload-metadata vocabulary. New
  non-promise **X7** states the substitution limitation for adopters, and
  the evidence map pins the absence of the impl so it cannot regress
  silently.
- **D3. First deliverable form — CLOSED (maintainer, 2026-08-11): the
  convergent answer.** The first deliverable is the payload-agnostic
  `LatestBuf<T>` exactly as this contract states it: a complete block is a
  payload (`T = Block<…>`), and sample-versus-block is a release-scheduling
  and RAM choice, never a separate primitive. There is no `LatestBlockBuf`
  to design; the coupled BlockBuf candidate's composition
  (`LatestBuf<Block<T, N>>` for latest, `EventBuf<Block<T, N>, Q>` for
  queued, caller policy on the returned block for drop-new) is the
  confirmed type identity. `Block<T, N>` names the developed candidate's
  accepted definition — payload type and sample count only; any per-sample
  stamp travels inside `T`, per X3. The seed sketch's
  `Block<T, Stamp, N>` parameterisation is superseded by it. Both lanes reached this independently, and the
  joint composition matrix (`8593b4a`; 66 pinned regions over 2/8/16-byte
  samples × `N = 8/32/128`) measured it: no row establishes a separate
  latest-block contract, and the probe-payload concern above is satisfied
  by having measured both shapes.
  **Documentation obligation, binding on the block-payload surfaces:**
  per-shape RAM stated plainly (three complete blocks plus the private
  builder — 136–8,280 B across the measured grid); the small-block
  inversion stated as guidance, not buried (at `N = 8`, block publication
  costs 54%/31% more than `N` individual publishes for 8/16-byte samples;
  block release wins for every 2-byte row and for 8/16-byte rows at
  `N >= 32`); and the no-partial-block limitation stated for adopters —
  composition never exposes an incomplete window, so sample-level
  freshness inside a partially filled block is unobtainable by design.
  **Registered reopening condition** (carried from the evaluation record):
  a separate block transport earns existence only by enforcing a guarantee
  composition cannot — direct-to-granted-slot filling with a measured
  copy/RAM win — and that question lives behind cycle decisions P/S, not
  here.

## 10. Evidence map

How each clause group gets its evidence when implementation starts. A clause
without a row here is a clause nobody is keeping true.

| Clauses | Evidence |
|---------|----------|
| G1–G3 | Unit tests at the wrap boundary (successor skips 0; distance excludes 0), mirroring `SeqRing`'s `seq_distance` test set; 32-bit Miri pass |
| P1–P5, C1–C5 | Loom models of publish/take interleavings (tiny state, capacity is fixed by design); threaded stress test for the same properties at scale |
| P6, C6 | Miri with the race detector **on** — this pass is the primitive's headline claim and must be pinned in `scripts/miri.sh`'s full-checking pass |
| C7 | Loom/stress scenario with a stalled consumer: the consumer claims a value and holds it across many publishes while every P-clause is re-asserted — producer progress is proven, not assumed |
| O1–O3 | Property assertions inside the Loom models and the stress test (conservation counting on both sides) |
| B1–B3 | Code review of the final algorithm (no loops) + `cycles.sh` regions showing cost constant w.r.t. lag/occupancy |
| B4 | `codesize.sh` panic-string probe (the 0.2.0 technique) |
| H1–H4 | Unit tests mirroring the existing `try_producer`/`static`-handle test set, plus drop-and-reacquire continuation checks stated observably: after a drop in one context and reacquisition in another, the generation sequence resumes (G1) and skipped accounting still satisfies C3 — exercised under Loom and race-detector-on Miri, because a sequential test cannot exercise a cross-context handoff. How continuation state survives the drop is the implementation's concern, and validating that mechanism belongs to the proposal's evidence (Appendix A.3), not this map |
| Ordering-strength | Mutation runs: each weakened ordering must fail at least one Loom model (proposal §14) |
| C3/C5/O2 beyond-span, X6 (D1 closure) | Unit test pinning the full-cycle result of the C3 formula (equal endpoints report zero — the prototype's `full_generation_cycle_uses_documented_approximation` is the template), alongside the G1–G3 wrap-boundary tests; a documentation check that the concrete type's public docs carry the X6 disclosure (span, silent-zero case, reachability arithmetic, escape hatch) |
| X7 (D2 closure) | Compile-fail test pinning that the consumer does **not** implement `Source<T>`, so the decision cannot regress via a later convenience impl; a documentation check that the public docs name `LatestSource` as the contract surface and state the adapter escape hatch |

## 11. Relationship to the design sketch

The three-slot ownership design in the proposal (§6–§8) is one candidate
implementation of this contract, and Appendix A of the proposal records
refinements to weigh against it. Conformance means satisfying every G/P/C/O/
B/H clause with the evidence in §10 — the contract does not care how many
slots that takes.
