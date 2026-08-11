# EventBuf — engineering record

- **Status:** shipped since 0.1.x; 0.2.0 made its costs measured and gated;
  0.3.0 removed the deprecated panicking constructors (#25) and confirmed
  it as the queued transport in the D3 block composition
  (`EventBuf<Block<T, N>, Q>`). Record written retroactively per mechanics
  rule 12 (materially touched by the 0.3.0 breaking anchor).
- **Normative sources:** the module documentation in `src/event_buf.rs`,
  AGENTS.md (invariants, the backpressure proof, worked rejections), the
  0.2.0 changelog (measured claims), `scripts/codesize.sh` baseline rows.

## 1. Value statement

`EventBuf<T, N>` is the channel for **when every event matters**: a
bounded, lock-free SPSC ring that **rejects** the newest push when full —
`push` returns `Err(val)` with the value handed back — so no event is
ever silently lost and the producer always knows, at the call site, when
delivery failed. It is a classic Lamport queue: each side owns its own
cursor, Release/Acquire pairs on the cursors are the publication fence,
and producer and consumer never touch the same slot. Unlike `SeqRing` it
is **fully race-free** — no deviation, no caveat; Miri's detector passes
it outright. It refuses to be: lossy (that is `SeqRing`), freshness-first
(that is `LatestBuf`), or an unbounded queue (nothing in this crate is).

## 2. Risks and integration concerns

- **Backpressure is returned, not handled.** `Err(val)` puts the policy
  decision — retry, log, count, deliberately discard — at the call site,
  every time. A producer that cannot afford to handle "full" belongs on
  `SeqRing`, where overload is absorbed and counted instead of returned.
  With `Block` payloads the returned value is the *entire completed
  block* (`#[must_use]`, within 5–31 instructions of the accepted path)
  — budget rejection like acceptance, not like error plumbing.
- **`N` is the backpressure threshold, chosen per application.** The
  point at which `push` starts failing is a sizing decision the crate
  cannot make; too small converts load spikes into rejection storms.
- **Fixed RAM, `N` slots always** — const-constructed into `.bss` (no
  flash image, no startup copy, measured in the 0.2.0 268-byte/11-target
  result). With block payloads that is `Q × size_of::<Block<T, N>>()`
  on top of the fill-side builder — state the per-shape number.
- **Role lifecycle follows the crate doctrine:** sole-role
  `Send + !Sync` handles, `try_*`-only acquisition since 0.3.0, roles
  held until the handle drops, deliberately no out-of-band reset (the
  LatestBuf contract's X8 states the boundary; the mechanism here is
  the same).

## 3. Technical claims and validation

| Claim | Evidence | Status |
|---|---|---|
| Race-free — no data race on slots or cursors, no deviation | Miri with the race detector on (full pass, no accommodation); Loom models incl. the full-buffer backpressure path | Proven |
| No silent loss — every failed delivery is observable at the call site | `push` returns `Err(val)`; the type has no overwrite path; unit + threaded stress incl. lossless-and-ordered Loom model | Proven |
| Bounded operations, no CAS retry loop | Lamport single-owner cursors: one Relaxed load + one Acquire load + one Release store per side | Proven by construction; cycle rows measured in 0.2.0 |
| `len` reads a consistent cursor pair | The bracketed `tail`/`head`/`tail` sampling documented in the module docs | Pinned |
| Const-constructs into `.bss` | 0.2.0 measurement across 11 targets; codesize baseline gated in CI | Measured, gated |
| Queued block transport (D3) adds no new concurrency contract | The block layer adds no atomics; existing EventBuf clauses apply with `T = Block<…>`; publication-cost matrix at `bc54a9a` | Measured |

## 4. The record

- **Backpressure over overwrite is the type's identity** (AGENTS): the
  crate's predictability rule demands loss be either impossible or
  reported; `EventBuf` is the "impossible" branch, `SeqRing` the
  "reported" branch, and the pair brackets the design space the
  taxonomy's admission rule polices.
- **D3 (closed 2026-08-11):** `EventBuf<Block<T, N>, Q>` is the queued
  arm of the confirmed block composition — no `QueuedBlockBuf` type
  exists; existing admission, rejection, and race-freedom carry over
  unchanged. Decision P's publication-cost rows (159–8,658 reference
  instructions; rejection within 5–31 of acceptance) are the measured
  cost basis.
- **Shared worked rejections (AGENTS):** `try_split()` rejected on the
  0.2.0 measurement (+43–59% flash on constrained cores); the panic
  machinery removed with the 0.3.0 constructor removal — the 0.2.0
  code-size probe showed no panic strings reach the binary on the
  `try_*` path.
- **0.3.0 changes:** panicking constructors removed (#25); Loom models
  drive `try_*` directly, so the proven orderings are the shipped
  orderings.
