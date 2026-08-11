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

- Saturation uses the sole-producer algorithm in §3.1: a Relaxed load, with
  an observed `u32::MAX` re-read through a no-op RMW (`fetch_or(0)`) that is
  guaranteed to see the latest value in modification order — a fixed
  instruction sequence on every gated ISA, with no compare-exchange and
  therefore no LR/SC retry loop on RISC-V.
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
let observed = count.load(Relaxed);
if observed == u32::MAX && count.fetch_or(0, Relaxed) == u32::MAX {
    return; // RMW re-read confirms current saturation: absorbed no-op
}
count.fetch_add(1, Relaxed);
```

A plain load-and-skip-on-`MAX` short-circuit is **not** exact: after
`take_count` has returned, a later `increment` may still observe a stale
`MAX` under Relaxed ordering and drop the occurrence from both take
intervals (violating T3/A1). The sentinel path therefore re-reads through a
no-op RMW — an RMW, unlike a load or a failed compare-exchange, observes the
latest value in modification order, so `MAX` proves current saturation
(absorbed no-op) and anything else falls through to `fetch_add` into the
post-take epoch. Every path is a fixed instruction sequence under B1; the
original sentinel used a strong compare-exchange, which was replaced when
review showed it lowers to an unbounded LR/SC retry loop on RISC-V.

This remains **not** correct for multiple producers: two producers could both
observe `u32::MAX - 1` and the second `fetch_add` would wrap. It is correct with the
crate's existing sole `Send + !Sync` producer handle. Between the load and
RMW (or between the sentinel re-read and the follow-up `fetch_add`), the
consumer's `swap(0)` is the only possible competing write and it can only lower
the value, so the RMW cannot wrap.

Consequences for the deferred decisions:

| Choice | Evaluation result |
|---|---|
| Sole `Send + !Sync` producer, methods take `&self` | Exact saturation; common path is one load plus one `fetch_add`; sentinel path adds one no-op RMW re-read and at most one follow-up `fetch_add`; matches existing handle ownership. |
| Shareable/multiple raisers | The intervening-writer proof fails; needs an unbounded CAS loop, a weaker overflow contract, or a substantially more complex bounded algorithm. |
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
- Loom exhaustively models ordinary take partitioning, the observe/take/commit
  interleaving at `u32::MAX - 1`, and a post-take increment seeded at `MAX`
  sequenced only by a Relaxed gate;
- the cycle and code-size probes include the ISR-side `increment` and consumer
  `take_count` paths so the remaining admission decision can be made from
  per-target measurements rather than from source shape.

The isolated release-mode code-size rows on the pinned Rust 1.92.0 toolchain
(after the sentinel-RMW fix) expose the architecture split for review:

| Target family | `increment` | `take_count` |
|---|---:|---:|
| Cortex-M0 (`thumbv6m`) | 46 B | 24 B |
| Cortex-M23 (`thumbv8m.base`) | 44 B | 22 B |
| Cortex-M3/M4/M33 | 44 B | 22 B |
| Armv7-R / Armv7-A | 64 B | 28 B |
| RV32IMAC | 26 B | 8 B |

The M0/M23 rows use the existing portable-atomic single-core probe backend.
Versus the pre-fix load-and-skip path this remains a deliberate growth on
gated rows — exactness after a completed take requires the confirming RMW
re-read — but the RMW refinement recovered 8–12 B on seven of the eight
rows relative to the interim compare-exchange form (thumbv6m's
critical-section row is unchanged), and on RV32IMAC the sentinel is a
single `amoor.w` with no `lr.w`/`sc.w` pair in the disassembly.

Re-measured in the pinned reference environment via `./scripts/verify.sh cycles`
after the sentinel fix. The probe brackets the common below-`MAX` path
(`load` + `fetch_add`), a `swap(0)` take, and — via the hidden `_cycles-probe`
seeding feature, since `u32::MAX` increments cannot be replayed under a
per-instruction trace — the saturated sentinel arm (`load` observing `MAX`,
no-op `fetch_or(0)` confirm, return):

| Cortex-M3 path | Retired guest instructions |
|---|---:|
| `increment` (below `MAX`, the hot path) | 8 |
| `increment` (saturated sentinel arm) | 9 |
| `take_count` | 7 |

The third arm — a stale `MAX` re-read below `MAX` after a completed take —
needs a racing consumer a single-hart deterministic trace cannot express; it
is the saturated arm plus one `fetch_add` by construction and is bounded by
the gated whole-function code size. All three measured rows are uncontended
single-pass counts.

Environment stamp: rustc 1.92.0 (`ded5c06cf`), LLVM 21.1.3, QEMU 10.0.11
(Debian trixie), release `opt-level = "z"` with LTO. The runner subtracts its
marker overhead and uses one guest instruction per translation block with
`-icount shift=0`; these are deterministic instruction/tick counts for that
environment, not a universal microarchitectural cycle claim.

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
   question has the sole-producer sentinel-RMW answer proved in §3.1.
2. **Complete:** the shared handle model is sole-role `Send + !Sync` handles
   with `&self` operations for both this lane and EventFlags. The proof
   dependency is recorded in the contract's H-decision section.
3. **Complete:** the short
   [`counted-signal-contract.md`](counted-signal-contract.md) gives stable
   I/T/A/B/H/X clause IDs and maps each guarantee to evidence.
4. **Complete for the SPSC candidate:** eight-target code-size rows and pinned
   Cortex-M3 instruction counts carry the cost case; the unit boundary test,
   threaded stress, Miri, and three Loom models cover atomic take,
   no-lost-increment accounting, saturation without wrapping, and the
   post-take stale-`MAX` litmus.

The promotion bar is complete; the candidate is ready for evaluation.
