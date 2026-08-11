# Engineering records

One document per shipped type, at `docs/records/<type>.md` — the enduring
engineering record. Established by maintainer decision, 2026-08-11.

## What a record is

The record answers the questions an engineer asks *before* and *around*
using a type, in the order they ask them — an executive briefing first,
the supporting work behind it second:

1. **Value statement.** Why the type exists, the niche it serves, and what
   it refuses to be. Short — a reader decides in one paragraph whether
   this type is even a candidate for their problem.
2. **Risks and integration concerns.** The decide-*against*-us
   information, in integrator terms: boundaries, failure modes, costs,
   and the contract's non-promises translated into consequences. This
   section is honest first and flattering never — its standard is that a
   downstream user can reject the type with full information.
3. **Technical claims and validation.** Every load-bearing claim the type
   makes, each mapped to the evidence that proves it and where that
   evidence lives. A claim without an evidence row is a claim nobody is
   keeping true.
4. **The record.** The work behind the briefing: decision history with
   rationale, measurements, the proof narrative, rejected alternatives
   (kept with their reasons — rejections teach), and links to the
   contract, proposal, evaluation, and measurement documents.

## What a record is not

- **Not rustdoc.** Rustdoc says how to use the type and carries the
  per-item disclosures; the record says why the type is trustworthy,
  what it cost, and how we know. Where both must state the same fact
  (a contract non-promise, a measured bound), the record cites the
  clause rather than restating the prose.
- **Not the contract or proposal.** Those are the normative sources; the
  record is the briefing over them, and it links rather than forks.
  Divergent copies of normative text are the first integration hazard.

## Rules

- A candidate lane **owes its record as part of its acceptance package**
  — a type is not accepted into a release without one (cycle mechanics,
  planning record).
- Records obey the stale-claims discipline: when a decision closes, a
  measurement lands, or a claim changes, the record is updated in the
  same change that moves the canonical source.
- The 0.2.0 types (`RingBuf`, `EventBuf`, `SeqRing`) receive records
  retroactively when next materially touched.
- [`latest-buf.md`](latest-buf.md) is the exemplar for structure and
  depth.
