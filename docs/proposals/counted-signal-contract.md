# CountedSignal semantic contract

- **Status:** Frozen for evaluation on the `candidate/counted-signal` lane.
  Promotion-bar item 3 in [`counted-signal.md`](counted-signal.md) §4 is
  complete.
- **Rule of this document:** the clauses describe an **abstract counted
  signal**, independently of atomics, memory orderings, or the candidate
  algorithm. The design and its evidence must satisfy the clauses; they do
  not define them.
- **Clause IDs are load-bearing.** Tests, models, and documentation cite these
  IDs. Do not renumber an existing clause; append a new one to its group.
- **Handle-decision status:** accepted for CountedSignal and EventFlags. The H
  clauses freeze sole-role `Send + !Sync` handles with `&self` hot-path
  operations. A future shareable/multi-producer signal requires a separate
  contract, type, algorithm, and evidence case. See §8.

## 1. Abstract model

A **signal** records identical, payload-free occurrences from one producer
handle for one consumer handle. Its abstract state is one of:

- `Exact(n)`, where `0 <= n < u32::MAX`; or
- `Saturated`, representing at least `u32::MAX` occurrences.

A new signal starts in `Exact(0)`. It has two operations:

- `increment() -> ()` — producer;
- `take_count() -> CountSnapshot` — consumer.

Both operations are **linearizable**: every completed call takes effect once at
some instant between invocation and return. An occurrence belongs to the take
interval containing its `increment` linearization point, regardless of when
the call was invoked.

## 2. Increment clauses (I)

- **I1.** If the state is `Exact(n)` for `n < u32::MAX - 1` at an
  `increment` linearization point, the state becomes `Exact(n + 1)`.
- **I2.** If the state is `Exact(u32::MAX - 1)` at an `increment`
  linearization point, the state becomes `Saturated`.
- **I3.** If the state is `Saturated` at an `increment` linearization point,
  it remains `Saturated`. Saturation never wraps or returns to an exact state
  except through `take_count` (T2).
- **I4.** `increment` has no rejection or failure result. Every completed call
  linearizes exactly once; while saturated, further calls are represented by
  the already-observable saturated state rather than by an exact excess count.

## 3. Take clauses (T)

- **T1.** If the state is `Exact(n)` at a `take_count` linearization point,
  the call returns a snapshot whose `count()` is `n` and whose
  `is_saturated()` is `false`, then the state becomes `Exact(0)`.
- **T2.** If the state is `Saturated` at a `take_count` linearization point,
  the call returns a snapshot whose `count()` is `u32::MAX` and whose
  `is_saturated()` is `true`, then the state becomes `Exact(0)`.
- **T3.** A concurrent `increment` linearizes either before a take and is
  represented by that take's snapshot, or after it and belongs to the following
  interval. It is never represented in both intervals and, subject to
  saturation's explicit loss of excess precision (A2), never disappears from
  both.
- **T4.** Taking an empty signal returns exact zero and leaves it empty.

## 4. Accounting and overload clauses (A)

- **A1.** A non-saturated snapshot with count `n` represents exactly `n`
  `increment` calls linearized since the preceding take, or since
  initialization for the first take. No count is fabricated and no call is
  duplicated.
- **A2.** A saturated snapshot represents at least `u32::MAX` `increment`
  calls in that interval. The exact excess is deliberately unrecoverable, but
  the fact that exact accounting was lost remains observable in the snapshot.
- **A3.** Saturation evidence is sticky until the take that returns it. Neither
  an additional increment nor counter-width overflow can erase that evidence.

## 5. Boundedness clauses (B)

Algorithmic bounds live here; instruction counts remain measured claims tied
to a target, toolchain, and reference environment.

- **B1.** `increment` performs a statically bounded amount of work independent
  of signal history and consumer activity: at most one contention retry from
  the sole consumer's reset (no unbounded retry loop), no dynamic allocation,
  no user code, and no wait for the consumer.
- **B2.** `take_count` performs a statically bounded amount of work independent
  of the number of increments in the interval and producer activity: no retry
  loop, no dynamic allocation, no user code, and no wait for the producer.
- **B3.** No panic is reachable from either hot-path operation.

## 6. Handle clauses (H)

- **H1.** At most one producer handle and at most one consumer handle are
  active per signal. Acquisition is fallible and non-panicking; a second
  acquisition of an active role fails.
- **H2.** Handles are `Send + !Sync`. They may move between execution contexts
  but may not be shared between them. An `&self` hot-path receiver does not
  weaken this ownership rule.
- **H3.** Dropping a handle makes its role re-acquirable without resetting the
  signal. After reacquisition, all I/T/A clauses continue from the existing
  state.
- **H4.** A signal is constructible in static storage on the normal build.

## 7. What the contract does not promise (X)

- **X1.** Ordering or identity for individual occurrences. Only their count in
  a take interval is retained.
- **X2.** Payload publication or memory visibility for unrelated data. An
  occurrence carries no payload.
- **X3.** The exact number of occurrences beyond saturation. A2 promises a
  lower bound and observable loss of precision, not an unbounded total.
- **X4.** Multiple producers, multiple consumers, or a shareable handle.
- **X5.** A universal wall-clock bound. B1–B3 are algorithmic guarantees;
  instruction and latency claims are environment-specific measurements.

## 8. Shared handle decision (H-decision)

The accepted answer shared with EventFlags is H1–H4: sole-role
`Send + !Sync` handles with `&self` hot-path operations and state resident in
the signal. For CountedSignal, this is a correctness choice rather than an API
style preference. The bounded candidate implementation confirms saturation with
a compare-exchange and advances below the sentinel the same way. Sole-producer
ownership means the consumer is the only possible intervening writer and can
only reset the state, so there is at most one contention retry and the RMW
cannot wrap; a second raiser would invalidate the no-wrap proof and the current
evidence for B1.

The independent LatestBuf A.3 decision currently recommends the same handle
shape. This closes H for EventFlags and CountedSignal only. Whether the
independent LatestBuf A.3 choice later becomes one crate-wide doctrine remains
a separate maintainer decision on issue #26.

## 9. Evidence map

| Clauses | Evidence on `candidate/counted-signal` |
|---|---|
| I1, T1, T4, A1 | `increments_accumulate_and_take_clears`; the threaded `concurrent_takes_do_not_lose_increments` sum check |
| I2–I3, T2, A2–A3 | `saturates_instead_of_wrapping`; Loom's `counted_signal_saturation_boundary_is_linearizable` model |
| T1–T3, A1 | Loom's `counted_signal_take_partitions_increments` model; threaded take stress |
| T3, A1 (post-take / stale MAX) | Loom's `counted_signal_post_take_increment_observes_reset_epoch` model (seeded at `MAX`; Relaxed gate only) |
| B1–B2 | Source review (≤1 contention retry under H1); eight-target gated code-size rows blessed post-fix (46–76 B `increment`); Cortex-M3 cycles pending re-measure after sentinel CAS |
| B3 | Source review plus normal, Miri, Loom, and embedded-target executions of both hot paths |
| H1, H3 | `handles_are_exclusive_and_reusable_after_drop`, including state continuation after reacquisition |
| H2 | `handles_are_send`; producer and consumer compile-fail doctests pin `!Sync` |
| H4 | `const_new_works_in_static_context` |

The candidate's relaxed atomic orderings and its sole-producer CAS proof are
implementation evidence for this abstract contract. They remain in
[`counted-signal.md`](counted-signal.md) §3.1 and the module documentation,
not in the semantic clauses above.

The candidate tree passed the complete pinned reference matrix with zero skips
on 2026-08-11: CI/features/docs/deny/coverage, Miri host and proxy targets, all
Loom models, eight-target code size, embedded checks, and QEMU cycles. Re-run
after the MAX short-circuit fix before treating that stamp as current.
