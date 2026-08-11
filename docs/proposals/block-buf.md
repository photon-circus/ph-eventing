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
  **Resolved (2026-08-11):** D3 closed as the composition this document
  recommends (contract §9, PR #37; confirmation recorded in §5 below).

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

## 3. Initial open questions (evaluated in section 5)

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

## 4. Initial promotion bar to PROPOSED

1. Resolve D3 jointly with this document (maintainer decision).
2. Answer the type-identity questions above — in particular whether each
   variant is a new primitive or a composition over existing/planned ones.
3. Develop to the detail level of [`latest-buf.md`](latest-buf.md); derive
   the contract (expected: a delta on the LatestBuf contract for the
   `Latest` variant).
4. Then the standard evidence bar: codesize on all gated targets, cycles in
   the reference environment (publication cost vs `N` stated explicitly),
   Miri detector-on, Loom, and the memory cost (~3 blocks for the `Latest`
   variant) stated per target in the docs.

## 5. Evaluated answers (candidate implementation)

The candidate branch contains `Block<T, N>` and `BlockBuilder<T, N>`. This
small implementation exercises composition and fill-side behavior without
committing to another synchronization primitive.

### 5.1 Type identity

| Working name | Candidate identity | Why |
|---|---|---|
| `LatestBlockBuf` | `LatestBuf<Block<T, N>>` | Every LatestBuf clause applies unchanged: a complete block is the published `T`. A wrapper adds a name, not a guarantee. |
| `QueuedBlockBuf` | `EventBuf<Block<T, N>, Q>` | Existing FIFO admission, rejection, handles, ordering, and race-freedom are the required queued policy. |
| `DropBlockBuf` | Call-site handling of `EventBuf::push`'s `Err(block)` | Drop-new is an action after observable rejection. The caller can count, log, retry, or deliberately discard the returned complete block. |

This resolves the architectural part of D3: sample and block are not mutually
exclusive primitive forms. `LatestBuf<T>` remains payload-agnostic while
`Block<T, N>` is a separately useful payload/fill abstraction. Which ships
first remains release scheduling, not a type-system choice.

**Confirmed (maintainer, 2026-08-11):** D3 is closed exactly this way — the
convergent answer is the answer (contract §9, PR #37). The closure binds a
documentation obligation on the block-payload surfaces: per-shape RAM stated
plainly, the small-`N` publication-cost inversion stated as guidance, and
the no-partial-block limitation stated for adopters.

A named wrapper should be reconsidered only if it enforces a contract that
composition cannot, such as direct-to-granted-slot filling with zero
publication copy. That would materially be a SlotPool/grant transport.

### 5.2 Fill-side contract

`BlockBuilder<T, N>` owns private partial storage. `push(sequence, sample)`:

1. rejects reserved sequence `0`;
2. accepts only the next wrap-aware contiguous sequence;
3. returns `Ok(None)` for the first `N - 1` samples;
4. returns `Ok(Some(Block))` exactly after the `N`th sample; and
5. resets only after successful completion.

A discontinuity returns the sample plus expected/received sequences without
mutating the partial block. The caller explicitly chooses to preserve it or
`clear` and retry the rejected sample as a new window. Dropping or clearing a
partial builder publishes nothing; there is no hidden drop count or panic.

This distinguishes pre-publication **filtered/incomplete windows**, visible as
a fill error or explicit clear, from post-completion **lost blocks**, reported
by the selected transport (`PublishReport::replaced_unread` for LatestBuf or
`EventBuf::push` returning the block).

### 5.3 Payload and timestamp policy

`Block<T, N>` stores `[T; N]` plus inclusive first/last sequences. Timestamping
is payload policy: applications needing per-sample time use a timestamped `T`;
applications needing one stamp per block use their own wrapper. The zero-stamp
case therefore has genuinely zero timestamp overhead.

The candidate keeps `T: Copy`, matching every existing transport and allowing
direct composition. That is a baseline for measurement, not evidence that
copying a large block is cheap.

## 6. Cost model and SlotPool decision

Let `B = size_of::<Block<T, N>>()`. Before alignment padding, it contains
`N * size_of::<T>() + 8` bytes.

| Boundary | Conservative payload traffic |
|---|---:|
| Builder completion to returned `Block` | `B` |
| Publication into EventBuf or a copying LatestBuf | `B` |
| Consumer pop/take into its returned value | `B` |

Optimization may elide a move, but the contract cannot rely on it. Publication
is O(`B`) / O(`N * size_of::<T>()`) even though synchronization is O(1). It is
bounded and compile-time-known, but not constant across block shapes.

Inline memory is equally explicit:

- builder: approximately one block plus `len` and sequence metadata;
- `EventBuf<Block<T, N>, Q>`: `Q * B` plus queue atomics;
- proposed three-slot `LatestBuf<Block<T, N>>`: approximately `3 * B` plus
  control atomics, in addition to fill-side storage;
- SlotPool/grants: pool slots plus control state; the producer fills the
  destination directly and publication transfers an index.

**Decision rule:** keep Copy composition when measured worst-case completion
plus publication fits the acquisition/interrupt budget on every claimed
target. Choose SlotPool/grants when it does not, or when the extra builder-sized
storage is unacceptable. Do not use an arbitrary `N` threshold; the answer
depends on sample width, target, and application budget.

Required matrix before promotion:

- sample widths 2, 8, and 16 bytes;
- `N = 8, 32, 128`;
- accepted and rejected EventBuf publication, plus LatestBuf when available;
- all code-size targets and the reference QEMU cycle target; and
- both instructions and bytes copied, not just atomic handoff cost.

### 6.1 Measurement result (2026-08-11)

The full matrix is recorded in
[`block-buf-measurements.md`](block-buf-measurements.md). It includes exact
code-size rows for all eight gated targets and accepted/rejected instruction
counts from the pinned QEMU 10.0.11 reference environment.

On the reference Cortex-M3, accepted completion plus publication ranges from
159 instructions for 2-byte samples at `N = 8` to 8,658 instructions for
16-byte samples at `N = 128`. Rejection is within 5–31 instructions of the
accepted path because the complete rejected block is preserved and returned to
the caller. Logical payload traffic ranges from 48 to 4,112 bytes per path.

This supplies the missing measurements but does not close the foundation
decision: no named ISR/task instruction budget or RAM envelope exists in the
record. Decision **P** therefore remains open, and decision **S** must continue
to wait. The next maintainer input is the budget for the supported shapes; only
then can the branch choose Copy composition or authorize the representative
BlockBuf-over-SlotPool integration.

## 7. Deferred choices and decision evidence

| Choice | Safe default | Evidence that changes it |
|---|---|---|
| Block or sample LatestBuf first (D3 scheduling) | Implement generic `LatestBuf<T>`; blocks already compose | A block-first integration partner or benchmark showing the sample payload misses the relevant costs |
| Copy composition or SlotPool | Copy composition | Section 6 exceeds a named ISR/task budget or RAM envelope |
| Automatic reset after a gap | Reject without mutation | Real integrations converge on one restart and accounting policy |
| Mandatory per-sample timestamps | Caller-selected `T` | A transport-level time contract cannot be expressed in payload |
| Dedicated `DropBlockBuf` | Observe and handle `Err(block)` | A distinct synchronization/accounting contract, not convenience |
| Extra block-wide metadata | Caller wrapper | Multiple integrations need one invariant enforced by this crate |

## 8. Block contract delta

No concurrency contract is added by composition. Latest delivery uses the
LatestBuf contract with `T = Block<T, N>`; queued delivery uses EventBuf with
the same substitution. The block-only, pre-publication delta is:

- **F1 Complete-only:** no public `Block` is yielded before all `N` samples are
  initialized.
- **F2 Contiguous:** represented sequences are consecutive under the successor
  that skips reserved zero.
- **F3 Explicit interruption:** reserved/discontinuous input is returned and
  does not silently alter the partial block.
- **F4 Teardown:** clearing or dropping partial state publishes nothing.
- **F5 Reuse:** completion resets the builder for a new independent block.

Unit tests pin F1-F5, including wrap and a `T: Copy` type without `Default`.
Miri remains required because completion copies initialized `MaybeUninit`
storage. The block layer adds no atomics or shared mutable state, so Loom adds
no block-specific evidence; Loom remains required for the selected transport.

## 9. Remaining promotion bar

1. Maintainer confirms the type-identity recommendation and treats D3 as a
   scheduling decision rather than separate sample/block primitive designs.
   **Done 2026-08-11** — D3 closed as composition (contract §9, PR #37).
2. Compare the completed section 6 matrix against named budgets; choose Copy
   composition or SlotPool/grants from the result. The measurement is complete;
   naming the instruction/time and RAM budgets remains a maintainer decision.
3. Run standard CI and Miri; run Loom for the selected transport.
4. If Copy wins, decide which completed matrix rows become release baselines.
   If SlotPool wins, specify grant teardown and ownership proof before
   PROPOSED.
