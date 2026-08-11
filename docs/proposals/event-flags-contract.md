# EventFlags semantic contract

- **Status:** Frozen for evaluation on the `candidate/event-flags` lane.
- **Rule of this document:** these clauses describe an abstract coalescing
  condition set independently of atomics, memory orderings, or the candidate
  algorithm. The design and evidence must satisfy the clauses; they do not
  define them.
- **Clause IDs are load-bearing.** Tests, models, and documentation cite these
  IDs. Do not renumber an existing clause; append a new one to its group.
- **Decision status:** the shared handle decision and EventFlags-specific
  width, mask, observation, and trait decisions are closed for this candidate.
  See §9.

## 1. Abstract model (M)

An EventFlags value records an unordered set of payload-free conditions from
one producer handle for one consumer handle. The condition namespace contains
exactly 32 members, represented by bits 0 through 31 of an `EventMask`.

- **M1.** A new EventFlags value has an empty pending set.
- **M2.** `raise(mask)` and `take_all()` are linearizable. Every completed call
  takes effect exactly once at an instant between invocation and return.

## 2. Raise clauses (R)

- **R1.** At `raise(mask)`'s linearization point, pending becomes the set union
  of its prior value and `mask`. Raising the empty set changes no condition.
- **R2.** Pending records only whether each condition occurred. Duplicate
  raises may coalesce; multiplicity is not observable.

## 3. Take clauses (T)

- **T1.** `take_all()` returns exactly the pending set immediately before its
  linearization point and makes pending empty at that point.
- **T2.** A take linearized while pending is empty returns the empty set and
  leaves it empty.

## 4. Window and conservation clauses (C)

- **C1.** For each condition, a take contains it if and only if at least one
  matching raise linearized after the preceding take and before this take, or
  after initialization for the first take.
- **C2.** A raise racing a take linearizes wholly before or wholly after it. If
  the raise is first, the condition is in that take; otherwise it remains
  pending for a later take. Clearing never erases the later raise.
- **C3.** A take fabricates no condition and returns no individual raise in two
  take windows. Multiple matching raises may legitimately make the same bit
  appear in separate windows; within one window they may coalesce under R2.

## 5. Publication clause (S)

- **S1.** Memory actions sequenced before a raise happen-before memory actions
  sequenced after a take that observes any condition from that raise.
  EventFlags publishes readiness for application-owned state; the state itself
  is not stored in the mask.

## 6. Boundedness clauses (B)

The clauses below count one target atomic RMW as one signal-word operation.
Target-specific instruction and interrupt-masked windows remain measured claims
tied to a compiler and architecture.

- **B1.** `raise` performs one signal-word `fetch_or`, with no source-level
  retry loop, allocation, callback, panic path, or work proportional to
  history, occupancy, or the number of set bits.
- **B2.** `take_all` performs one signal-word `swap(0)`, with the same bounds
  independent of producer activity and the number of pending conditions.
- **B3.** Producer work never waits for or invokes consumer work, and consumer
  work never waits for or invokes producer work.

## 7. Handle clauses (H)

- **H1.** At most one producer handle and at most one consumer handle are
  active per EventFlags value. Acquisition is fallible and non-panicking; a
  second acquisition of an active role fails.
- **H2.** Handles are `Send + !Sync`. They may move between execution contexts
  but may not be shared between them. An `&self` hot-path receiver does not
  weaken this ownership rule.
- **H3.** Dropping a handle makes only its role re-acquirable without resetting
  pending state. After reacquisition, all M/R/T/C/S clauses continue from the
  existing state.
- **H4.** EventFlags is constructible in static storage on the normal build.

## 8. Width and representation clauses (W)

- **W1.** `EventMask` is a transparent, dependency-free `u32` set containing
  exactly 32 conditions. Raw conversion is explicit and preserves every bit.
- **W2.** Constructing a one-condition mask from an external index is
  panic-free: indices `0..32` succeed and all other values return `None`.

## 9. Accepted candidate decisions

- **H:** sole-role `Send + !Sync` producer/consumer handles with `&self`
  operations, accepted once for EventFlags and CountedSignal on issue #30.
- **D2:** transparent `EventMask(u32)`. The exploratory enum mapping retained
  runtime range checks and could not prove that two variants did not alias;
  the transparent mask erased to the raw operation within two bytes.
- **D3:** exactly 32 conditions. A generic word width would expose
  target-specific atomic availability and fallback cost as public API.
- **D4:** no non-clearing peek. An advisory snapshot invites check-then-act
  reasoning that cannot be upheld across a concurrent take or raise.
- **D5:** no stream `Sink`/`Source`/`Link` implementation. Coalesced state is
  not an item stream. Signal traits remain deferred: CountedSignal's
  increment/count-snapshot vocabulary disproves the original single generic
  `SignalSink<S>`/`SignalSource<S>` sketch as an honest shared surface.

## 10. What the contract does not promise (X)

- **X1.** FIFO order, timestamps, identity, or multiplicity for conditions.
- **X2.** Payload storage. S1 is publication for separately-owned memory.
- **X3.** More than 32 conditions or a target-dependent word width.
- **X4.** Multiple producers, multiple consumers, or shareable handles.
- **X5.** A non-clearing observation method or stream/signal trait vocabulary.
- **X6.** A universal wall-clock bound. B1-B3 are algorithmic guarantees;
  instruction and interrupt-latency claims are environment-specific evidence.

## 11. Evidence map

| Clauses | Evidence on `candidate/event-flags` |
|---|---|
| M1, R1-R2, T1-T2, C1, C3, W1-W2 | `event_mask_is_an_explicit_panic_free_32_bit_set`, `duplicate_raises_coalesce_and_take_clears`, `multi_bit_and_all_bit_masks_round_trip`, and `empty_raise_and_empty_take_are_no_ops` |
| C1-C3 | Loom's `event_flags_raise_racing_take_is_partitioned_exactly` and `event_flags_distinct_raises_partition_across_takes`; native/Miri threaded stress |
| S1 | Loom's `event_flags_observed_raise_publishes_payload`; the model fails with either Release or Acquire independently weakened to Relaxed |
| B1-B3 | Source review; eight-target gated code-size rows; Cortex-M3 QEMU rows; `event-flags-atomic-window.sh` disassembly checks for thumbv6m and ESP32-S2/S3 |
| H1, H3 | `handles_are_exclusive_and_reusable_after_drop`, including pending-state continuation |
| H2 | `handles_are_send_and_container_is_sync`; producer and consumer compile-fail doctests pin `!Sync` |
| H4 | `const_new_works_in_static_context` |

The candidate uses Release `fetch_or` and Acquire `swap(0)`. Those orderings
are implementation evidence for S1, not abstract operations added to the
contract. The mutation checks are recorded because bit conservation alone
cannot distinguish Relaxed from the accepted publication pair.
