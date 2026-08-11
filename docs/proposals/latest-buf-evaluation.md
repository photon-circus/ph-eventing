# LatestBuf implementation evaluation

- **Purpose:** implementation-independent comparison record for issue #27.
- **Inputs:** [`latest-buf.md`](latest-buf.md),
  [`latest-buf-contract.md`](latest-buf-contract.md), and the BlockBuf candidate.
- **Status:** evaluation scaffold; it does not select an implementation before
  the evidence below exists.

## 1. Decisions sufficient for an evaluable prototype

These defaults make development possible without pretending the deferred
choices have received a permanent API decision.

| Decision | Prototype default | Evidence or decision that would change it |
|---|---|---|
| D1, generation ambiguity | Exact within one non-zero `u32` span. Beyond that span use the documented approximation `generation_distance(last, current).saturating_sub(1)`; a repeated generation after exactly one full cycle therefore reports zero rather than panicking. Applications requiring a longer identity span carry a wider producer-assigned sequence in `T`. | Adopt an explicit `Gap::Unknown`-style API only with a concrete mechanism that can detect the wrap (extra epoch metadata, not inference from equal `u32` values), plus its RAM, flash, and cycle results. |
| D2, `Source<T>` | No `Source<T>` implementation. Provide `LatestSource<T>` so replacement evidence remains structural. | A design that preserves loss evidence through generic `Source`/`forward`; documentation alone is insufficient. |
| D3, sample or block | Implement generic `LatestBuf<T>` first. A complete block is a `T`; the BlockBuf candidate demonstrates `LatestBuf<Block<T, N>>` for latest and `EventBuf<Block<T, N>, Q>` for queued delivery. | A separate block transport must enforce a guarantee composition cannot, such as direct-to-granted-slot filling with a measured copy/RAM win. |
| A.3, role continuation | Compare channel-resident role state with handle-resident state persisted on `Drop`. Keep H2/H4 in both candidates. | Narrow H2 only if both candidates fail the evidence bar; reacquisition must not silently restart. |

The BlockBuf branch also makes `DropBlockBuf` a caller policy over the block
returned by `EventBuf::push`, not a third synchronization primitive. A named
type earns its place only through a distinct synchronization or accounting
contract.

## 2. State-persistence comparison

Both candidates use the same three exclusive roles and a single encoded
exchange atomic. Both endpoint exchanges must be `AcqRel`: the Release half
relinquishes the offered slot, while the Acquire half makes the acquired old
exchange slot safe to read or reuse.

| Property | Channel-resident role state | Persist-on-drop handle state |
|---|---|---|
| Steady-state role fields | `{back, next_generation}` and `{front, last_generation}` in role-owned channel cells | Fields in the active handles, with saved copies in the channel between handles |
| Reacquisition proof | Drop Release-clears `*_taken`; successful AcqRel acquisition orders all prior role-cell access | `Drop` must save every field before Release-clearing `*_taken`; acquisition must Acquire before loading every saved field |
| Failure surface | Follows current stateless-handle precedent; no state-copying Drop path | Missing or reordered Drop field strands a slot or restarts generation/accounting |
| Expected performance | May reload/store role fields through the channel on every operation | Fields may remain in registers across calls; Drop/reacquire does extra work |
| Handle size | Reference plus `!Sync` marker | Reference, role fields, and `!Sync` marker |
| Selection evidence | Loom/Miri correctness, then cycles and code size | Same; an unmeasured register-residency claim is not a win |

For channel-resident state, producer and consumer state require separate
tracked cells. They are role-owned, not concurrently shared. Reacquisition is
nevertheless a cross-context handoff and depends on the taken flag's
Release/Acquire pair. For persist-on-drop, a joined first thread must not be
used as the only model: `join` adds an independent happens-before edge that
can conceal a broken taken-flag handoff.

Handles in both candidates must be `Send + !Sync`. A reference to a `Sync`
channel otherwise makes a marker-free handle `Sync` automatically.

## 3. Required Loom models

Use tracked payload and role-state cells. A model that replaces payloads with
atomics proves only state encoding, not exclusive access.

### L1 — two publications versus two takes

One producer publishes distinct `(generation, value)` pairs 1 and 2 while one
consumer calls `take_latest` twice.

Assert after every explored execution:

- the first report has `replaced_unread == false`;
- the second report has `replaced_unread == !consumer_observed_generation_1`;
- every returned generation maps to exactly its published value;
- no publication instance is returned twice;
- returned generations do not go backwards for this no-wrap model;
- each successful take reports
  `skipped == generation_distance(previous, current) - 1`; and
- returned, skipped, pending, and displaced-awaiting-report publications
  satisfy O2 without double counting.

This model covers P1-P6, C1-C6, and O1-O3. Fixed calls are preferable to an
unbounded drain loop.

### L2 — stalled consumer

The consumer claims generation 1 and retains its private front role while the
producer publishes 2, 3, and 4. Every publish must complete, the producer must
never access the retained slot, and the next successful take must return 4
with two skipped publications. This is the direct evidence for C7 rather than
an inference from the absence of loops.

### L3 — role drop and cross-context reacquisition

The old handle mutates all continuation fields, drops, and a concurrently
running second thread retries acquisition with yields until it succeeds. The
old thread must not be joined before acquisition. Assert generation resumes,
gap accounting resumes, and tracked cells report no overlapping ownership.
Model producer and consumer roles separately so a failure identifies the
broken handoff.

### L4 — optional empty-poll fast path

If A.1 is implemented, interleave its ready-bit load with publication before,
during, and after the load. A false load returns without touching a slot and
can linearize before a concurrent publication. A true load must still use the
normal `AcqRel` exchange; a pre-load is not an ownership transfer. Compare
this model with the unconditional-swap reference before measuring it.

## 4. Ordering mutation matrix

A passing model is not ordering evidence until plausible weakenings fail.
Run each mutation independently and record which model rejects it.

| Mutation | Obligation removed | Expected detector |
|---|---|---|
| Producer exchange `AcqRel -> Acquire` | Release of the newly published slot | L1/L2 tracked payload race or stale value |
| Producer exchange `AcqRel -> Release` | Acquire of its next writable slot | L1/L2 tracked payload race |
| Producer exchange `AcqRel -> Relaxed` | Both halves | L1/L2 |
| Consumer exchange `AcqRel -> Acquire` | Release of its old private slot | L1/L2 tracked payload race |
| Consumer exchange `AcqRel -> Release` | Acquire of the claimed publication | L1 stale/unpublished value or tracked-cell failure |
| Consumer exchange `AcqRel -> Relaxed` | Both halves | L1/L2 |
| Handle drop `Release -> Relaxed` | Publication of saved/role-owned continuation state | L3 |
| Handle acquisition loses Acquire | Visibility of prior handle state | L3 |

If a mutation survives, first strengthen the model so it observes the stated
obligation. Do not strengthen production ordering merely to make a mutation
test convenient.

## 5. Unit and stress evidence

Sequential unit tests pin API semantics and arithmetic:

- new channel empty; publication 1 taken once, then empty;
- three publishes before a take yield replacement reports `false, true,
  true`, generation 3, and `skipped == 2`;
- drop/reacquire each role independently and together;
- second producer/consumer acquisition fails without panic;
- static construction and `Send + !Sync` compile assertions;
- `T: Copy` without `Default`, plus an array/block payload;
- successor `u32::MAX -> 1`, never zero;
- `(last, current) = (u32::MAX, 1)` gives distance 1 and skipped 0;
- `(u32::MAX - 1, 1)` gives skipped 1; and
- `last == current` at the full-cycle ambiguity seam does not underflow or
  panic and matches the selected D1 approximation.

A threaded stress test uses a patterned multiword payload and checks that a
returned value belongs wholly to one generation. It is useful scale evidence,
but it is not the race-freedom claim: Miri with the race detector on is.

The wording "bit-for-bit the value passed" in P6 is too strong for generic
Rust `T`: `Copy` does not promise preservation of padding bytes, and inspecting
padding is not a valid generic test. Before adoption, P6/C6 should say that the
returned `T` is the value from one publication under Rust value semantics,
never a torn or mixed value. Patterned payload tests can then exercise that
claim honestly.

## 6. Miri and bounded-cost evidence

The detector-on Miri pass must include LatestBuf rather than placing it in the
known SeqRing exclusion. It covers normal publication/take, replacement,
stalled-consumer reuse, and both role-reacquisition paths. A detector-off pass
or a native stress pass does not satisfy P6/C6.

Measure these regions separately:

- first publication and replacement publication;
- empty and pending `take_latest`;
- channel-state and persist-on-drop steady-state operations;
- handle drop and reacquisition for both candidates;
- `u32`, 16-byte, and representative block payloads; and
- unconditional empty swap versus A.1, if retained.

Publication and take may scale with `size_of::<T>()`, but not with lag,
occupancy, or history. Report handle size, channel size, and three-slot payload
RAM explicitly. Run code size across the complete target matrix, with
thumbv6m and ESP32-S2 treated as the kill-criterion rows because every RMW may
become an interrupt-disabled critical section.

## 7. Acceptance record

An implementation is ready to compare only when its PR can fill every cell:

| Evidence | Channel state | Persist on drop |
|---|---|---|
| Unit semantics and wrap seam | pending | pending |
| L1-L4 Loom models | pending | pending |
| All applicable ordering mutations fail | pending | pending |
| Miri race detector on | pending | pending |
| Native patterned-payload stress | pending | pending |
| Full code-size matrix | pending | pending |
| Cycle regions above | pending | pending |
| Handle/channel/RAM sizes | pending | pending |

Correctness removes a candidate; measurements select between candidates that
remain. Ergonomics is evaluated only after those results.
