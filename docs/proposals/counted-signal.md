# CountedSignal: multiplicity without payloads

- **Status:** PROPOSED — shared handle decision, contract, and admission
  evidence complete; ready for evaluation.
- **Origin:** substantive design text moved from the
  [bounded-handoff taxonomy](exploratory-primitives.md) §5 (received
  2026-08-11); triaged Tier 1 in [`../0.3.0-candidates.md`](../0.3.0-candidates.md) §4.
- **Taxonomy row:** retention *count per condition* · overload *counter
  saturates* · representative use *pulse/event accumulation*.
- **Contract:** [`counted-signal-contract.md`](counted-signal-contract.md) —
  short abstract contract with stable clause IDs and an evidence map.
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

## 3. Resolved candidate decisions

- Saturation uses the sole-producer algorithm in §3.1: one Relaxed load and,
  below `u32::MAX`, one Relaxed `fetch_add`. There is no retry loop.
- `u32::MAX` is the observable saturation sentinel, wrapped in
  `CountSnapshot`; exact excess beyond the sentinel is intentionally absent.
- `take_count` is a `swap(0)` and reports one take interval, not a running
  total.
- The initial type is one `u32` counter. Applications compose a fixed array of
  signals when they need multiple classes; a dedicated multi-counter type or
  dynamic registry needs a concrete caller and separate evidence.
- The width is `u32`, matching the crate's atomic shim and every shipped
  32-bit target. Width genericity would create target-dependent contracts and
  needs its own evidence before it can earn a separate type.

## 3.1 Candidate result: exact bounded saturation requires one producer

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

The cycle probe has also been run in the pinned reference environment via
`./scripts/verify.sh cycles`:

| Cortex-M3 hot path | Retired guest instructions |
|---|---:|
| `increment` | 8 |
| `take_count` | 7 |

Environment stamp: rustc 1.92.0 (`ded5c06cf`), LLVM 21.1.3, QEMU 10.0.11
(Debian trixie), release `opt-level = "z"` with LTO. The runner subtracts its
marker overhead and uses one guest instruction per translation block with
`-icount shift=0`; these are deterministic instruction/tick counts for that
environment, not a universal microarchitectural cycle claim. Together with
the eight-target code-size rows, this closes the lane's missing pinned-cost
evidence.

## 3.2 Shared handle decision

The accepted EventFlags/CountedSignal answer is sole-producer/sole-consumer
`Send + !Sync` handles with `&self` hot-path operations. For CountedSignal the
choice is load-bearing: it makes §3.1 exact and bounded. EventFlags does not
need exclusivity for `fetch_or` correctness, but loses no guarantee under the
same conservative ownership model. A future MPSC signal can be added as a
separate type after it has its own contract and evidence; it cannot weaken this
type's proof.

## 4. Promotion bar to PROPOSED

1. **Complete for the SPSC candidate:** the bounded-saturating-increment
   question has the load-plus-conditional-RMW answer proved in §3.1.
2. **Complete:** the shared handle model is sole-role `Send + !Sync` handles
   with `&self` operations for both this lane and EventFlags. The proof
   dependency is recorded in the contract's H-decision section.
3. **Complete:** the short
   [`counted-signal-contract.md`](counted-signal-contract.md) gives stable
   I/T/A/B/H/X clause IDs and maps each guarantee to evidence.
4. **Complete for the SPSC candidate:** eight-target code-size rows and pinned
   Cortex-M3 instruction counts carry the cost case; the unit boundary test,
   threaded stress, Miri, and two Loom models cover atomic take,
   no-lost-increment accounting, and saturation without wrapping.

The promotion bar is complete; the candidate is ready for evaluation.
