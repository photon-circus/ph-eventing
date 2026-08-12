# SlotPool: bounded zero-copy ownership transfer (exploratory design document)

- **Status:** **DEFERRED** (maintainer decision S, 2026-08-11) — evaluated
  in full on `candidate/slot-pool` (draft PR #32, closed as deferred;
  branch preserved and pinned at tag `archive/slot-pool-0.3.0-evaluation`,
  commit `2c3d37f`), then deferred with its evidence
  banked: with cycle decision P closed as Copy composition, no in-release
  consumer exists, and standalone admission would commit the crate's first
  non-`Copy` RAII surface without a real adopter to shape the API.
  **Reopening trigger** (any one suffices): a named adopter whose measured
  budget the Copy path breaches at a supported shape; a
  direct-to-granted-slot (DMA-class) requirement composition cannot
  satisfy; or a standalone zero-copy adopter. See the planning record's
  P/S closure for the full rationale.
- **Prior status:** EXPLORATORY — design exploration vehicle, not yet PROPOSED.
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

## 4. Open questions *(historical — preserved as written before the evaluation)*

The questions below were the pre-evaluation state of this document. The
`candidate/slot-pool` evaluation (banked at tag
`archive/slot-pool-0.3.0-evaluation`) subsequently answered them — the
atomic representation, initialization proof, and measured rows live on that
branch — and decision S then deferred the primitive. They are kept as
written because they define what a reopened evaluation must re-derive if
the banked branch has rotted.

<!-- historical section follows -->
### Original questions

- Ordering discipline for the state transitions: the
  `Free → ProducerOwned → Published → ConsumerOwned → Free` cycle is a
  per-slot state machine — what is the minimal atomic representation, and
  does FIFO ordering across *slots* need to be promised at all, or only
  per-slot exclusivity plus some documented delivery order?
- Grant cancellation on drop: an RAII drop path must stay panic-free and
  bounded — what does a cancelled half-initialized slot return to
  (`Free` with no initialization residue), and how is that pinned?
- Initialization tracking: `MaybeUninit` storage with commit as the
  initialization witness — what prevents a consumer claim from reading
  bytes the producer never wrote (the `RingBuf` `len`-outruns-writes
  lesson, in pool form)?
- Generation tags against stale-handle reuse: width, wrap policy, and
  whether the LatestBuf contract's G-clauses can be reused.
- How many outstanding grants/claims at once? Single-reservation SPSC first
  (matching the crate's handle model), or multiple in flight from day one?
- Is the first deliverable the pool itself, or the pool *as consumed by*
  [`BlockBuf`](block-buf.md) — i.e., does it earn admission standalone or
  as the foundation the block primitive demonstrates?

## 5. Promotion bar to PROPOSED *(historical — superseded by decision S)*

This bar described promotion into the 0.3.0 cycle. Decision S closed as
DEFERRED with the evaluation evidence banked, so the operative gate is now
the **reopening trigger** in the status header, not this list; a future
reopening still owes everything below before PROPOSED.

### Original bar

1. Answer the ownership-model questions above; write the state machine down
   precisely (it is the whole primitive).
2. Develop to the detail level of [`latest-buf.md`](latest-buf.md),
   including the grant/claim lifecycle and its failure paths; derive a
   contract with citable clauses.
3. Then the standard evidence bar — with two additions specific to this
   primitive: Loom models must cover grant-drop (cancellation)
   interleavings, and Miri must cover the initialization-tracking claim
   (uninitialized bytes are never readable through a claim).
