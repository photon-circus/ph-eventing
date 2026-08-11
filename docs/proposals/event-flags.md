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

## 4. Exploratory decision package (2026-08-11)

The questions below remain decisions, not accidental consequences of the first
implementation that happens to compile. Three isolated prototypes were built
from the same `master` commit so the choices can be evaluated independently:

| Prototype | Conditions | Handles | Consumer | Deliberate omissions |
|-----------|------------|---------|----------|----------------------|
| A — raw shared word | `u32` | `EventFlags` itself; unlimited `&self` callers | any caller may `take_all(&self)` | claims, peek, traits, width genericity |
| B — enum-driven MPSC | user `EventFlag::bit() -> u8`; checked at runtime | unlimited `Copy + Sync` `Raiser`s | one claimed `Send + !Sync` `Taker` | width genericity, stream traits |
| C — SPSC typed mask | transparent `EventMask(u32)` | claimed `Producer`, `Send + !Sync` | claimed `Consumer`, `Send + !Sync` | peek, traits, width genericity |

All three use one `AtomicU32`, `fetch_or(Release)` to raise, and `swap(0,
Acquire)` to take. All are `no_std`, allocation-free, panic-free on their hot
paths, const/static constructible outside Loom, and add no normal dependency.
They are exploration vehicles, not three APIs proposed for shipment.

### 4.1 Candidate contract

These clauses are implementation-independent. Their IDs are candidates for
the permanent contract; once tests and user documentation cite them, they must
not be renumbered.

- **M1 — Initial state.** The pending condition set is initially empty.
- **M2 — Linearization.** Every raise and take has one instant between its
  invocation and return at which it takes effect.
- **R1 — Raise.** At `raise(m)`'s linearization point, pending becomes the set
  union of its prior value and `m`; raising the empty set changes nothing.
- **R2 — Coalescing.** Pending records only whether each condition occurred.
  Duplicate raises may coalesce; multiplicity is not observable.
- **T1 — Take.** A take returns exactly the pending set immediately before its
  linearization point and makes pending empty at that point.
- **T2 — Empty take.** A take linearized while pending is empty returns the
  empty set and changes no observable state.
- **C1 — Window exactness.** For each condition, a take contains it if and only
  if at least one matching raise linearized after the preceding take and before
  this take (or after initialization for the first take).
- **C2 — Concurrent boundary.** A raise racing a take is ordered by their
  linearization points. If the raise is first, the condition is in that take;
  otherwise it remains pending for a later take. Clearing never erases the
  later raise.
- **C3 — No fabrication.** A take returns no condition absent a matching raise
  in its take window.
- **O1 — Unordered.** There is no FIFO, timestamp, multiplicity, or
  cross-condition ordering guarantee.
- **S1 — Publication.** Memory actions sequenced before a raise happen-before
  memory actions sequenced after a take that observes that raise. This is why
  the prototypes use Release/Acquire rather than Relaxed operations.
- **B1 — Bounded hot paths.** Raise and take each perform one signal-word
  operation. Neither contains a retry loop, allocation, callback, panic path,
  or work proportional to history, occupancy, or the number of set bits.
- **B2 — Independence.** Producer work never waits for or invokes consumer
  work, and consumer work never waits for or invokes producer work.

Handle clauses depend on D1 and therefore cannot yet be frozen. The
conservative candidate is: at most one active handle per role; acquisition is
fallible and non-panicking; handles are `Send + !Sync`; dropping a handle makes
only that role re-acquirable; pending state survives drop/reacquisition; the
container is const/static constructible.

### 4.2 D1 — handle and concurrency model (deferred)

`raise(&self)` does **not** by itself imply a shareable handle. Existing crate
handles already use `&self` hot-path methods while `PhantomData<Cell<()>>`
makes the handle `!Sync`. The auto traits, not receiver mutability, define the
concurrency contract.

| Option | Predictability | Efficiency | Shared answer with `CountedSignal` | Cost / risk |
|--------|----------------|------------|------------------------------------|-------------|
| A: direct shared container | Atomic operations are sound with many raisers and takers, but multiple takers distribute observations nondeterministically | smallest state and API; no claim acquisition | poor — it would commit the vocabulary to contention before bounded exact saturation is solved | no exclusive observer; expands Loom and publication obligations |
| B: many raisers, one taker | one observation owner; natural fit for atomic OR | raiser is zero-claim and copyable | uncertain — exact saturating multi-raiser increment is the unresolved core of `CountedSignal` | asymmetric first non-SPSC contract |
| C: one handle per role | matches existing ownership and makes the smallest shared promise | two claim bits and one extra handle indirection; hot word operation remains one RMW | strongest — does not force `CountedSignal` to promise a bounded contended increment | leaves hardware-supported EventFlags concurrency unused |

The current recommendation is **C**, deliberately conservative. A future MPSC
type can be added after independent evidence; an MPSC promise cannot be taken
back compatibly. This is a recommendation for the shared planning decision,
not a decision silently made on this branch.

### 4.3 D2 — condition representation (deferred)

| Option | Strength | Runtime / flash expectation | API and regression burden |
|--------|----------|-----------------------------|---------------------------|
| raw `u32` | exact mechanism, but unrelated domains and arbitrary bits mix freely | baseline minimum | smallest surface, weakest semantic boundary |
| transparent `EventMask(u32)` | makes set operations explicit; checked constructors can prevent shift panics; named application constants remain possible | should erase to the raw mask; preliminary results are within 2 bytes | small hand-rolled surface, no dependency |
| `EventFlag` enum trait | strong call-site vocabulary and namespace | checked shift/validation is visible in the hot path | user owns uniqueness/range correctness; largest docs and semver surface |
| const-generic/enum macro | can recover named ergonomics at compile time | must be proved per expansion/monomorphization | macro diagnostics, generated docs, and a much larger evidence matrix |

The current recommendation is the transparent mask, with raw conversion kept
explicit and panic-free. Enum ergonomics can be layered on later by a macro if
real callers demonstrate that it earns the surface. The prototype exposed an
important limit: `EventFlag::bit()` can validate range, but cannot prove that
two enum variants do not alias the same bit.

### 4.4 D3 — width (deferred, recommendation: `u32` only)

`u32` matches the crate's existing atomic shim and sequence width and provides
one contract on every shipped 32-bit target. A generic atomic-word trait would
expose target-dependent atomic availability, fallback behaviour, code size,
and monomorphization as public API. `u64` is particularly misleading on these
targets: “one word” need not mean one native or one critical-section operation.

The admission candidate should therefore provide exactly 32 conditions.
Additional named widths remain possible later, but only with a concrete user
and a separate Loom/Miri/codesize/cycles matrix.

### 4.5 D4 — non-clearing observation (deferred, recommendation: omit)

An atomic load is race-free and linearizable as a snapshot, but the result is
intrinsically advisory: another take may clear it immediately and a concurrent
raise may arrive immediately after it. A `peek` name invites check-then-act
reasoning that the primitive cannot uphold. Omit it from the initial surface.
If demonstrated demand later earns it, call it `snapshot_pending` or
`load_pending` and state explicitly that it predicts no later take.

### 4.6 D5 — traits (deferred, recommendation: keep disjoint)

Do not implement stream `Sink`/`Source`/`Link`. Coalesced state is not an item
stream, and `forward()` could destructively take a mask and then lose it when a
destination rejects the value. It also cannot report per-condition
coalescing.

Do not freeze `SignalSink`/`SignalSource` yet. `CountedSignal` has not shown
that `raise(S) -> take_pending() -> S` is honest vocabulary: its producer
operation is an increment and its take likely returns a count-plus-saturation
report. If both primitives survive, associated `Signal` and `Pending` types on
traits implemented by handles are a better direction than one shared generic
`S`, but the second implementation must prove it first.

## 5. Preliminary implementation evidence

### 5.1 Behaviour and build checks

The isolated prototypes were checked without modifying the candidate branch:

| Prototype | Native tests | Additional checks |
|-----------|--------------|-------------------|
| A — raw shared word | 74 unit + 11 doctests + 3 compile-fail | fmt, clippy, threaded multi-raiser and raise-vs-take stress |
| B — enum-driven MPSC | 73 unit + 15 doctests | fmt, clippy, `thumbv7em`, zero normal dependencies, focused Loom raise-vs-take model |
| C — SPSC typed mask | 75 unit + 11 doctests + 3 compile-fail | fmt, clippy, `thumbv7em`, focused Loom raise-vs-take model |

These checks establish that each API shape is viable. The focused model checks
only C2 for the prototype; it is not the complete evidence map below.

### 5.2 Exploratory hot-path code size

Minimal `opt-level = "z"`, LTO static-library probes were built with the pinned
rustc for three representative installed targets. Each row is the isolated
`.text.<function>` size in bytes; handle acquisition and application call-site
code are excluded.

| Target | raw raise / take | enum MPSC raise / take | SPSC mask raise / take |
|--------|-----------------:|-----------------------:|-----------------------:|
| `thumbv6m-none-eabi` (portable-atomic single-core) | 22 / 24 | 38 / 24 | 24 / 24 |
| `thumbv7m-none-eabi` | 24 / 26 | 36 / 26 | 26 / 26 |
| `riscv32imac-unknown-none-elf` | 6 / 6 | 20 / 8 | 8 / 8 |

This is decision evidence, not an admission measurement: it covers three of
the eleven target rows, uses standalone probes rather than the committed gate,
and does not measure interrupt-disabled duration. It nevertheless answers two
questions usefully:

1. The transparent mask erases as intended; the SPSC handle costs only one
   extra pointer load in these non-inlined boundary probes (2 bytes per hot
   function).
2. The enum trait's range check and mapping remain visible: raise is 12–16
   bytes larger than the SPSC mask and 14 bytes larger than raw on the measured
   targets. Ergonomics is therefore not free on the ISR path.

No cycle count is recorded here. Local QEMU was unavailable, and the existing
reference probe measures Cortex-M3 rather than the portable-atomic critical
section that carries this primitive's admission case. Treating a host or one
native-atomic number as closure would violate the reason this issue exists.

## 6. Evidence map required before promotion

- **Contract/unit:** M1–T2, empty and all-bit masks, bit 31, duplicate and
  multi-bit raises, no fabrication, take clears, claim failure, static handles,
  and drop/reacquisition preserving pending state.
- **Loom C1–C3:** raise racing take is observed now or later but never neither;
  distinct raises partition exactly across takes plus final pending; duplicates
  coalesce; no condition is fabricated or returned twice without re-raise.
- **Loom S1:** a separate payload-publication litmus. Mutating either Release or
  Acquire to Relaxed must make this model fail; the bit-conservation model alone
  cannot distinguish the orderings.
- **Miri:** detector-on unit/stress and handle transfer/reacquisition, including
  a 32-bit std target. There is no unsafe slot access here, so EventFlags must
  not inherit the `SeqRing` detector exception.
- **Code size:** committed rows for acquisition, raise, and empty/non-empty take
  across all eight gated upstream targets plus the three opt-in Xtensa rows;
  compare the chosen wrapper with a raw atomic baseline and confirm no panic
  strings or startup data.
- **Cycles/latency:** reference-image Cortex-M3 counts for raise with a clear and
  already-set bit and take when empty/non-empty; plus target-credible
  instruction and maximum interrupt-disabled-window measurements for
  thumbv6m and ESP32-S2/S3 portable-atomic paths. Verify exactly one RMW and no
  hidden compare-exchange loop.
- **Integration:** full `verify.sh` with zero skips; README, crate docs,
  CHANGELOG, AGENTS memory-ordering notes and test counts; `Cargo.toml` include
  allowlist; normal `cargo tree` still the crate alone.

## 7. Promotion bar to PROPOSED

1. Record D1 and D2 as shared planning decisions; D1 is one answer for this
   primitive and `CountedSignal`.
2. Confirm or change the recommendations for D3–D5 explicitly.
3. Freeze the accepted subset of the §4.1 clauses and handle clauses.
4. Implement the §6 evidence map. The portable-atomic ISR measurement is the
   admission case; Loom/Miri close the semantic and publication clauses.

Until steps 1–3 are explicit, status remains **EXPLORATORY** and the three
prototypes remain options rather than a de facto public API.
