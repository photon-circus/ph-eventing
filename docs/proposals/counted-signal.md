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
