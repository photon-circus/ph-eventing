# BlockBuf: complete window handoff (exploratory design document)

- **Status:** EXPLORATORY — design exploration vehicle, not yet PROPOSED.
- **Origin:** substantive design text moved from the
  [bounded-handoff taxonomy](exploratory-primitives.md) §2 (received
  2026-08-11); triaged Tier 1 in [`../0.3.0-candidates.md`](../0.3.0-candidates.md) §4.
- **Taxonomy row:** retention *latest complete block* · overload
  *replace/reject whole block* · representative use *DMA, IMU/DSP windows*.
- **Related:** [`latest-buf.md`](latest-buf.md) §11 (the complete-block
  variant sketch) and [`latest-buf-contract.md`](latest-buf-contract.md)
  decision point **D3** — this document and D3 must be resolved together.
  **Resolved (2026-08-11):** D3 is closed as the convergent answer —
  payload-agnostic `LatestBuf<T>`, blocks as payloads, composition as the
  type identity (contract §9). The developed candidate on
  `candidate/block-buf` carries the confirmed design; this seed document is
  historical.

## 1. Design sketch (from the taxonomy)

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

> **Superseded (2026-08-11):** the sketch below, including its
> `Block<T, Stamp, N>` parameterisation and the three named types that
> follow, is the historical seed. D3's closure accepted the developed
> candidate's `Block<T, N>` (payload and count only; stamps travel inside
> `T`) with composition as the type identity — `LatestBuf<Block<T, N>>`
> latest, `EventBuf<Block<T, N>, Q>` queued, caller policy for drop-new.
> See §5.1 of the developed document on `candidate/block-buf` and the
> contract §9.

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

## 2. Triage notes (0.3.0 cycle)

- **D3 coupling.** A `LatestBlockBuf` is close to "`LatestBuf` where `T` is
  a block." The LatestBuf contract's decision point D3 (sample-vs-block
  first deliverable) and this document answer the same question from two
  directions; resolving them separately risks designing the same thing
  twice. The LatestBuf contract clauses (G/P/C/O/B/H) look nearly
  wholesale-reusable for the `Latest` variant with `T = Block<…>`.
- **Separate compile-time types per overload policy** matches the taxonomy's
  no-runtime-policy-branches rule and this repo's predictability priority.
- **Publication cost is not free just because synchronization is.** The
  synchronization step is constant, but publishing a block copies (or
  transfers ownership of) `N` samples; the worst case must be measured
  against the target's acquisition budget (per the LatestBuf proposal §11),
  and is the strongest argument for building on [`SlotPool`](slot-pool.md)
  ownership transfer rather than copying.

## 3. Open questions

> **Superseded (2026-08-11):** the type-identity questions below are
> answered by D3's closure — composition over payload-agnostic
> primitives, no new named types (contract §9). The fill-side,
> `Copy`-vs-`SlotPool`, and stamp questions are answered in the developed
> document on `candidate/block-buf` (`BlockBuilder`, decision **P** —
> closed 2026-08-11 as Copy composition, see the planning record's P/S
> closure — and stamps-inside-`T` respectively). Retained unedited as the
> seed record.

- Is `LatestBlockBuf` a new type or `LatestBuf<Block<T, Stamp, N>>` plus a
  fill-side helper? What, concretely, does a separate type buy?
- Is `QueuedBlockBuf` a new type or `EventBuf<Block<…>>`? If the answer is
  "mostly `EventBuf`," the deliverable may be the `Block` type, the
  fill-side builder, and documentation rather than a new primitive.
- `DropBlockBuf` discards the *newly completed* block — who observes the
  discard, and how does that report compose with the filtered-vs-lost
  vocabulary?
- Fill-side API: what does the producer hold while filling (a builder? a
  `SlotPool` grant?), and what happens to a partially filled block on
  producer teardown?
- Does `Block` stay `Copy` (crate convention) at realistic `N`, or does the
  copy cost at publication force the `SlotPool` foundation from the start?
- `Timestamped` dependency: the sketch embeds per-sample stamps — is that
  mandatory, or a parameterisation the caller can collapse to zero size?

## 4. Promotion bar to PROPOSED

1. Resolve D3 jointly with this document (maintainer decision). **Done
   2026-08-11** — closed as composition; see the contract §9.
2. Answer the type-identity questions above — in particular whether each
   variant is a new primitive or a composition over existing/planned ones.
3. Develop to the detail level of [`latest-buf.md`](latest-buf.md); derive
   the contract (expected: a delta on the LatestBuf contract for the
   `Latest` variant).
4. Then the standard evidence bar: codesize on all gated targets, cycles in
   the reference environment (publication cost vs `N` stated explicitly),
   Miri detector-on, Loom, and the memory cost (~3 blocks for the `Latest`
   variant) stated per target in the docs.
