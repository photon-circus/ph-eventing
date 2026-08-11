# SlotPool: bounded zero-copy ownership transfer (exploratory design document)

- **Status:** **DEFERRED** (maintainer decision S, 2026-08-11) —
  evaluation complete, evidence banked, implementation behind
  `experimental-slot-pool` on this branch; not admitted this cycle. With
  cycle decision **P** closed as Copy composition there is no in-release
  consumer, and standalone admission would commit the crate's first
  non-`Copy` RAII surface with no adopter to shape the grant/claim API.
  **Reopening trigger** (any one suffices): a named adopter whose
  measured budget the Copy path breaches at a supported shape; a
  direct-to-granted-slot (DMA-class) requirement composition cannot
  satisfy; a standalone zero-copy adopter. On trigger the lane resumes
  from this banked evidence with BlockBuf-over-SlotPool as the
  demonstration path. Full rationale: the planning record's P/S closure.
- **Origin:** substantive design text moved from the
  [bounded-handoff taxonomy](exploratory-primitives.md) §3 (received
  2026-08-11); triaged Tier 2 in [`../0.3.0-candidates.md`](../0.3.0-candidates.md) §4.
- **Taxonomy row:** retention *every admitted owned buffer* · overload
  *reservation fails* · representative use *large zero-copy payloads*.
- **Related:** [`block-buf.md`](block-buf.md), the
  [target-cost record](slot-pool-measurements.md), and the taxonomy's
  out-of-scope list (DMA cache maintenance stays outside the crate).

## 1. Design sketch

For larger payloads, copying through a queue can dominate transfer cost. A
statically allocated slot pool transfers access to the same storage instead:

```text
free slot → producer reservation → committed slot
                                  ↓
                             consumer claim
                                  ↓
                             released slot
```

Representative uses include DMA buffers, large FIFO bursts, encoded packets,
audio blocks, protocol frames, and diagnostic snapshots.

The important safety property is that memory moves through exclusive logical
states:

```text
Free → ProducerOwned → Published → ConsumerOwned → Free
```

No slot can be written while the consumer owns it. No slot can be read before
the producer initializes and commits it. Reservation failure is explicit
backpressure; no operation scans the pool or retries a compare-exchange.

DMA cache maintenance remains outside ph-eventing because it is a
target-specific visibility effect. The pool transfers Rust ownership only.

## 2. Trait sketches

### ReservableSink

```rust
pub trait ReservableSink<T> {
    type Reservation<'a>
    where
        Self: 'a;

    fn try_reserve(&mut self) -> Option<Self::Reservation<'_>>;
}
```

### ClaimSource

```rust
pub trait ClaimSource<T> {
    type Claim<'a>
    where
        Self: 'a;

    fn try_claim(&mut self) -> Option<Self::Claim<'_>>;
}
```

The GATs are available at MSRV 1.92. The traits remain sketches: per the
crate's trait rule, a shared vocabulary ships only after the concrete primitive
is admitted and a second implementation demonstrates the abstraction.

## 3. Evaluation choices

The implementation chooses:

- SPSC, matching the crate's existing endpoint model;
- FIFO delivery across slots;
- one outstanding reservation per producer and one outstanding claim per
  consumer, enforced by mutable endpoint borrows;
- initialized construction with `SlotPool::new([T; N])` and generic lazy
  construction with `SlotPool::new_uninit()`;
- `T` need not be `Copy` or `Default`;
- reject-new backpressure when all slots are published or consumer-owned;
- two wrapping cursors, not one atomic state per slot;
- one producer-owned initialized-prefix count, not an `N`-bit bitmap; and
- no detached tokens, generation tags, MPSC/MPMC, overwrite policy, or DMA
  cache operations.

The implementation lives in `src/slot_pool.rs`. Default downstream builds do
not expose it; crate tests compile it so the evidence cannot silently rot.

## 4. Initialization witness

A safe API cannot infer that arbitrary writes through `&mut MaybeUninit<T>`
constructed a complete `T`. Treating `commit()` as that witness would let a
claim create `&T` over uninitialized bytes.

The implementation instead makes initialization a type-state transition:

```text
Reservation::Vacant(VacantGrant)
              |
              +-- VacantGrant::write(T) --> Grant -- commit --> Published

Reservation::Initialized(Grant) ------------ commit --> Published
```

`VacantGrant` has no `commit` method. Its safe `write(T)` constructs the value
in the reserved `MaybeUninit<T>` slot and returns the only type that can
publish. A compile-fail doctest pins that API boundary. `Claim` can therefore
rely on a stronger invariant than a runtime flag check: every cursor position
below `head` was advanced by an initialized `Grant`.

Once initialized, a slot remains a valid `T` until pool teardown. Cancelling a
newly written grant leaves initialized residue for reuse and invokes no user
drop code. Later grants provide `&mut T`, which is the hot path for reusable
DMA and block buffers.

### 4.1 Why one prefix count is sufficient

Initialization and publication both proceed at `head`; neither can skip a
slot. Until all `N` slots are initialized, initialized storage is therefore
exactly a prefix of the backing array. Cancelling after `write` can put the
prefix one slot ahead of `head`, but it cannot create a hole.

A producer-owned `u32` count records that prefix. It is also the exact number
of `T` values pool teardown must drop. After it reaches `N`, every slot remains
initialized and cursor wrap cannot make the tracking ambiguous. This adds one
pool-wide word rather than per-slot state.

`write(T)` constructs `T` as a whole; it does not claim that arbitrary
field-by-field in-place initialization is safely observable. Large reusable
buffers use the initialized constructor and mutate the slot directly, so the
first-construction move is not on their steady-state path.

## 5. State machine and representation

The two cursors define a half-open FIFO range:

```text
[tail, head) = published or consumer-owned slots
head - tail  = occupancy, bounded by N
```

The slot index for a cursor position is `position % N`. Cursor arithmetic is
wrapping `u32`; construction rejects `N == 0` and `N > u32::MAX`.

Initialization adds two pre-publication states to the original cycle:

```text
UninitializedFree --reserve--> VacantProducerOwned --write--> ProducerOwned
        ^                              |                         |
        +-----------cancel-------------+                         |
                                                                  |
InitializedFree --reserve-----------------------------------------+
        ^                                                         |
        |                   +---------------cancel----------------+
        |                   |
        +---- claim drop <-- ConsumerOwned <-- Published <--commit
```

| Logical state | Cursor/guard representation |
|---|---|
| `UninitializedFree` | Current prefix boundary, outside `[tail, head)` |
| `VacantProducerOwned` | Position `head`, live `VacantGrant` |
| `InitializedFree` | Outside `[tail, head)`, within initialized prefix |
| `ProducerOwned` | Position `head`, live `Grant`, cursor not advanced |
| `Published` | Inside `[tail, head)`, not the live claim at `tail` |
| `ConsumerOwned` | Position `tail`, live `Claim`, cursor not advanced |

This representation depends on SPSC and one reservation/claim in flight.
Detached tokens, multiple reservations, FFI handles, or multiple endpoints
would require per-slot states and generation tags; they are not silent future
optimizations of this design.

Safe code cannot retain a slot handle across reuse. Reservations borrow the
sole producer mutably, claims borrow the sole consumer mutably, and guards are
`!Send + !Sync`. Generation tags add no safety within this restricted API.

## 6. Algorithms and ordering

### 6.1 Reserve, initialize, and commit

`try_reserve` performs:

1. relaxed load of producer-owned `head`;
2. acquire load of consumer-owned `tail`;
3. reject when `head.wrapping_sub(tail) >= N`;
4. otherwise compare `head % N` with the producer-owned initialized prefix and
   return `Reservation::Initialized` or `Reservation::Vacant`.

The Acquire load observes claim release before producer reuse. Prefix access is
non-atomic because the single producer is its only live accessor.

`VacantGrant::write` writes one `T`, then extends the prefix. It performs no
atomic operation and cannot publish. `Grant::commit` performs one Release store
of `head + 1`, sequencing all construction/mutation before a claim.

Dropping or cancelling any uncommitted reservation performs no atomic
operation. A vacant cancellation leaves the slot uninitialized; cancellation
after `write` leaves an initialized reusable value.

### 6.2 Claim and release

`try_claim` performs:

1. relaxed load of consumer-owned `tail`;
2. acquire load of producer-owned `head`;
3. return `None` if equal; otherwise return a claim for `tail % N`.

Only an initialized grant can Release-publish `head`, so the Acquire both
orders the slot read and proves it is a valid `T`.

`Claim::drop` and explicit `release` perform one Release store of `tail + 1`.
The producer cannot reuse the slot before its Acquire load observes that
release. There are no compare-exchange retry loops or slot scans.

## 7. Exploratory API

```rust
pub struct SlotPool<T, const N: usize> { /* ... */ }
pub struct Producer<'pool, T, const N: usize> { /* ... */ }
pub struct Consumer<'pool, T, const N: usize> { /* ... */ }
pub enum Reservation<'grant, 'pool, T, const N: usize> {
    Initialized(Grant<'grant, 'pool, T, N>),
    Vacant(VacantGrant<'grant, 'pool, T, N>),
}
pub struct VacantGrant<'grant, 'pool, T, const N: usize> { /* ... */ }
pub struct Grant<'grant, 'pool, T, const N: usize> { /* ... */ }
pub struct Claim<'claim, 'pool, T, const N: usize> { /* ... */ }

impl<T, const N: usize> SlotPool<T, N> {
    pub const fn new(slots: [T; N]) -> Self;
    pub const fn new_uninit() -> Self;
    pub fn try_producer(&self) -> Option<Producer<'_, T, N>>;
    pub fn try_consumer(&self) -> Option<Consumer<'_, T, N>>;
}

impl<T, const N: usize> Producer<'_, T, N> {
    pub fn try_reserve(&mut self) -> Option<Reservation<'_, '_, T, N>>;
}

impl<T, const N: usize> VacantGrant<'_, '_, T, N> {
    pub fn write(self, value: T) -> Grant<'_, '_, T, N>;
    pub fn cancel(self);
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
`Deref<Target = T>`. Endpoints are `Send + !Sync` when `T: Send`, matching the
existing handle model. Guards are deliberately `!Send + !Sync`.

## 8. Failure and teardown paths

| Event | Result | Bound | User `T` drop? |
|---|---|---:|---:|
| reserve while full | `None`, no state change | O(1) | no |
| drop/cancel vacant grant | slot stays uninitialized and free | O(1) | no |
| `write(T)` then cancel | initialized residue stays free | O(1) | no |
| drop/cancel initialized grant | mutations stay free and unpublished | O(1) | no |
| claim while empty | `None`, no state change | O(1) | no |
| drop/release claim | slot becomes free | O(1), one Release store | no |
| drop endpoint | endpoint can be reacquired | O(1), one Release store | no |
| drop pool | initialized prefix values are dropped once | O(initialized) | yes |

A guard borrows its endpoint, and endpoints borrow the pool, so pool teardown
cannot overlap a live access. Hot-path RAII invokes no user code and has no
panic path. User `T::drop` runs only during pool teardown.

## 9. Evaluation contract

These clauses are implemented and evidence-backed, but remain an exploratory
contract until the admission decision and review close.

**Ownership and initialization**

- **SP-O1:** at most one producer and one consumer endpoint are active.
- **SP-O2:** at most one reservation and one claim are active per endpoint.
- **SP-O3:** producer and consumer never access the same slot concurrently.
- **SP-O4:** no type with a `commit` method exists until its slot contains a
  valid `T`.
- **SP-O5:** each initialized `T` is dropped exactly once with the pool; no
  uninitialized slot is dropped.

**Publication and delivery**

- **SP-P1:** only `Grant::commit` makes a producer-owned slot claimable.
- **SP-P2:** a committed slot is observed only after all construction and grant
  mutations.
- **SP-C1:** claims are returned in commit order.
- **SP-C2:** a claimed slot cannot be reused until release.
- **SP-C3:** cancellation publishes nothing and does not roll contents back.

**Bounds and overload**

- **SP-B1:** reserve, write, commit, claim, cancel, and release contain no
  unbounded loop or capacity-dependent scan.
- **SP-B2:** occupancy never exceeds `N`.
- **SP-B3:** full-pool overload rejects the new reservation explicitly.
- **SP-B4:** no hot-path operation allocates, panics, or invokes user drop code.

## 10. Representative BlockBuf integration

The unit test
`representative_block_handoff_fills_and_claims_the_same_storage` uses a
pre-initialized complete-block slot containing sequence metadata, logical
length, and an inline sample array. The producer fills all samples directly
through `Grant`, commits, and the consumer validates the block through `Claim`.
The test pins pointer identity between the producer and consumer views, so a
future change that reintroduces a publication copy cannot pass unnoticed.

The probe demonstrates the SlotPool foundation without merging or coupling to
the separate BlockBuf candidate branch:

- partial logical initialization remains metadata inside the reusable block;
- only a complete block is committed;
- cancellation leaves explicit reusable residue;
- publication transfers the slot, not a block-sized value; and
- cache/DMA completion remains caller policy.

This answers the representative-integration development item. It does not
close decision **S**: whether that integration justifies admission remains
sequenced behind the deferred **P** budget on issue #26.

## 11. Evidence

| Contract area | Evidence |
|---|---|
| FIFO, cancellation, backpressure, endpoint uniqueness | unit tests |
| initialization witness and no uninitialized claim | type state, compile-fail doctest, Miri unit tests |
| initialized/uninitialized drop obligations | non-`Copy` drop-counter tests + Miri |
| commit/claim exclusivity and ordering | Loom tracked-slot model |
| grant-drop cancellation interleavings | dedicated Loom tracked-slot model |
| representative zero-copy block handoff | pointer-identity block integration test |
| fixed state-path cost | pinned QEMU instruction regions |
| cross-target flash | all eight gated code-size targets |
| `no_std`, embedded targets, zero dependencies | standard repository matrix |

The two SlotPool Loom models use capacity one and Loom's tracked cell access.
That makes a missing publication or reuse ordering edge surface as an actual
overlapping slot access. One model covers normal commit/claim/release; the
other mutates and drops a grant, then proves that only the later committed
value can be claimed.

Miri's detector-on matrix runs the initialization, cancellation, teardown, and
block integration tests on the host, 16 scheduler seeds for the explicit
initialization case, i686, ARMv7, and big-endian s390x. The compile-fail
doctest separately proves that safe source cannot call `commit` on
`VacantGrant`.

Target measurements are recorded in
[`slot-pool-measurements.md`](slot-pool-measurements.md). In the pinned QEMU
10.0.11 Cortex-M3 environment:

| Path | Instructions |
|---|---:|
| reserve available / reserve full | 14 / 13 |
| commit | 4 |
| claim empty / claim available | 14 / 16 |
| release | 3 |
| initialize and commit | 13 |

Cross-target emitted code is 30–56 bytes for reserve/cancel, 32–72 bytes for
initialized reserve/mutate/commit, 34–84 bytes for initialize/commit, and
40–72 bytes for claim/read/release.

## 12. Remaining decisions

The development follow-ons named on issue #31 are complete. The remaining work
is review and admission policy, not missing implementation evidence:

1. **P — budget reading:** compare Copy-composition measurements with a named
   ISR/task instruction/time budget and RAM envelope. The record still must not
   invent an application-wide threshold.
2. **S — admission path:** after P, decide whether SlotPool is admitted as a
   standalone primitive, as the BlockBuf foundation, or not in this cycle.
3. **API review:** if admitted, decide whether `Reservation`/`VacantGrant`
   names and Deref surfaces are the publication contract.
4. **Scope review:** one reservation and one claim in flight remain deliberate.
   A requirement for more invalidates the cursor-only representation.
5. **Trait review:** `ReservableSink`/`ClaimSource` wait for a second concrete
   implementation; admission alone does not automatically publish them.

## 13. Promotion bar to PROPOSED

The implementation and evidence portions of the original promotion bar are
now met: precise state machine, type-state initialization witness, failure and
teardown contract, SlotPool-specific Loom/Miri coverage, target measurements,
and a representative complete-block integration.

Promotion still requires maintainer closure of **P** and **S**, followed by
review of the SP clauses and public names. Until those decisions are recorded,
the feature stays experimental and the document does not claim PROPOSED
status.
