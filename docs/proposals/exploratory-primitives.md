# Exploratory Primitives: A Bounded-Handoff Taxonomy for ph-eventing

- **Status:** Exploratory design foundation — deliberately less detailed than
  the [`LatestBuf` proposal](latest-buf.md); these are candidates for
  triage and exploration, not designs ready for a contract.
- **Received:** 2026-08-11 (0.3.0 cycle; triage in
  [`../0.3.0-candidates.md`](../0.3.0-candidates.md) §4)
- **Structure note (2026-08-11):** the substantive design text for the
  Tier 1/2 primitives has moved to per-primitive design documents —
  [`block-buf.md`](block-buf.md), [`slot-pool.md`](slot-pool.md),
  [`event-flags.md`](event-flags.md), [`counted-signal.md`](counted-signal.md)
  — where exploration happens. This document remains the taxonomy: the
  four questions, the table, the tiering rationale, the trait index, and
  the standing filters. `LatestBuf` was already owned by its own proposal
  and contract; its entry here is a cross-reference.

The broader opportunity for ph-eventing is to become a small collection of
explicitly bounded handoff mechanisms — not a general event bus or executor.

Each primitive should answer four questions:

1. What information is retained?
2. What happens under overload?
3. Which context owns each piece of memory?
4. What execution-time and memory bounds can be measured?

## A useful primitive taxonomy

| Primitive | Retention policy | Overload behavior | Representative use |
|-----------|-----------------|-------------------|--------------------|
| `EventBuf` | Every admitted item, FIFO | Reject newest | Commands, control events |
| `SeqRing` | Most recent ordered window | Overwrite oldest | High-rate telemetry |
| `LatestBuf` | Latest complete value | Replace unread value | State snapshots |
| `BlockBuf` | Latest complete block | Replace/reject whole block | DMA, IMU/DSP windows |
| `SlotPool` | Every admitted owned buffer | Reservation fails | Large zero-copy payloads |
| `EventFlags` | One bit per condition | Repeated signals coalesce | ISR-to-task notification |
| `CountedSignal` | Count per condition | Counter saturates | Pulse/event accumulation |
| `FanoutRing` | Per-consumer recent window | Independent consumer loss | DSP plus logger/telemetry |
| `PriorityBuf` | FIFO within fixed lanes | Lane-specific policy | Control mixed with telemetry |

The important point is that these are different contracts. They should remain
separate types rather than one configurable "universal channel" with runtime
policy branches.

## 1. LatestBuf: latest complete state

The triple-buffered ownership-transfer primitive already discussed. **This
entry is a cross-reference, not a design:** the full proposal lives at
[`latest-buf.md`](latest-buf.md) and its semantic contract at
[`latest-buf-contract.md`](latest-buf-contract.md). Where this taxonomy and
those documents differ, they are authoritative — contract clauses over
prose.

It fits data for which intermediate states have no independent value:
orientation estimates, control setpoints, power measurements, GNSS fixes,
actuator status, the newest decoded sensor frame.

Its contract, by clause: bounded always-successful publication (P1–P2, B1);
observable replacement of an unread publication (P4–P5); only complete
values delivered (P6, C6); monotonic delivered generations (C5); skipped
publications reported (C3); producer and consumer never access the same
payload bytes concurrently (proposal §14).

This is freshness-oriented but not a stream history. It complements rather
than directly replaces `SeqRing`.

## 2. BlockBuf: complete window handoff

**Design document: [`block-buf.md`](block-buf.md)** (the substantive design
text has moved there; this entry is the summary).

Complete-window handoff for producers that naturally operate in blocks —
IMU FIFO bursts, ADC DMA half/full buffers, audio frames, analysis windows.
A producer fills a privately owned block; once complete, ownership is
published atomically, so the consumer can never see a partially filled
block. Overload policies (`Latest` / `Queued` / `Drop`) are separate
compile-time types. Each delivered block is internally contiguous even when
whole blocks are skipped, so a DSP knows exactly where a discontinuity
occurred. Likely one of the strongest additions for embedded DSP; coupled
to the LatestBuf contract's decision point D3.

## 3. SlotPool: bounded zero-copy ownership transfer

**Design document: [`slot-pool.md`](slot-pool.md)** (the substantive design
text has moved there; this entry is the summary).

For payloads large enough that copying dominates transfer cost, a
statically allocated pool transfers ownership instead: slots move through
`Free → ProducerOwned → Published → ConsumerOwned → Free`, with RAII grants
on the producer side and claims on the consumer side. No slot can be
written while the consumer owns it; none can be read before the producer
commits it. A candidate foundation for `BlockBuf`, DMA handoff, and future
zero-copy primitives. The hard parts — grant cancellation, initialization
tracking, stale-handle generations, bounded reservation — are catalogued in
the design document; DMA cache maintenance stays outside the crate.

## 4. EventFlags: coalesced condition notification

**Design document: [`event-flags.md`](event-flags.md)** (the substantive
design text has moved there; this entry is the summary).

Some producer-consumer communication is not a stream at all: an ISR
signalling conditions (watermark reached, DMA complete, error, shutdown)
where repeated notification adds no value. An atomic bitset —
`fetch_or` to raise, `swap(0)` to take — coalesces duplicates, costs
constant producer work, and needs no queue capacity. Contract in one line:
conditions unordered, duplicates coalesce, each bit means "occurred at
least once since the last take," and clearing must never lose a concurrent
raise. Much narrower and more predictable than a general semaphore
abstraction.

## 5. CountedSignal: multiplicity without payloads

**Design document: [`counted-signal.md`](counted-signal.md)** (the
substantive design text has moved there; this entry is the summary).

For events whose duplicates matter but whose individual payloads do not —
encoder pulses, timer expirations, drop counts. A saturating atomic
counter: bounded increments, atomic take, saturation instead of wrap (and
saturation observable), with ordering between increments intentionally
absent. Avoids allocating a queue entry per identical event. A multi-counter
form stays statically sized; a dynamic registry is pre-rejected.

## 6. PriorityBuf: fixed-priority lanes

A single FIFO can create priority inversion when control messages sit behind
telemetry.

For example:

```
telemetry
telemetry
telemetry
emergency-stop
```

A bounded priority channel could provide two or three fixed lanes:

- critical;
- control;
- telemetry.

The consumer follows a documented polling rule, such as:

1. take at most one critical event;
2. take at most two control events;
3. take at most four telemetry events;
4. repeat.

This avoids starving lower-priority traffic while bounding how long urgent
traffic waits behind it.

However, this may not require a new low-level queue. It could initially be a
composition helper around several existing `EventBuf` instances. Separate
queues make capacity, ordering, and overload behavior easier to inspect.

A fully generic priority heap would be a poor fit: it adds variable work,
comparison logic, and more complicated timing.

## 7. FanoutRing: one producer, several independent consumers

Telemetry often has more than one destination:

```
                  ┌─▶ DSP
IMU acquisition ──┼─▶ diagnostic logger
                  └─▶ live telemetry output
```

The producer should not have to push the same sample into three unrelated
queues, and a slow logger should not delay the DSP.

A bounded fanout primitive could provide:

- one producer;
- a compile-time number of consumers;
- one cursor per consumer;
- independent drop accounting;
- no consumer dependency on another consumer;
- producer work bounded by the fixed consumer count or independent of it.

This is significantly harder than SPSC because slot reuse depends on
multiple consumer positions. A sound design needs to determine when no
consumer still owns or may read a slot.

Possible approaches include:

- per-consumer cursors with a retained immutable slot window;
- atomically owned reference counts;
- static index pools; or
- separate queues fed by a bounded fanout stage.

The last option may be preferable initially. Fanout is valuable, but it
should be deferred until at least two concrete consumers demonstrate that
duplicated queues are an actual cost.

## 8. Count-based rate adaptation

Producer and consumer rates may be intentionally different even without
overload.

Useful deterministic adapters could include:

- pass every Nth event;
- form fixed-size blocks;
- retain the latest value per externally supplied tick;
- route every Nth event to a secondary observer;
- compute a fixed-count moving selection.

These must be count-based unless the caller explicitly supplies time.
ph-eventing should not read a clock implicitly.

For example:

```rust
let mut decimator = EveryNth::<_, 4>::new(sink);
```

Every fourth item is forwarded. The others are intentionally classified as
**filtered** rather than **lost**.

This distinction matters:

- **Loss** means the transport could not retain data.
- **Filtering** means the declared policy intentionally selected a subset.

The adapter should report both separately.

Arbitrary transformation closures are less attractive because their cost
cannot be bounded by ph-eventing. A generic map or filter may be useful, but
it must not inherit the crate's deterministic-performance claims merely by
implementing a crate trait.

## 9. Deadline- or age-aware consumption

Commands and observations sometimes become useless when stale.

Examples include:

- motor setpoints;
- actuator commands;
- UI state transitions;
- network-derived control updates;
- temporary calibration requests.

A deadline-aware source could discard expired entries when the caller
supplies the current time:

```rust
source.try_pop_valid(now, max_discards)
```

The clock remains external. `max_discards` bounds the work performed in one
call.

The result should distinguish:

- no item available;
- valid item returned;
- expired items discarded;
- more expired work remains.

This is less central to high-speed telemetry, but it can prevent stale
control work from being executed merely because it was admitted earlier.

## 10. Request/response pairing

An MCU often centralizes ownership of a peripheral in one task:

```
application task → request queue → peripheral-owner task
application task ← response queue ← peripheral-owner task
```

A lightweight request/response helper could compose two bounded channels
with correlation identifiers.

Useful cases include:

- flash service;
- configuration storage;
- shared bus manager;
- radio service;
- secure-element access;
- background checksum or compression work.

This should probably remain a composition of two queues rather than a new
synchronization algorithm. The value would be in:

- bounded correlation identifiers;
- static request slots;
- explicit cancellation;
- no heap-backed futures;
- no dependency on an async executor.

The design becomes much more complicated if it promises arbitrary concurrent
callers, so a first version should remain single-requester/single-service.

## Additive trait directions

The existing `Sink`, `Source`, and `Link` traits should remain minimal. New
traits should express genuinely different operations rather than attach
aspirational labels such as `RealTime` or `NonBlocking`.

Rust cannot verify a marker trait's execution-time claim.

Traits ride with the primitive that demonstrates their need, so the
sketches live in the owning documents; this table is the index:

| Trait | Owning document | Note |
|-------|-----------------|------|
| `LatestSink`, `LatestSource` | [`latest-buf.md`](latest-buf.md) §12 | Producers that always publish but may replace an unread value; freshness sources that report skips |
| `ObservedSource` | [`latest-buf.md`](latest-buf.md) §12.3 | Cross-primitive vocabulary, gated on two honest implementations. **Known divergence:** this taxonomy originally sketched a wider `DeliveryObservation` (`ordinal`, `skipped_before`, `filtered_before`) than the proposal's (`sequence`, `skipped_before`); the `filtered_before` field earns its place only if the filtering adapters (§8) ship. Reconcile at the two-implementations gate, not before |
| `SignalSink`, `SignalSource` | [`event-flags.md`](event-flags.md) §2 | Coalesced conditions, not streams; to be proven against both `EventFlags` and `CountedSignal` before the vocabulary freezes |
| `ReservableSink`, `ClaimSource` | [`slot-pool.md`](slot-pool.md) §2 | Grant/claim lifecycles for zero-copy transfer |
| `Sequenced`, `Timestamped` | [`latest-buf.md`](latest-buf.md) §12.4 | Optional payload-metadata conveniences: generic consumers inspect producer-owned identity without the crate prescribing sequence width, time units, or a clock |

## What should remain outside ph-eventing

To preserve its architectural value, the crate should avoid becoming
responsible for:

- task scheduling or executors;
- hardware access;
- interrupt registration;
- clocks and timers;
- dynamic subscriptions;
- general pub/sub routing;
- arbitrary priority heaps;
- serialization;
- DSP algorithms;
- DMA cache maintenance;
- application-specific retry policies; or
- opaque transformation pipelines.

A useful admission rule is:

> Add a primitive when it isolates a recurring bounded ownership or overload
> problem shared by multiple producer/consumer integrations.

If the feature primarily contains domain behavior, it belongs in the
integration application or endpoint crate.

## Suggested exploration order

The strongest near-term candidates appear to be:

1. [`LatestBuf`](latest-buf.md) for latest-complete-value handoff.
2. [`BlockBuf`](block-buf.md) for IMU, ADC, audio, and DMA windows.
3. [`SlotPool`](slot-pool.md) as the ownership foundation for large
   zero-copy transfers.
4. [`EventFlags`](event-flags.md) and [`CountedSignal`](counted-signal.md)
   for bounded ISR notification.
5. `LatestSource` and carefully scoped observational metadata.
6. Count-based batching and decimation adapters.

`FanoutRing`, request/response helpers, priority composition, and
deadline-aware sources are plausible later additions, but they introduce
more policy and should wait for concrete adopters.

The unifying idea is that ph-eventing should not promise one universal
transfer model. It should offer a small set of named, measurable choices:

```
preserve admitted data
preserve the newest window
preserve only the newest state
preserve complete blocks
transfer ownership without copying
coalesce equivalent signals
count repeated signals
```

Each choice should make its loss, ordering, ownership, and progress behavior
obvious from the type selected.
