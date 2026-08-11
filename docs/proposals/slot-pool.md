# SlotPool: bounded zero-copy ownership transfer (exploratory design document)

- **Status:** EXPLORATORY — evaluation implementation behind the
  `experimental-slot-pool` feature; not yet PROPOSED.
- **Origin:** substantive design text moved from the
  [bounded-handoff taxonomy](exploratory-primitives.md) §3 (received
  2026-08-11); triaged Tier 2 in [`../0.3.0-candidates.md`](../0.3.0-candidates.md) §4.
- **Taxonomy row:** retention *every admitted owned buffer* · overload
  *reservation fails* · representative use *large zero-copy payloads*.
- **Related:** [`block-buf.md`](block-buf.md) (named candidate foundation
  for it), and the taxonomy's out-of-scope list (DMA cache maintenance
  stays outside the crate).

## 1. Design sketch (from the taxonomy)

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

## 2. Trait sketches (moved from the taxonomy's trait directions)

### ReservableSink

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

Per the crate's trait rule, these ship only with the primitive that
demonstrates their need — this one.

## 3. Triage notes (0.3.0 cycle)

- **Non-`Copy` RAII payloads are a departure, not an extension.** Every
  current primitive is `T: Copy` with no drop obligations; grants and
  claims introduce RAII drop behaviour and borrowed access to shared
  storage. The safety story is new ground for this crate and concentrates
  the design effort.
- **GATs** (`type Grant<'a>`) are fine at MSRV 1.92; the lifecycle
  (commit-or-cancel, bounded reservation, no unbounded scan) is the hard
  part, not the language feature.
- **DMA cache maintenance stays outside the crate** per the taxonomy's
  out-of-scope list — the pool transfers ownership; target-specific
  visibility effects belong to the integration layer.

## 4. Evaluation slice and decisions

The first implementation is intentionally narrower than a publishable pool.
Its job is to test the ownership shape and cost model without prematurely
claiming that generic uninitialized construction is solved.

The evaluation slice chooses:

- SPSC, matching the crate's existing endpoint model;
- FIFO delivery across slots;
- one outstanding grant per producer handle and one outstanding claim per
  consumer handle, enforced by borrowing the handle mutably;
- pre-initialized reusable `T` slots supplied at construction;
- `T` need not be `Copy` or `Default`;
- reject-new backpressure: `try_reserve` returns `None` when every slot is
  published or consumer-owned;
- two monotonic wrapping cursors rather than one atomic state per slot; and
- no `ReservableSink` / `ClaimSource` traits until the concrete API earns
  them.

The implementation lives in `src/slot_pool.rs` and is available downstream
only with `experimental-slot-pool`. Default builds do not expose it from the
crate root. This keeps the evaluation runnable without presenting it as an
accepted fourth primitive.

### 4.1 Why pre-initialized reusable slots first

A safe generic API cannot infer that arbitrary writes through
`&mut MaybeUninit<T>` initialized a complete `T`. Making `commit()` the witness
would therefore be unsafe unless the API uses type-state such as
`Grant::write(T) -> InitializedGrant`, which constructs the value elsewhere and
does not evaluate in-place fill well.

The first slice instead accepts `[T; N]` and grants `&mut T`. This is directly
useful for reusable DMA blocks, packet buffers, and arrays:

```rust
static POOL: SlotPool<[u8; 1024], 3> =
    SlotPool::new([[0; 1024]; 3]);
```

Every slot is valid for the pool's entire lifetime. A cancelled grant may
leave changed bytes in a free slot, but those bytes are not published. The
next grant receives the same reusable storage and is responsible for writing
the portion its own metadata says is meaningful. Cancellation is not a
transaction and does not restore the previous contents.

This resolves the evaluation slice's initialization safety claim by removing
uninitialized storage from it. It does **not** resolve the promotion question
of whether the eventual primitive should support one-time initialization or
generic in-place construction.

## 5. State machine and representation

The two cursors define a half-open FIFO range:

```text
[tail, head) = published or consumer-owned slots
head - tail  = occupancy, bounded by N
```

The slot index for a cursor position is `position % N`. Cursor arithmetic is
wrapping `u32`; construction rejects `N == 0` and `N > u32::MAX`.

The logical per-slot state remains:

```text
Free --try_reserve--> ProducerOwned --commit--> Published
  ^                       |                         |
  |                       +--drop/cancel-----------+
  |                                                 |
  +---------------- claim drop <-- ConsumerOwned <--try_claim
```

The representation encodes those states without per-slot atomics:

| Logical state | Cursor/guard representation |
|---|---|
| `Free` | Outside `[tail, head)` and not held by a grant |
| `ProducerOwned` | Position `head`, cursor not advanced, live `Grant` |
| `Published` | Inside `[tail, head)`, not the live claim at `tail` |
| `ConsumerOwned` | Position `tail`, cursor not advanced, live `Claim` |

This works only because the evaluation slice is SPSC with one grant and claim
in flight. Supporting multiple producers, multiple consumers, or reservations
that outlive their endpoint would require a different representation and
reopen per-slot atomic states and generation tags.

### 5.1 Stale handles and generations

Safe code cannot retain a slot handle across reuse. `Grant` borrows the sole
producer mutably, `Claim` borrows the sole consumer mutably, and neither guard
is `Send` or `Sync`. A claim cannot survive the consumer handle, and the
producer cannot reserve again while its grant exists. Generation tags add no
safety to this restricted API and are omitted from the evaluation slice.

If a future API exposes detached slot tokens, multiple in-flight reservations,
or FFI handles, generation tags become mandatory again. That is an API
boundary decision, not an optimization to defer silently.

## 6. Algorithms and ordering

### 6.1 Reserve and commit

`try_reserve` performs:

1. relaxed load of producer-owned `head`;
2. acquire load of consumer-owned `tail`;
3. reject if `head.wrapping_sub(tail) >= N`; otherwise return a grant for
   `head % N`.

The Acquire load observes the consumer claim's Release on `tail`, so producer
mutation cannot begin before the previous consumer read of that slot completes.

`Grant::commit` performs one Release store of `head + 1`. All slot mutations
are sequenced before publication. Dropping or explicitly cancelling an
uncommitted grant performs no atomic operation and cannot publish the slot.

### 6.2 Claim and release

`try_claim` performs:

1. relaxed load of consumer-owned `tail`;
2. acquire load of producer-owned `head`;
3. return `None` if equal; otherwise return a claim for `tail % N`.

The Acquire load observes the grant's Release publication, so the consumer
cannot read the slot before all producer mutations complete.

`Claim::drop` and explicit `release` perform one Release store of `tail + 1`.
The producer cannot reuse that slot before an Acquire load observes the
release.

There are no compare-exchange retry loops and no slot scans. Reserve, commit,
claim, cancellation, and release are O(1) in capacity and occupancy.

## 7. Exploratory API

The implemented shape is:

```rust
pub struct SlotPool<T, const N: usize> { /* ... */ }
pub struct Producer<'pool, T, const N: usize> { /* ... */ }
pub struct Consumer<'pool, T, const N: usize> { /* ... */ }
pub struct Grant<'grant, 'pool, T, const N: usize> { /* ... */ }
pub struct Claim<'claim, 'pool, T, const N: usize> { /* ... */ }

impl<T, const N: usize> SlotPool<T, N> {
    pub const fn new(slots: [T; N]) -> Self;
    pub fn try_producer(&self) -> Option<Producer<'_, T, N>>;
    pub fn try_consumer(&self) -> Option<Consumer<'_, T, N>>;
}

impl<T, const N: usize> Producer<'_, T, N> {
    pub fn try_reserve(&mut self) -> Option<Grant<'_, '_, T, N>>;
}

impl<T, const N: usize> Grant<'_, '_, T, N> {
    pub fn commit(self);
    pub fn cancel(self);
}

impl<T, const N: usize> Consumer<'_, T, N> {
    pub fn try_claim(&mut self) -> Option<Claim<'_, '_, T, N>>;
}

impl<T, const N: usize> Claim<'_, '_, T, N> {
    pub fn release(self);
}
```

`Grant` implements `Deref<Target = T>` and `DerefMut`; `Claim` implements
`Deref<Target = T>`. The endpoints are `Send + !Sync` when `T: Send`, matching
the existing handle model. Guards are deliberately `!Send + !Sync` in this
slice, keeping each access in its endpoint's context.

## 8. Failure and teardown paths

| Event | Result | Bound | User `T` drop? |
|---|---|---:|---:|
| reserve while full | `None`, no state change | O(1) | no |
| drop/cancel grant | slot remains free and unpublished | O(1), no atomic | no |
| claim while empty | `None`, no state change | O(1) | no |
| drop/release claim | slot becomes free | O(1), one Release store | no |
| drop endpoint | endpoint can be reacquired | O(1), one Release store | no |
| drop pool | all `N` reusable values are dropped once | O(N) teardown | yes |

A guard borrows its endpoint, so Rust drops the guard before that endpoint can
be dropped or reused. The pool is borrowed by both endpoints and therefore
cannot be dropped while either endpoint or guard exists. Hot-path RAII does not
invoke `T::drop` and has no user-code panic path.

## 9. Preliminary evaluation contract

These clauses describe what the prototype is intended to test. They are not a
release contract until the evidence bar is met.

**Ownership**

- **SP-O1:** at most one producer and one consumer endpoint are active.
- **SP-O2:** at most one grant and one claim are active per endpoint.
- **SP-O3:** producer and consumer never access the same slot concurrently.
- **SP-O4:** a slot remains initialized for the pool's entire lifetime.

**Publication and delivery**

- **SP-P1:** only `Grant::commit` makes a producer-owned slot claimable.
- **SP-P2:** a committed slot is observed only after all grant mutations.
- **SP-C1:** claims are returned in commit order.
- **SP-C2:** a claimed slot cannot be reused until the claim is released.
- **SP-C3:** cancellation publishes nothing and does not roll contents back.

**Bounds and overload**

- **SP-B1:** reserve, commit, claim, cancel, and release contain no unbounded
  loop or capacity-dependent scan.
- **SP-B2:** occupancy never exceeds `N`.
- **SP-B3:** full-pool overload rejects the new reservation explicitly.
- **SP-B4:** no hot-path operation allocates, panics, or invokes user code.

## 10. What the prototype can evaluate

The slice is sufficient to answer:

- whether reusable pre-initialized blocks cover the motivating DMA/DSP cases;
- whether FIFO reject-new semantics compose naturally with `BlockBuf`;
- whether mutable grant and shared claim ergonomics are acceptable;
- whether cancellation-with-residue is workable or needs an explicit reset
  hook outside the hot path;
- RAM cost (`N * size_of::<T>()` plus four atomics) and target-specific code
  size/cycle cost; and
- whether the no-per-slot-atomic SPSC representation survives Miri and Loom.

It is not evidence for detached handles, multiple in-flight grants, MPSC/MPMC,
uninitialized generic payload construction, DMA cache visibility, or any
overwrite/latest policy.

## 11. Evidence status

Implemented evidence in the evaluation branch:

- unit tests for FIFO delivery, cancellation, held-claim backpressure,
  endpoint reacquisition, and non-`Copy` drop obligations;
- a two-thread stress test over 5,000 transfers; and
- normal tests, feature-enabled check, feature-enabled Clippy, and rustdoc;
- feature-enabled checks for `thumbv6m-none-eabi`,
  `thumbv7em-none-eabi`, and `riscv32imac-unknown-none-elf`; and
- the repository Miri matrix with the race detector on for SlotPool: host,
  16 scheduler seeds, 32-bit i686/ARM, and big-endian s390x, using
  `rustc 1.99.0-nightly (8ab9fdff5 2026-07-30)`.

Still required before promotion:

- a Loom model of commit/claim and grant-drop interleavings using tracked slot
  access;
- code-size rows on every gated target and instruction counts for empty,
  available, full, committed, and release paths;
- an actual `BlockBuf` integration probe; and
- a decision and Miri proof for any API that introduces `MaybeUninit<T>`.

## 12. Remaining design questions

1. Is pre-initialized reusable storage the intended primitive, or must the
   public design support one-time initialization of arbitrary `T`?
2. If partial logical initialization matters (for example a valid byte prefix),
   does metadata live in `T`, beside each slot, or in a specialized block type?
3. Does cancellation residue require a caller-supplied reset operation, and if
   so can it remain outside `Drop` so grant cancellation stays panic-free?
4. Does SlotPool earn admission standalone, or only through a concrete
   [`BlockBuf`](block-buf.md) evaluation?
5. Are one grant and one claim in flight sufficient for target integrations?
   A negative answer invalidates the cursor-only state representation.
6. Should the final traits expose `Deref` guards, explicit `get` methods, or a
   narrower block-specific surface?

## 13. Promotion bar to PROPOSED

1. Run the prototype through at least one representative complete-block
   integration and answer the remaining questions above.
2. Either keep the pre-initialized invariant explicitly or design the generic
   initialization witness; do not imply `commit` proves initialization without
   an API that makes that true.
3. Turn the preliminary SP clauses into the reviewed contract and map each
   clause to tests, Loom, Miri, documentation, or target measurements.
4. Add Loom coverage of grant-drop cancellation and Miri coverage proving that
   every claim exposes only a valid initialized `T`.
5. Run the standard code-size, cycle, embedded-target, and dependency evidence
   bars. A `SKIP` is not a pass.
