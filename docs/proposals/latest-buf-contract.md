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
- **P2.** `publish` never waits for, observes the progress of, or invokes
  the consumer. Its behaviour and its cost are independent of whether the
  consumer exists, is running, or has ever run.
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
- **C3.** `skipped` equals the wrap-aware count (per G3) of generations
  assigned after `last_taken` and before `g` — that is, publications that
  were displaced before this consumer observed them. Taking the immediate
  successor of `last_taken` yields `skipped = 0`; last-taken 101 and taken
  104 yields `skipped = 2`.
- **C4.** Each publication is observed **at most once**. A taken generation
  is never returned again.
- **C5.** Observed generations are strictly increasing in wrap-aware order:
  `take_latest` never returns a generation older than or equal to a
  previously returned one.
- **C6.** The value returned under generation `g` is exactly and completely
  the value published under `g` (with P6: no torn, mixed, or partially
  initialized value is ever returned).
- **C7.** `take_latest` never waits for, observes the progress of, or
  invokes the producer, and never delays it: the consumer may hold a
  returned value indefinitely without affecting any producer clause.

## 5. Ordering and accounting clauses (O)

- **O1.** *(Freshness)* The publication returned by `take_latest` is the
  newest publication that existed at its linearization point. (This follows
  from P3 + C1 — `pending` always holds the newest — and is stated
  separately because it is the property the primitive exists for.)
- **O2.** *(Conservation)* Over any consumer history within one wrap span
  (see D1), every assigned generation is accounted exactly once: either
  returned by some `take_latest` (C4/C5), counted in exactly one `skipped`
  (C3), or still pending. No generation is double-counted and none
  disappears unaccounted.
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
- **B4.** No panic is reachable from either operation. (Constructor-time
  rejections, if any, must be compile-time, matching the crate's `N == 0`
  convention.)

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

## 9. Decision points (D) — deferred by maintainer decision (2026-08-11)

All three remain open deliberately; none blocks contract review. They must
be closed before implementation (step 2 of the adoption sequence) begins.

- **D1. Wrap-ambiguity policy.** If more than a full generation cycle of
  publications occurs between two takes, `skipped` is inherently ambiguous
  (proposal §18). The contract must pick one before implementation:
  (a) document the maximum reliable gap and leave larger gaps approximate;
  (b) return an explicit "unknown/wrapped" gap state;
  (c) point callers at a wider producer-assigned sequence in the payload.
  O2 is scoped "within one wrap span" until this is decided.
- **D2. `Source<T>` policy.** Whether the consumer implements the existing
  `Source<T>` (proposal §13). The contract default is the proposal's option
  1 (no impl; `LatestSource` only) — the safest position given the
  `RingBuf::pop` precedent — pending maintainer confirmation.
- **D3. First deliverable form.** Latest-sample vs latest-complete-block
  (proposal §11). The contract above is form-agnostic (a block is just a
  `T`), but the acceptance measurements are not; the choice shapes the
  probe payloads.

## 10. Evidence map

How each clause group gets its evidence when implementation starts. A clause
without a row here is a clause nobody is keeping true.

| Clauses | Evidence |
|---------|----------|
| G1–G3 | Unit tests at the wrap boundary (successor skips 0; distance excludes 0), mirroring `SeqRing`'s `seq_distance` test set; 32-bit Miri pass |
| P1–P5, C1–C5 | Loom models of publish/take interleavings (tiny state, capacity is fixed by design); threaded stress test for the same properties at scale |
| P6, C6 | Miri with the race detector **on** — this pass is the primitive's headline claim and must be pinned in `scripts/miri.sh`'s full-checking pass |
| O1–O3 | Property assertions inside the Loom models and the stress test (conservation counting on both sides) |
| B1–B3 | Code review of the final algorithm (no loops) + `cycles.sh` regions showing cost constant w.r.t. lag/occupancy |
| B4 | `codesize.sh` panic-string probe (the 0.2.0 technique) |
| H1–H3 | Unit tests mirroring the existing `try_producer`/`static`-handle test set |
| Ordering-strength | Mutation runs: each weakened ordering must fail at least one Loom model (proposal §14) |

## 11. Relationship to the design sketch

The three-slot ownership design in the proposal (§6–§8) is one candidate
implementation of this contract, and Appendix A of the proposal records
refinements to weigh against it. Conformance means satisfying every G/P/C/O/
B/H clause with the evidence in §10 — the contract does not care how many
slots that takes.
