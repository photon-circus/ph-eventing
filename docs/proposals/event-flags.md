# EventFlags: coalesced condition notification (exploratory design document)

- **Status:** EXPLORATORY — design exploration vehicle, not yet PROPOSED.
- **Origin:** substantive design text moved from the
  [bounded-handoff taxonomy](exploratory-primitives.md) §4 (received
  2026-08-11); triaged Tier 1 in [`../0.3.0-candidates.md`](../0.3.0-candidates.md) §4.
- **Taxonomy row:** retention *one bit per condition* · overload *repeated
  signals coalesce* · representative use *ISR-to-task notification*.
- **Related:** [`counted-signal.md`](counted-signal.md) — same niche
  (bounded ISR notification), different contract (multiplicity); the signal
  trait vocabulary below is shared between the two.

## 1. Design sketch (from the taxonomy)

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

## 2. Trait sketches (moved from the taxonomy's trait directions)

### SignalSink and SignalSource

```rust
pub trait SignalSink<S> {
    fn raise(&self, signal: S);
}

pub trait SignalSource<S> {
    fn take_pending(&self) -> S;
}
```

These describe coalesced conditions rather than event streams.

Per the crate's trait rule, they ship only with a primitive that
demonstrates their need — this one and [`CountedSignal`](counted-signal.md)
are the candidates, and the vocabulary should be proven against both before
it is frozen.

## 3. Triage notes (0.3.0 cycle)

- **The ISR-side cost must be measured, not assumed.** `fetch_or` and
  `swap` are both interrupt-disable critical sections under portable-atomic
  on thumbv6m / ESP32-S2 — cheap, but ISR latency is exactly what this
  primitive exists to protect, so the cost lands in `cycles.sh` and
  codesize rows before any claim is made.
- **`&self` operations are new ground for the crate.** The sketched
  `raise(&self)` would be the first `&self`-operation primitive here; every
  existing handle mutates through `&mut self`. This is a handle-model
  question to settle at design time (see open questions), not a detail.
- **The lost-event clause is Loom material.** "Clearing and raising must
  not lose a concurrent event" is satisfied by `fetch_or`/`swap(0)` on one
  word — but the model, not the prose, is the evidence, and the model also
  pins it against future "optimisation."

## 4. Open questions

- Handle model: does `&self` on `raise` imply multiple raisers are sound
  (an `AtomicU32` `fetch_or` is), and if so, is this deliberately the
  crate's first non-SPSC primitive — or is the SPSC handle discipline kept
  anyway for uniformity and future-proofing?
- Typed conditions: raw `u32` mask, a `bitflags`-style wrapper (no new
  dependency — hand-rolled), or a const-generic/enum-driven API? What does
  each cost in flash and in `missing_docs`-grade API surface?
- Width: is `u32` (32 conditions) the only offering, or does the type
  parameterise over the atomic width the target supports?
- Does `take_all` need a peek variant (read without clearing), and does
  that stay race-free in the same trivial way?
- Is a `Sink`/`Source` bridge meaningful at all here, or is the signal
  vocabulary deliberately disjoint from the stream vocabulary (the taxonomy
  implies disjoint — confirm and document)?

## 5. Promotion bar to PROPOSED

1. Settle the handle-model and typed-conditions questions.
2. Write the contract (short — the four clauses in §1 are most of it) with
   citable clause IDs.
3. Standard evidence bar, with emphasis inverted from the ring primitives:
   the codesize and cycles evidence (especially the ISR-side `raise` on
   portable-atomic targets) *is* the case for admission; Loom/Miri close
   the lost-event and coalescing clauses.
