# Proposal: Freshness-First SPSC Transfer Primitives for ph-eventing

- **Status:** Exploratory design proposal
- **Scope:** Additive primitives and traits only
- **Compatibility:** No changes to existing `Sink`, `Source`, or `Link` traits
- **Implementation status:** Not implemented
- **Received:** 2026-08-11 (0.3.0 cycle; see `docs/0.3.0-candidates.md` §3)

## 1. Summary

ph-eventing exists to isolate the difficult integration layer between
high-rate producers and variably scheduled consumers.

Its core promise is not merely to provide ring buffers. It is to keep
queueing, synchronization, overload handling, and concurrency reasoning out
of producer crates such as IMU drivers and out of consumer crates such as DSP
or motion-analysis libraries.

The intended properties are:

- `#![no_std]`;
- no dynamic allocation;
- fixed, measurable memory use;
- bounded and predictable operations;
- no producer dependence on consumer progress;
- explicit overload behavior;
- observable loss where loss is permitted;
- deterministic ordering of delivered data;
- independently replaceable producer and consumer integrations; and
- evidence-backed safety and performance claims.

The existing `SeqRing` closely matches the desired freshness-first behavior:
the producer never waits, old samples may be overwritten, and the consumer
receives an ordered recent window with drop accounting. However, its current
seqlock implementation contains a known formal data race and therefore cannot
be proven sound under Rust's memory model.

This proposal introduces a complementary primitive based on exclusive slot
ownership rather than concurrent slot access. The primitive would retain only
the latest complete publication — or latest complete block — while allowing
intermediate publications to be discarded deliberately and observably.

The working name used here is **`LatestBuf`**.

## 2. Motivation

Consider a high-rate telemetry producer such as an IMU:

```
IMU hardware FIFO → acquisition task/ISR → transfer primitive → DSP
```

The IMU produces samples at a configured output data rate. The acquisition
side must drain the hardware FIFO before it overflows and must return control
to the MCU within a predictable amount of time.

The downstream DSP behaves differently. Its execution rate may vary because
of:

- scheduling;
- other interrupts;
- varying algorithmic work;
- different processing-window boundaries;
- power-management transitions; or
- temporary contention for CPU time.

The producer therefore cannot safely depend on the consumer being ready for
every sample.

The central design requirement is:

> Transfer ordered high-rate telemetry without allowing delayed consumer
> progress to create unbounded or workload-dependent producer latency.

For many telemetry applications, losing an occasional sample or block is
preferable to delaying acquisition, overflowing the hardware FIFO, or
preventing the rest of the MCU from making progress.

The transport must consequently optimize for system liveness and freshness
rather than guaranteed delivery.

## 3. Architectural Boundary

Without a dedicated integration crate, the transfer mechanism tends to leak
into one of its endpoints.

If implemented inside the IMU driver, that driver becomes responsible for:

- atomic ordering;
- queue ownership;
- backpressure;
- drop accounting;
- target-specific concurrency;
- consumer scheduling assumptions; and
- proving lock-free or bounded behavior.

Those responsibilities do not belong to a peripheral driver.

If implemented inside the DSP crate, the DSP becomes responsible for the same
concurrency machinery in addition to proving its signal-processing
algorithms.

That responsibility does not belong to an analysis library either.

ph-eventing intentionally accepts this maintenance burden so that both
endpoints remain narrow:

```
┌────────────────┐     ┌────────────────────┐     ┌────────────────┐
│ IMU / producer │ ──▶ │    ph-eventing     │ ──▶ │ DSP / consumer │
│                │     │                    │     │                │
│ acquisition    │     │ ownership          │     │ algorithms     │
│ decoding       │     │ synchronization    │     │ windows        │
│ timestamps     │     │ overload policy    │     │ gap handling   │
└────────────────┘     │ loss observation   │     └────────────────┘
                       └────────────────────┘
```

The producer and consumer should depend on transport contracts, not on a
particular queue implementation.

## 4. Required Semantics

A freshness-first telemetry primitive should provide the following behavior.

### 4.1 Producer requirements

A publication operation must:

- have a statically bounded amount of work;
- contain no unbounded retry loop;
- perform no dynamic allocation;
- never wait for consumer progress;
- never invoke user code;
- avoid hidden panic paths;
- publish either one complete value or nothing;
- return an explicit report when an unread publication was displaced; and
- have a cost measurable on supported targets.

The payload copy remains proportional to `size_of::<T>()`, but that size is
fixed at compile time. Synchronization cost should remain constant with
respect to occupancy, backlog, and consumer lag.

### 4.2 Consumer requirements

A consumer operation must:

- return the latest complete publication available at its linearization
  point;
- never observe a partially written value;
- identify the publication's generation;
- report skipped intermediate publications;
- perform bounded work regardless of how far behind it is; and
- never delay the producer while processing a claimed value.

### 4.3 Ordering requirements

Delivery of every publication is not required. Ordering of delivered
publications is required.

If the producer publishes generations:

```
101, 102, 103, 104, 105
```

the consumer may observe:

```
101, 104, 105
```

but must never observe:

```
101, 104, 103
```

The observation of generation 104 should report that two intermediate
publications were skipped.

### 4.4 Reliability definition

For this primitive, "reliable" does not mean lossless.

It means:

- no torn or partially initialized payload is returned;
- no publication is silently reordered;
- skipped publications are observable;
- overload behavior is stable and documented;
- producer and consumer ownership cannot overlap;
- no hidden blocking or allocation occurs; and
- behavior matches the evidence claimed for the release.

## 5. Relationship to Existing Primitives

### 5.1 RingBuf

`RingBuf` is a single-owner history window.

It is useful where one context owns all mutation and observation. It is not
the cross-context IMU-to-DSP primitive because it does not establish
producer/consumer synchronization.

### 5.2 EventBuf

`EventBuf` is a bounded, race-free SPSC FIFO with explicit backpressure.

When full, `push` immediately returns `Err(value)`. It does not block, but
its retention policy preserves older admitted samples and rejects the newest
sample.

That is appropriate when:

- admitted events must remain FIFO;
- the producer has an explicit rejection policy;
- every accepted event matters; or
- formal memory-model soundness is required.

It may be less appropriate when the DSP values fresh state more than stale
backlog.

### 5.3 SeqRing

`SeqRing` implements the desired recent-window policy:

- producer work does not depend on the consumer;
- old samples are overwritten;
- the consumer drains retained samples in order;
- consumer lag is reported through sequence and drop statistics; and
- recovery is bounded independently of total lag.

These are valuable semantics and should remain part of the design vocabulary.

The problem is its current implementation. The producer may overwrite a slot
while the consumer copies it. A sequence re-check discards the copy when
overlap is detected, but the conflicting non-atomic accesses have already
occurred.

Volatile access constrains compiler transformations; it does not make the
access atomic. Consequently, the design contains undefined behavior under
Rust's formal memory model.

This establishes an important distinction:

- `SeqRing` demonstrates useful transfer semantics.
- It does not currently demonstrate a sound safe-Rust implementation of those
  semantics.

The known limitation should continue to be documented prominently for as long
as `SeqRing` remains available. It may eventually be replaced if Rust gains
suitable facilities or if a verifiably better algorithm is developed.

Architecture-specific assembly could contribute to such a solution only if it
creates actual atomicity or exclusive ownership and exposes that behavior
soundly to Rust. Assembly by itself does not exempt surrounding Rust code
from the language memory model.

## 6. Proposed Primitive: LatestBuf

`LatestBuf<T>` is a freshness-first SPSC snapshot channel.

Rather than permitting the producer and consumer to touch the same slot
concurrently, it maintains three exclusive ownership roles:

```
producer-owned slot | shared exchange slot | consumer-owned slot
```

At every instant:

- the producer exclusively owns one writable slot;
- the consumer exclusively owns one readable slot; and
- one slot is owned by an atomic exchange state.

Neither endpoint accesses the exchange slot until it atomically acquires
ownership.

### 6.1 Why three slots

Two slots are insufficient if the producer must remain independent of a
consumer that may hold a value indefinitely:

- one slot may be held by the consumer;
- one slot may contain the latest publication;
- the producer still needs somewhere safe to write the next publication.

A third slot provides that writable space without waiting.

### 6.2 Conceptual state

```rust
pub struct LatestBuf<T: Copy> {
    state: AtomicU32,
    slots: [UnsafeCell<MaybeUninit<Entry<T>>>; 3],
    producer_taken: AtomicBool,
    consumer_taken: AtomicBool,
}

struct Entry<T> {
    generation: u32,
    value: T,
}
```

The shared atomic state identifies:

- which slot currently occupies the exchange position; and
- whether that slot contains an unread publication.

The generation can live inside the slot because it is written and read under
the same exclusive-ownership protocol as the payload.

### 6.3 Producer state

The producer handle owns:

```rust
pub struct Producer<'a, T: Copy> {
    buffer: &'a LatestBuf<T>,
    back: SlotIndex,
    next_generation: u32,
}
```

`back` names the slot that only this producer may access.

### 6.4 Consumer state

The consumer handle owns:

```rust
pub struct Consumer<'a, T: Copy> {
    buffer: &'a LatestBuf<T>,
    front: SlotIndex,
    last_generation: u32,
}
```

`front` names the slot that only this consumer may access.

## 7. Publication Algorithm

The producer operation would conceptually perform:

1. Increment its publication generation.
2. Write the generation and complete payload into its private back slot.
3. Atomically exchange the back-slot index into the shared state, marked
   ready.
4. Acquire the previously shared slot as its next back slot.
5. Report whether the exchanged shared slot contained an unread publication.

Pseudocode:

```rust
fn publish(&mut self, value: T) -> PublishReport {
    let generation = next_nonzero(self.next_generation);
    self.next_generation = generation;

    write_owned_slot(
        self.back,
        Entry {
            generation,
            value,
        },
    );

    let previous = self
        .buffer
        .state
        .swap(encode(self.back, Ready::Yes), Ordering::AcqRel);

    self.back = previous.slot();

    PublishReport {
        generation,
        replaced_unread: previous.ready(),
    }
}
```

The producer performs:

- one fixed-size payload write;
- one atomic exchange;
- a small fixed amount of local bookkeeping; and
- no retry loop.

Repeated publication may replace a value in the exchange slot, but it can
never replace the consumer-owned slot.

## 8. Consumer Algorithm

The consumer operation atomically exchanges its current front slot into the
shared position.

Pseudocode:

```rust
fn take_latest(&mut self) -> Option<LatestItem<T>> {
    let previous = self
        .buffer
        .state
        .swap(encode(self.front, Ready::No), Ordering::AcqRel);

    self.front = previous.slot();

    if !previous.ready() {
        return None;
    }

    let entry = read_owned_slot(self.front);
    let skipped = generation_gap(self.last_generation, entry.generation);
    self.last_generation = entry.generation;

    Some(LatestItem {
        value: entry.value,
        generation: entry.generation,
        skipped,
    })
}
```

This exchange is unconditional.

If no publication is ready, the consumer simply trades one privately owned
spare slot for another and returns `None`. This remains safe because it does
not read the unready slot.

If a publication is ready, the exchange gives the consumer exclusive
ownership before it reads the payload.

There is no compare-and-exchange retry loop. Both producer and consumer
perform one atomic exchange per operation.

## 9. Proposed API

The following is illustrative rather than final.

```rust
pub struct LatestBuf<T: Copy> {
    // private fields
}

pub struct Producer<'a, T: Copy> {
    // private fields
}

pub struct Consumer<'a, T: Copy> {
    // private fields
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PublishReport {
    pub generation: u32,
    pub replaced_unread: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct LatestItem<T> {
    pub value: T,
    pub generation: u32,
    pub skipped: u32,
}
```

Construction would follow the existing SPSC pattern:

```rust
static TELEMETRY: LatestBuf<ImuSample> = LatestBuf::new();

let producer = TELEMETRY.try_producer().expect("producer available");
let consumer = TELEMETRY.try_consumer().expect("consumer available");
```

Publication:

```rust
let report = producer.publish(sample);

if report.replaced_unread {
    transport_metrics.increment_replaced();
}
```

Consumption:

```rust
if let Some(item) = consumer.take_latest() {
    if item.skipped != 0 {
        dsp.note_transport_gap(item.skipped);
    }

    dsp.process(item.value);
}
```

## 10. End-to-End Telemetry Identity

Transport generation is useful, but acquisition identity should belong to the
producer.

A recommended telemetry envelope is:

```rust
pub struct Telemetry<T, Timestamp> {
    pub sequence: u32,
    pub acquired_at: Timestamp,
    pub value: T,
}
```

The IMU integration layer assigns the sequence and timestamp before
publication:

```rust
let event = Telemetry {
    sequence: acquisition_sequence,
    acquired_at: imu_timestamp,
    value: sample,
};

sink.try_push(event)?;
```

This produces two separate forms of evidence:

- **Acquisition sequence:** detects loss anywhere after acquisition.
- **Transport generation:** detects replacement inside this particular
  primitive.

The DSP should treat the acquisition timestamp as authoritative. ph-eventing
should not manufacture time or assume a timestamp unit.

Keeping sequence identity in the payload also preserves gap detection when
the transport is replaced with `EventBuf`, a test double, or a future
implementation.

## 11. Complete-Block Variant

A DSP commonly operates on windows rather than isolated samples. Publishing
only the latest individual sample may not provide a useful processing unit.

A companion primitive or adapter could publish complete blocks:

```rust
pub struct TelemetryBlock<T, Timestamp, const N: usize> {
    pub first_sequence: u32,
    pub last_sequence: u32,
    pub samples: [Telemetry<T, Timestamp>; N],
}
```

The integration layer fills a producer-owned block at the acquisition rate.
Once the block is complete, it publishes the block through `LatestBuf`.

```
fill private block
       │
       ▼
publish complete block ──▶ latest complete block ──▶ DSP
```

This provides:

- contiguous ordering inside each published block;
- no partially filled block visible to the DSP;
- explicit gaps between observed blocks;
- timestamps retained per sample;
- replacement of stale unconsumed blocks;
- fixed memory use; and
- bounded synchronization work at publication.

The memory cost is approximately three complete blocks plus metadata. That is
larger than a conventional ring, but it is fixed and directly measurable.

A block publication operation copies or transfers ownership of one complete
block. Its worst-case cost must be measured against the target's acquisition
budget.

## 12. Additive Trait Proposal

The existing traits remain unchanged:

```rust
pub trait Sink<T> {
    type Error;

    fn try_push(&mut self, value: T) -> Result<(), Self::Error>;
}

pub trait Source<T> {
    fn try_pop(&mut self) -> Option<T>;
}

pub trait Link<In, Out>: Sink<In> + Source<Out> {}
```

These remain the minimal interoperability layer.

They do not, however, express freshness semantics or loss observations.
Additive traits can provide those contracts without breaking existing users.

### 12.1 LatestSink

```rust
pub trait LatestSink<T> {
    fn publish_latest(&mut self, value: T) -> PublishReport;
}
```

Contract:

- publication never waits for a consumer;
- a complete new value becomes the latest publication;
- an unread previous publication may be displaced;
- displacement is reported;
- execution is bounded by the implementation's documented cost model.

`LatestBuf::Producer` could implement both `Sink<T>` and `LatestSink<T>`.

The `Sink<T>` implementation would use `Infallible` because publication
always succeeds:

```rust
impl<T: Copy> Sink<T> for latest_buf::Producer<'_, T> {
    type Error = Infallible;

    fn try_push(&mut self, value: T) -> Result<(), Infallible> {
        self.publish(value);
        Ok(())
    }
}
```

Generic producers can continue using `Sink<T>`. Producers that require
replacement telemetry can use `LatestSink<T>`.

### 12.2 LatestSource

```rust
pub trait LatestSource<T> {
    fn try_take_latest(&mut self) -> Option<LatestItem<T>>;
}
```

Contract:

- returns the latest complete publication claimed at the operation's
  linearization point;
- consumes that publication;
- may skip intermediate publications;
- reports the transport generation and skipped count;
- never returns an older generation after a newer one within one wrap span
  (the scope of contract clause C5; per D1's closure, no ordering claim is
  made beyond a full generation cycle — contract C5/X6).

This is intentionally different from a FIFO `Source`.

### 12.3 ObservedSource

A more general optional trait could allow multiple lossy primitives to expose
delivery metadata:

```rust
pub trait ObservedSource<T> {
    type Observation;

    fn try_pop_observed(
        &mut self,
    ) -> Option<(T, Self::Observation)>;
}
```

Possible observations include:

```rust
pub struct DeliveryObservation {
    pub sequence: u32,
    pub skipped_before: u32,
}
```

Potential implementors:

- `SeqRing::Consumer`, using its sequence and drop accounting;
- `LatestBuf::Consumer`, using publication generations;
- future freshness-first primitives; and
- test doubles that inject declared gaps.

This trait should only be introduced if at least two concrete implementations
demonstrate that the shared vocabulary is honest.

### 12.4 Payload metadata traits

Generic DSP utilities may benefit from optional payload traits:

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

These traits would not assign sequences or timestamps. They would only allow
generic consumers to inspect metadata owned by the producer.

They are optional conveniences rather than requirements of the transport.

## 13. Should LatestBuf::Consumer Implement Source\<T\>?

This requires care.

Implementing `Source<T>` would maximize substitution:

```rust
impl<T: Copy> Source<T> for Consumer<'_, T> {
    fn try_pop(&mut self) -> Option<T> {
        self.take_latest().map(|item| item.value)
    }
}
```

However, that implementation discards the skipped-publication count. It risks
repeating the reason `RingBuf::pop` + `Source` was rejected: loss could occur
without being represented by the trait result.

Three policies are possible:

1. **Do not implement `Source<T>`.** Require `LatestSource<T>` wherever
   replacement is possible. This is the most explicit option.
2. **Implement `Source<T>` only as a convenience.** Document that callers
   requiring loss evidence must use `LatestSource<T>` or carry acquisition
   sequences in `T`.
3. **Require sequenced payloads for the `Source` implementation.** This
   preserves end-to-end gap detection but introduces a stronger generic bound
   and couples the transport implementation to metadata vocabulary.

The first option is the safest initial position. The second may be acceptable
once real adopters demonstrate that producer-assigned sequence identity is
consistently present.

No implementation should silently imply FIFO semantics where the primitive
intentionally returns only the newest value.

**Decision (D2, closed 2026-08-11): the first option.** The maintainer's
articulation, on the record: `LatestSink`/`LatestSource` were designed to
solve the problem the existing traits could not — they *are* the contract
surface for the type. The second option remains the pre-registered additive
upgrade path under exactly the adopter-evidence condition stated above; the
third stays gated behind the second-implementation rule for payload-metadata
vocabulary. See the contract §9 and non-promise X7.

## 14. Safety Argument

The proposed soundness argument is based on ownership, not post-hoc
validation.

The principal invariant is:

> Each slot is owned by exactly one of the producer, consumer, or atomic
> exchange state.

Consequences:

- The producer writes only its private slot.
- The consumer reads only its private slot.
- Neither endpoint accesses the shared slot.
- An atomic exchange transfers ownership before access begins.
- Producer and consumer never concurrently access the same payload bytes.

Memory ordering requirements are expected to be:

- producer exchange uses `AcqRel`;
- consumer exchange uses `AcqRel`;
- writes to a published slot happen before the consumer acquires it;
- consumer reads finish before its previous front slot can be acquired and
  overwritten by the producer.

The precise ordering must be validated rather than accepted from this sketch.

Required evidence includes:

- Loom models for all producer/consumer exchange interleavings;
- mutation tests demonstrating that weakened ordering fails those models;
- Miri with the data-race detector enabled;
- threaded stress tests;
- generation and wraparound tests;
- handle uniqueness tests;
- compile checks across supported atomic targets; and
- an explicit unsafe-code audit.

Unlike `SeqRing`, this design should not require disabling Miri's race
detector for its concurrent tests.

## 15. Determinism and Performance Claims

The implementation should not initially claim "fast" or "real-time."

It should make narrower falsifiable claims:

- producer publication executes one payload write and one atomic exchange;
- consumer acquisition executes one atomic exchange and at most one payload
  copy;
- neither operation contains an unbounded retry loop;
- work is independent of accumulated producer/consumer lag;
- memory use is fixed at three payload slots plus metadata;
- no dynamic allocation occurs;
- publication never invokes the consumer;
- consumption never invokes the producer; and
- no incomplete value is returned.

These claims should be evaluated with the same discipline used for
ph-eventing 0.2.0:

- instruction counts on representative targets;
- code-size comparison across supported ISAs;
- native-atomic and portable-atomic configurations;
- Cortex-M0+/ESP32 critical-section cost;
- Miri and Loom;
- compile-time target matrix;
- generated assembly inspection where necessary; and
- comparison against `EventBuf` and `SeqRing`.

A new primitive should not be accepted merely because its algorithm looks
cleaner. It should be measurably competitive on the targets least able to
absorb additional flash, cycles, or interrupt latency.

## 16. Progress Guarantees

The intended progress properties are:

**Producer**

The producer is wait-free at the API level:

- one finite payload write;
- one atomic exchange;
- no loop;
- no dependency on consumer execution.

**Consumer**

The consumer is also bounded:

- one atomic exchange;
- either one payload read or immediate `None`;
- no loop;
- no dependency on accumulated backlog.

These are algorithmic bounds, not complete worst-case execution-time
guarantees. Actual instruction cost depends on:

- payload size and alignment;
- target atomic support;
- portable-atomic backend;
- whether atomics become bounded critical sections;
- compiler version and optimization;
- memory placement; and
- cache behavior on applicable targets.

Every released cost claim must identify its target and toolchain.

## 17. Failure and Overload Semantics

`LatestBuf` does not become full in the conventional queue sense. It always
has one producer-owned slot.

Overload appears as replacement:

```
producer publishes A
producer publishes B before consumer takes A
B replaces unread A
```

The producer receives:

```rust
PublishReport {
    generation: generation_of_b,
    replaced_unread: true,
}
```

The consumer later receives B with a skipped count.

No retry is required, and no publication blocks.

This is the deliberate semantic difference from `EventBuf`:

```
EventBuf:  preserve A, reject B
LatestBuf: replace A, publish B
```

Neither policy is universally correct. The caller chooses based on whether
completeness or freshness dominates.

## 18. Sequence Wrap

A `u32` generation eventually wraps.

The design should reserve zero for initialization and use explicit wrap-aware
arithmetic. Exact skipped counts are possible as long as fewer than the
complete sequence space of publications occurs between observations.

If a consumer can remain absent for an entire generation cycle, the gap is
inherently ambiguous. The API should either:

- document the maximum observable gap;
- return an "unknown or wrapped" gap state; or
- rely on a wider producer-assigned acquisition sequence in the payload.

**Decision (D1, closed 2026-08-11):** the first and third together. The
contract's C3/C5/O2 carry the beyond-span text and non-promise X6 states
the limitation for adopters — including why the second option was rejected
(hot-path cost, and "unknown" cannot recover the lost count). See the
contract §9.

Transport generation should not be mistaken for permanent global identity.

## 19. Non-Goals

This proposal does not attempt to:

- guarantee delivery of every sample;
- schedule the consumer;
- make SPI or I²C FIFO reads nonblocking;
- define a universal timestamp representation;
- provide multi-producer or multi-consumer operation;
- replace the IMU hardware FIFO;
- prove DSP algorithm correctness;
- hide sustained overload;
- make arbitrary `Sink` or `Source` implementations deterministic; or
- use hardware-in-the-loop evidence as a substitute for Rust memory-model
  soundness.

## 20. Hardware-in-the-Loop Composition Evidence (later, not now)

A future hardware-in-the-loop harness could evaluate the exact compiled
composition:

```
IMU → acquisition → LatestBuf → DSP
```

Potential observations include:

- configured and observed sample rate;
- hardware FIFO occupancy and overflow;
- acquisition-task or ISR duration;
- publication instruction count or timing distribution;
- replacement count;
- consumer gap count;
- oldest and newest sample age;
- DSP progress;
- recovery after deliberate consumer stalls; and
- behavior under declared CPU and interrupt load.

That evidence would establish behavior for the identified target, firmware
build, configuration, and workload.

It would not establish the Rust soundness of the ownership state machine.
Miri, Loom, code review, and formal reasoning remain responsible for that
claim. Composition evidence is deliberately sequenced *after* the soundness
and cost evidence — it is the last mile of the argument, never a prerequisite
for implementation and never a substitute for the memory-model proof.

## 21. Proposed Adoption Sequence

1. Write the semantic contract independently of an implementation.
2. Prototype the three-slot ownership state machine.
3. Model the state machine in Loom before optimizing it.
4. Run Miri with the race detector enabled.
5. Mutate ordering operations to ensure the models detect weakened
   synchronization.
6. Measure code size and instruction cost across the existing target matrix.
7. Compare individual-sample and complete-block forms.
8. Add `LatestSink` and `LatestSource` only with the primitive that
   demonstrates their need.
9. Add `ObservedSource` only after a second implementation confirms the
   vocabulary.
10. Integrate through an application or test harness before asking an IMU or
    DSP crate to depend on it.
11. Treat hardware-in-the-loop integration as later composition evidence, not
    as an implementation prerequisite.

## 22. Open Questions

- Is retaining only the latest individual sample sufficient for realistic
  DSP consumers?
- Should the first implementation publish fixed-size complete blocks instead?
- Should transport generation and replacement reports be mandatory or
  optional?
- Should `LatestBuf::Consumer` implement the existing `Source<T>` trait?
- Is three-payload memory cost acceptable for typical IMU sample and block
  sizes?
- How expensive is the atomic exchange on targets without native atomics?
- Does a bounded critical-section implementation outperform atomic ownership
  exchange on single-core targets?
- Can the same observational vocabulary honestly cover both `SeqRing` and
  `LatestBuf`?
- What should happen when the publication-generation counter wraps?
- Should freshness-first primitives remain a distinct module or become a
  documented behavioral profile?
- What evidence threshold would justify describing the primitive as a
  production replacement for `SeqRing`?

## 23. Conclusion

`SeqRing` captures the correct problem: a high-rate producer must remain
independent of a slower or unpredictably scheduled consumer, and loss of
stale telemetry can be preferable to unbounded latency.

Its current implementation does not provide a sound Rust solution to that
problem.

A three-slot ownership-transfer primitive offers a plausible alternative
because it changes the concurrency model. Instead of allowing a producer and
consumer to race and validating the result afterward, it transfers exclusive
ownership before either side accesses a slot.

The intended result is not a lossless queue. It is a freshness-first channel
with:

- bounded producer and consumer work;
- fixed memory;
- no allocation;
- no waiting for the other endpoint;
- latest-complete-value semantics;
- observable replacement and gaps;
- ordered delivered observations;
- independent endpoint integration; and
- a credible path to Miri- and Loom-supported soundness.

That would extend ph-eventing's core purpose without moving the maintenance
grenade back into either the producer or consumer crate.

---

## Appendix A — Review caveats captured for development (2026-08-11)

Raised in review when the proposal was captured; recorded here **for
discussion, not as accepted design**. Each is to be validated — proven or
disproven with the repo's usual evidence — when implementation starts, and
folded into the body of the proposal or discarded on that evidence.

### A.1 The empty-poll path costs an atomic RMW

As sketched in §8, `take_latest` performs its `swap` unconditionally, so a
consumer polling an idle channel executes a full atomic read-modify-write on
every call. On targets without native atomics (Cortex-M0+/M23, ESP32-S2/S3
under portable-atomic) each such swap is an interrupt-disable critical
section — a recurring interrupt-latency cost proportional to poll rate, paid
even when no data ever arrives.

**Candidate refinement:** an `Acquire` load of the ready bit before the swap.
Not ready → return `None` with no RMW; ready → proceed with the swap exactly
as sketched. The claim to validate: this is sound because a publication that
lands between the load and the return is simply observed on the next poll —
the linearization point moves earlier, nothing is torn, and no ownership is
transferred on the fast path (the consumer performs no slot access).

**Validation required before adoption:**

- Loom model covering the load/publish interleavings around the fast path;
- confirmation that the fast path cannot starve or livelock (it performs one
  load and returns — but the model, not the prose, is the evidence);
- `cycles.sh` measurement of both variants of the empty poll;
- codesize on the portable-atomic targets, where the win should be largest.

The unconditional-swap form stays the reference semantics; the fast path is
an optimisation and must be observationally equivalent.

### A.2 `generation_gap` off-by-one and wrap arithmetic

Two related hazards in the skipped-count arithmetic of §8/§18:

- **Off-by-one:** if the consumer last took generation 101 and now takes
  104, `skipped` must be **2** (generations 102 and 103), i.e. the distance
  minus one — not the raw distance. The §4.3 example already implies this;
  the implementation must pin it with a test before any wrap complexity is
  added.
- **Wrap over-counting:** with generation 0 reserved, raw `wrapping_sub`
  over-counts by one across the wrap because the counter skips 0 — the exact
  bug `SeqRing` had and fixed via `seq_distance` (see the 0.1.3 changelog
  and `seq_distance_skips_the_reserved_zero`). `generation_gap` needs the
  same reserved-zero-aware distance function, and the existing `SeqRing`
  wrap tests are the template for its test set
  (`push_wraps_seq_from_zero_to_one`, `lag_across_wrap_counts_drops_exactly`).

**Validation required before adoption:** unit tests at the wrap boundary for
both the producer's generation succession (`u32::MAX` → 1, never 0) and the
consumer's gap arithmetic across it, plus the 32-bit Miri pass — the
`SeqRing` `dropped_accum` overflow was caught by exactly that pass and not by
the 64-bit host run.

### A.3 Handle-carried producer state does not survive reacquisition

Found by Codex review on the capture PR (#24). The sketched `Producer`
(§6.3) carries `back` and `next_generation` in the handle, while the
sketched buffer (§6.2) stores only the exchange state and the taken flags.
The contract makes roles re-acquirable after a handle drop (H2) — but with
the sketch as written, a dropped producer takes its private slot index and
the generation cursor with it. A fresh handle cannot compute which slot is
safe to own (the buffer does not know the consumer's `front` either, so
"the third slot" is not derivable), and cannot resume the sequence; any
reset risks duplicated slot ownership or reused generations, breaking the
ownership invariant, G1, and C3's skipped accounting at once. The same
question applies to the consumer's `front` and `last_generation`.

Note the crate precedent: `SeqRing` and `EventBuf` handles are stateless —
every cursor lives in the buffer — which is exactly why their reacquisition
is trivially sound. Handle-carried state is this design's novelty, and this
is its cost.

**Candidate refinements, to be decided at implementation:**

- move the handle fields into the channel as role-owned (never shared)
  storage, matching the crate's stateless-handle precedent; or
- persist them into the channel in the handle's `Drop`; or
- withdraw reacquisition for stateful roles (narrow H2 explicitly).

**Decision (A.3, closed 2026-08-11): the first refinement.** Role state is
channel-resident in role-owned storage; handles are stateless — matching
the `SeqRing`/`EventBuf` precedent and the sole-role handle doctrine
accepted for the signal primitives (cycle decision H). Continuation holds
by construction: state never leaves the channel, so a drop-and-reacquire
has nothing to lose. The validation this appendix required exists in
full: drop-and-reacquire continuation tests, Loom models of both
cross-context role handoffs, race-detector-on Miri coverage, and
ordering-mutation runs detecting all four weakened role-handoff orderings.

The persist-on-drop alternative is **considered and not selected**, with
its partial evidence preserved in the evaluation record (§7): its
Drop-time state copy is a permanent failure surface — a missing or
reordered write-back silently restarts accounting, the exact H4 violation
this appendix exists to prevent — its L3 model does not isolate the
taken-flag handoff, and its register-residency claim is unmeasured.
Narrowing H2 was the registered fallback if both candidates failed the
evidence bar; the condition was not met.

Contract non-promise **X8** states the boundary implications for
integrators — reacquisition requires the previous handle's drop; handle
lifetime is an application property; there is deliberately no out-of-band
role reset — informing downstream design without prescribing or solving
it.

Contract clause **H4** (added on the same review round) pins the
requirement whichever way the design goes: reacquisition continues, never
restarts.

**Validation required before adoption:** drop-and-reacquire continuation
tests — the generation sequence resumes across the drop, skipped accounting
keeps satisfying contract C3 within its wrap-span scope, no slot is ever
owned by two roles — plus a Loom model (drop in
one context, reacquire in another) and race-detector-on Miri coverage of the
handoff. Whatever mechanism is chosen, the handoff is cross-context by
nature: state written at drop must be visible to the reacquiring context,
and sequential tests cannot see a missing publication ordering.
