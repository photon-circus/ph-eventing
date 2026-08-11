# SeqRing — engineering record

- **Status:** shipped since 0.1.x; 0.2.0 made its costs measured and gated;
  0.3.0 removed the deprecated panicking constructors (#25). Record written
  retroactively per mechanics rule 12 (the type was materially touched by
  the 0.3.0 breaking anchor).
- **Normative sources:** the module documentation in `src/seq_ring.rs`
  (the canonical seqlock-deviation analysis lives there), AGENTS.md
  (invariants and worked rejections), the 0.2.0 changelog (measured
  claims), `scripts/codesize.sh` baseline rows.

## 1. Value statement

`SeqRing<T, N>` is the **ordered recent window**: a lock-free SPSC
overwrite ring for high-rate telemetry where the producer must never
block and stale data is droppable — but unlike a latest-value snapshot,
the consumer drains *in order* (`poll_one`/`poll_up_to`) or samples the
newest (`latest`), and losses on the ordered path are counted. A consumer
that falls more than `N` behind skips ahead and is told exactly how many
items it lost (`PollStats::dropped`; the saturating `dropped_accum` is the
origin of the crate's observable-loss convention). The exactness promise
belongs to the ordered `poll_*` paths only: `latest()` samples without
advancing the cursor or reporting a gap, and `skip_to_latest()` discards
the backlog *without* adding to the dropped counter — both are deliberate
sampling/fast-forward semantics, disclosed in §2 so they are chosen, not
discovered. It refuses to be: a delivery
guarantee (`EventBuf` is), a race-free primitive (see §2 — this is the
crate's one deliberate formal-soundness trade), or a latest-value-only
channel (`LatestBuf` is, with a race-free ownership argument).

## 2. Risks and integration concerns

- **The seqlock deviation — the crate's one accepted formal data race.**
  The consumer may copy a slot the producer is overwriting; the sequence
  re-check discards the racy copy as raw `MaybeUninit` bytes. Miri's
  race detector is *right* to flag this: `read_volatile` constrains the
  compiler but is not atomic. The module docs carry the full analysis,
  including the honest bounds: nothing is known to miscompile (the Linux
  kernel's `seqlock_t` is the same construct), but "no known failure" is
  not a guarantee, and **your own Miri runs over consumer-under-writer
  tests will report UB** — the crate's own Miri matrix runs a dedicated
  seqlock pass for exactly this reason. Generality (`T: Copy`) was
  chosen over formal soundness deliberately; a word-payload-restricted
  ring would be fully race-free and remains referential (C1). If your
  certification regime cannot accept a documented deviation, use
  `EventBuf` (fully race-free) or `LatestBuf` (race-free by ownership).
- **Extra drops at the sequence wrap — known limitation.** Around the
  `u32` sequence wrap the skip-ahead can drop a few more entries than
  the lag alone requires (bounded, `N`-dependent, documented in the
  module docs). Realignment was analysed and left referential (C2, in
  the planning record) — reopen it with a real adopter, not by default.
- **Loss is designed behaviour.** Overwrite is the overload policy; the
  counters report it, nothing prevents it. Consumers that must see every
  event belong on `EventBuf`.
- **Two consumption modes bypass the loss accounting — by design.**
  `latest()` peeks the newest value without advancing the cursor or
  producing a skipped count, and `skip_to_latest()` fast-forwards past
  the backlog without modifying the dropped counter. A consumer mixing
  these with `poll_*` must not read `dropped`/`dropped_accum` as a total
  loss ledger — it accounts only what the ordered path drained past.
- **Role lifecycle follows the crate doctrine:** sole-role
  `Send + !Sync` handles, `try_*`-only acquisition (the panicking
  constructors are gone in 0.3.0), roles held until the handle drops,
  deliberately no out-of-band reset (the LatestBuf contract's X8 states
  the boundary; the mechanism here is the same).

## 3. Technical claims and validation

| Claim | Evidence | Status |
|---|---|---|
| No torn value ever materialises as `T` (racy copies discarded before use) | Volatile access + `MaybeUninit` holding + re-check discipline; Loom models; the dedicated seqlock Miri pass | Proven within the documented deviation |
| Fence placement is necessary (Release before value write; Acquire before re-check) | Module-doc argument; Loom; ordering discipline enforced by AGENTS' rerun-after-atomic-change rule | Proven |
| Recovery cost is constant w.r.t. lag | 0.2.0 measurement: a consumer 2,000 sequences behind recovers in the same 115 instructions as one 16 behind | Measured |
| Loss accounting is exact and saturating (`read + dropped` accounts for every sequence; `dropped_accum` saturates, never wraps) | Unit set incl. `lag_across_wrap_counts_drops_exactly`, `dropped_accum_saturates_instead_of_overflowing`; `seq_distance` wrap tests (0.1.3 fix pinned) | Pinned |
| Const-constructs into `.bss` — no flash image, no startup copy | By construction (`const fn new`); the 0.2.0 codesize probe demonstrates the crate's `.bss` discipline on an `EventBuf` static (268 B) — **no dedicated SeqRing static is probed or baseline-gated**, and SeqRing's RAM adds `N` per-slot sequence atomics over the payload array, `size_of`-computable per shape | By construction; probe coverage is EventBuf's |

## 4. The record

- **The seqlock trade (module docs, §"Known deviation"):** three
  alternatives rejected on the record — blocking the producer (removes
  the reason the type exists), per-word atomic copies (needs unstable
  `generic_const_exprs`; per-byte atomics hit padding UB), and
  word-restricted payloads (fully sound, passed over for generality;
  kept referential as C1).
- **`pop` + `Source` worked rejection (AGENTS):** O(1) and bounded, but
  overwrite would lose data `forward` structurally cannot report —
  unreportable loss is unpredictable behaviour. The same reasoning later
  shaped LatestBuf's D2.
- **`try_split()` rejected on measurement (AGENTS):** better ergonomics,
  +43–59% flash on the constrained cores least able to pay — invisible
  on Cortex-M4, which is why one-target evidence is banned (mechanics
  rule 9's scar).
- **0.3.0 changes:** panicking constructors removed (#25); the Loom
  models now drive the `try_*` path directly, so the proven orderings
  are the shipped orderings.
- **Relationships:** `LatestBuf` covers race-free latest-value with
  general payloads; `SeqRing` remains the ordered-recent-window; the
  taxonomy's C1/C2 entries hold its two reopening paths.
