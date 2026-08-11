# Exploratory Primitives: A Bounded-Handoff Taxonomy for ph-eventing

- **Status:** Exploratory design foundation — deliberately less detailed than
  the [`LatestBuf` proposal](latest-buf.md); these are candidates for
  triage and exploration, not designs ready for a contract.
- **Received:** 2026-08-11 (0.3.0 cycle; triage in
  [`../0.3.0-candidates.md`](../0.3.0-candidates.md) §4)

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

This is the triple-buffered ownership-transfer primitive already discussed
(in full: [`latest-buf.md`](latest-buf.md), with its semantic contract in
[`latest-buf-contract.md`](latest-buf-contract.md)).

It fits data for which intermediate states have no independent value:

- most recent orientation estimate;
- current control setpoint;
- latest power measurement;
- most recent GNSS fix;
- current actuator status;
- newest decoded sensor frame.

Its contract would be:

- producer always publishes in bounded time;
- unread publication may be replaced;
- consumer receives only a complete value;
- delivered generations are monotonic;
- skipped publications are reported;
- producer and consumer never access the same slot concurrently.

This is freshness-oriented but not a stream history. It complements rather
than directly replaces `SeqRing`.

## 2. BlockBuf: complete window handoff

A block primitive is likely one of the strongest additions for embedded DSP.

Many producers naturally operate in blocks:

- IMU FIFO bursts;
- ADC DMA half/full buffers;
- audio frames;
- vibration-analysis windows;
- CAN receive batches;
- sampled power waveforms;
- image-sensor lines or tiles.

Conceptually:

```rust
pub struct Block<T, Stamp, const N: usize> {
    pub first_sequence: u32,
    pub last_sequence: u32,
    pub samples: [Timestamped<T, Stamp>; N],
}
```

A producer fills a privately owned block. Once complete, ownership is
published atomically. The consumer can never see a partially filled block.

Possible overload policies should be separate types or modes fixed at
compile time:

- **`LatestBlockBuf`:** replace an unread block with the newest complete
  block.
- **`QueuedBlockBuf`:** reject publication when every block is owned.
- **`DropBlockBuf`:** explicitly discard the newly completed block when
  unavailable.

This is particularly attractive for DSP because each delivered block can be
internally contiguous even if whole blocks are skipped. The DSP sees:

```
block 100–131
block 196–227
```

It knows exactly where the discontinuity occurred and can reset,
interpolate, or classify that analysis window as incomplete.

The synchronization operation is constant, although filling the block
naturally takes N producer calls. Memory use is larger but fixed and
obvious.

## 3. SlotPool: bounded zero-copy ownership transfer

For larger payloads, copying through a queue can dominate the transfer cost.
A statically allocated slot pool could transfer ownership instead.

```
free slot → producer reservation → committed slot
                                   ↓
                              consumer claim
                                   ↓
                              released slot
```

Representative uses include:

- DMA buffers;
- large FIFO bursts;
- encoded packets;
- audio blocks;
- protocol frames;
- diagnostic snapshots.

The producer attempts to reserve a slot:

```rust
match producer.try_reserve() {
    Some(grant) => {
        // Fill exclusively owned memory.
        grant.commit(metadata);
    }
    None => {
        // Explicit overload policy.
    }
}
```

The consumer claims a committed slot and releases it when finished.

The important safety property is that memory is transferred through
ownership states:

```
Free → ProducerOwned → Published → ConsumerOwned → Free
```

No slot can be written while the consumer owns it. No slot can be read
before the producer commits it.

This would provide a reusable foundation for `BlockBuf`, DMA handoff, and
potentially a future sound freshness-first queue.

The difficult areas are:

- cancellation when a producer drops an uncommitted grant;
- consumer release on early return;
- initialization tracking;
- DMA memory visibility and target cache maintenance;
- generation tags preventing stale-handle reuse; and
- ensuring reservation never contains an unbounded scan.

RAII grants can make cancellation safe, while the actual cache/DMA
operations should remain outside ph-eventing because they are
target-specific effects.

## 4. EventFlags: coalesced condition notification

Some producer-consumer communication is not a stream at all.

An interrupt may need to communicate conditions such as:

- FIFO watermark reached;
- DMA block complete;
- peripheral error;
- configuration changed;
- shutdown requested;
- watchdog warning;
- data available.

Repeated notification of the same condition may have no additional value. An
atomic bitset is a natural primitive:

```rust
producer.raise(Event::DataReady);
producer.raise(Event::Overflow);

let events = consumer.take_all();
```

A possible representation is an `AtomicU32`:

- producer uses `fetch_or`;
- consumer uses `swap(0)`;
- duplicate events coalesce;
- producer work is constant;
- no allocation or queue capacity is required.

This is an excellent ISR-to-task primitive when ordering and multiplicity do
not matter.

Its contract must say explicitly:

- conditions are unordered;
- duplicate raises may coalesce;
- each bit means "occurred at least once since the last take";
- clearing and raising must not lose a concurrent event.

This is much narrower and more predictable than adding a general semaphore
abstraction.

## 5. CountedSignal: multiplicity without payloads

Sometimes duplicates matter, but their individual payloads do not.

Examples include:

- encoder pulses;
- timer expirations;
- dropped packet counts;
- completed conversions;
- retry requests;
- accumulated wakeups.

A counted signal could use a saturating atomic counter:

```rust
producer.increment();

let count = consumer.take_count();
```

Its contract might be:

- increments are bounded;
- the consumer atomically takes the current count;
- overflow saturates instead of wrapping;
- saturation is observable;
- ordering between individual increments is intentionally absent.

This avoids allocating a queue entry for every identical event.

A multi-counter form could provide a fixed array of event classes, but it
should remain statically sized and avoid a general dynamic registry.

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

### LatestSource

```rust
pub trait LatestSource<T> {
    fn try_take_latest(&mut self) -> Option<LatestItem<T>>;
}
```

For freshness-oriented channels that report skipped publications.

### ObservedSource

```rust
pub trait ObservedSource<T> {
    type Observation;

    fn try_pop_observed(
        &mut self,
    ) -> Option<(T, Self::Observation)>;
}
```

For sources that provide ordering, gaps, filtering, or transport-loss
metadata.

A standardized observation might eventually contain:

```rust
pub struct DeliveryObservation {
    pub ordinal: u32,
    pub skipped_before: u32,
    pub filtered_before: u32,
}
```

This should be introduced only if multiple primitives can implement the
vocabulary honestly.

### LatestSink

```rust
pub trait LatestSink<T> {
    fn publish_latest(&mut self, value: T) -> PublishReport;
}
```

For producers that always publish but may replace an unread value.

### SignalSource and SignalSink

```rust
pub trait SignalSink<S> {
    fn raise(&self, signal: S);
}

pub trait SignalSource<S> {
    fn take_pending(&self) -> S;
}
```

These describe coalesced conditions rather than event streams.

### ReservableSink

Conceptually:

```rust
pub trait ReservableSink<T> {
    type Grant<'a>
    where
        Self: 'a;

    fn try_reserve(&mut self) -> Option<Self::Grant<'_>>;
}
```

The grant gives exclusive access to statically owned storage and must be
committed or automatically cancelled.

The concrete grant API requires careful design to keep initialization and
cancellation safe.

### ClaimSource

```rust
pub trait ClaimSource<T> {
    type Claim<'a>
    where
        Self: 'a;

    fn try_claim(&mut self) -> Option<Self::Claim<'_>>;
}
```

A claim borrows or owns a queued slot until it is released. This supports
zero-copy consumers without exposing raw shared storage.

### Sequenced and Timestamped

```rust
pub trait Sequenced {
    type Sequence: Copy;

    fn sequence(&self) -> Self::Sequence;
}

pub trait Timestamped {
    type Timestamp: Copy;

    fn timestamp(&self) -> Self::Timestamp;
}
```

These let generic consumers inspect producer-owned identity without
prescribing sequence width, time units, or a clock source.

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

1. `LatestBuf` for latest-complete-value handoff.
2. `BlockBuf` for IMU, ADC, audio, and DMA windows.
3. `SlotPool` as the ownership foundation for large zero-copy transfers.
4. `EventFlags` and `CountedSignal` for bounded ISR notification.
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
