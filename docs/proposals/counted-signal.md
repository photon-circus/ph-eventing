# CountedSignal: multiplicity without payloads (exploratory design document)

- **Status:** EXPLORATORY — design exploration vehicle, not yet PROPOSED.
- **Origin:** substantive design text moved from the
  [bounded-handoff taxonomy](exploratory-primitives.md) §5 (received
  2026-08-11); triaged Tier 1 in [`../0.3.0-candidates.md`](../0.3.0-candidates.md) §4.
- **Taxonomy row:** retention *count per condition* · overload *counter
  saturates* · representative use *pulse/event accumulation*.
- **Related:** [`event-flags.md`](event-flags.md) — same niche (bounded ISR
  notification), different contract (multiplicity matters here); the
  `SignalSink`/`SignalSource` trait vocabulary lives in that document and
  must be proven against both primitives before it is frozen.

## 1. Design sketch (from the taxonomy)

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

## 2. Triage notes (0.3.0 cycle)

- **Saturate-don't-wrap matches the codebase.** The saturation rule is the
  `dropped_accum` convention (`SeqRing`'s counter saturates because
  overflow panicked in debug and wrapped silently in release on 32-bit
  targets); the same reasoning and the same test shape
  (`dropped_accum_saturates_instead_of_overflowing`) apply here.
- **Shared triage with EventFlags:** ISR-side cost measured on
  portable-atomic targets before claims; `&self` handle-model question;
  the atomically-take clause is Loom material. See
  [`event-flags.md`](event-flags.md) §3 — the notes apply verbatim and are
  not duplicated here.

## 3. Open questions

- Saturating increment on a plain atomic needs care: a
  `compare_exchange` loop is unbounded under contention, which violates the
  producer bound — is the answer `fetch_add` with a saturation check that
  tolerates a bounded overshoot window, a claimed-bit scheme, or accepting
  `fetch_add` wrapping on a width wide enough that saturation is
  unreachable in practice (and documenting that instead)? This is the
  design's central question — the naive implementations are either
  unbounded or not actually saturating.
- How is saturation observed — a sticky flag in `take_count`'s return, a
  reserved sentinel value, or a separate query?
- `take_count` semantics: `swap(0)` is the obvious atomic take — confirm
  the contract is "count since last take," not a running total.
- Multi-counter form: fixed `[counter; K]` per event class — is that a
  distinct type, or `CountedSignal` composed by the caller? (The taxonomy
  pre-rejects a dynamic registry; the question is only whether the static
  array earns a type.)
- Counter width: `u32` everywhere (matching the crate's sequence width and
  the 32-bit `usize` of every shipped target), or parameterised?

## 3.1 Exploratory result: exact bounded saturation requires one producer

The candidate branch now carries an executable SPSC design. It found a fourth
answer to the central question that is stronger than the three initially
listed, but only under a deliberately narrow handle model:

```rust
if count.load(Relaxed) != u32::MAX {
    count.fetch_add(1, Relaxed);
}
```

This is **not** correct for multiple producers: two producers could both load
`u32::MAX - 1` and the second `fetch_add` would wrap. It is correct with the
crate's existing sole `Send + !Sync` producer handle. Between that handle's
load and `fetch_add`, the consumer's `swap(0)` is the only possible competing
write and it can only lower the value. The RMW therefore increments either the
observed epoch or the newly reset epoch and cannot wrap. If the load observes
`u32::MAX`, that no-op linearizes before a concurrent take and is already
represented by the saturated snapshot.

Consequences for the deferred decisions:

| Choice | Evaluation result |
|---|---|
| Sole `Send + !Sync` producer, methods take `&self` | Exact saturation; bounded to one load plus at most one RMW; matches existing handle ownership. |
| Shareable/multiple raisers | The two-operation proof fails; needs an unbounded CAS loop, a weaker overflow contract, or a substantially more complex bounded algorithm. |
| `u32::MAX` reserved sentinel wrapped by `CountSnapshot` | Full exact range below the sentinel; `is_saturated()` observes saturation in the take that clears it; the snapshot remains one word and needs no second sticky atomic. |
| `swap(0)` take, relaxed ordering | Counts only, with no payload publication to order. Atomic modification order partitions each increment into exactly one take epoch. |

The prototype deliberately retains handles even though their operations use
`&self`: `&self` is an API receiver choice, not permission to share a handle.
`PhantomData<Cell<()>>` keeps each handle `!Sync`, while the handle itself
remains `Send`. This is directly reusable by EventFlags if that lane chooses
the same ownership model, though EventFlags does not need exclusivity for its
`fetch_or` correctness.

Evidence added with the prototype:

- a unit boundary test seeds `u32::MAX - 1`, increments twice, and observes a
  saturated (never wrapped) snapshot;
- a threaded stress test proves repeated concurrent takes preserve the sum of
  100,000 increments;
- Loom exhaustively models ordinary take partitioning and the load/take/RMW
  interleaving at `u32::MAX - 1`;
- the cycle and code-size probes include the ISR-side `increment` and consumer
  `take_count` paths so the remaining admission decision can be made from
  per-target measurements rather than from source shape.

The isolated release-mode code-size rows on the pinned Rust 1.92.0 toolchain
are small and, importantly, expose the architecture split for review:

| Target family | `increment` | `take_count` |
|---|---:|---:|
| Cortex-M0 (`thumbv6m`) | 30 B | 24 B |
| Cortex-M23 (`thumbv8m.base`) | 28 B | 22 B |
| Cortex-M3/M4/M33 | 28 B | 22 B |
| Armv7-R / Armv7-A | 40 B | 28 B |
| RV32IMAC | 18 B | 8 B |

The M0/M23 rows use the existing portable-atomic single-core probe backend.
Instruction counts still require the pinned QEMU reference environment; the
probe regions are committed, but no cycle number is claimed from a machine
without QEMU.

This result refines rather than silently closes the handle decision: choose
SPSC handles and the central bounded-saturation problem has a small exact
solution; choose multiple raisers and this implementation must be rejected.

## 4. Promotion bar to PROPOSED

1. Solve the bounded-saturating-increment question — it decides whether the
   primitive can honour the crate's boundedness rules at all.
2. Settle the shared handle-model question with
   [`event-flags.md`](event-flags.md) (one answer for both).
3. Write the short contract with citable clause IDs.
4. Standard evidence bar, same emphasis as EventFlags: codesize/cycles on
   the ISR-side `increment` carry the admission case; Loom/Miri close the
   atomic-take and no-lost-increment clauses; the saturation test mirrors
   `dropped_accum_saturates_instead_of_overflowing`.
