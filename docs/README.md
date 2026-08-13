# Documentation map

Three layers, split by the question they answer and how long the answer
stays true:

- [`records/`](records/) — **engineering records**, one per shipped type.
  The enduring briefing layer: value statement, risks, every load-bearing
  claim mapped to the evidence that keeps it true. Read these first when
  deciding whether to use a type. What a record is (and is not) is defined
  in [`records/README.md`](records/README.md).
- [`proposals/`](proposals/) — **design-decision documents**: proposals,
  frozen contracts, evaluations, and measurement reports. These record how
  and why a decision was made, in the state the decision was made in; each
  header states its closed outcome (shipped, deferred, rejected). They are
  history — corrections land in the records, not here.
- [`planning/`](planning/) — **per-cycle planning records**
  (e.g. [`planning/0.3.0-candidates.md`](planning/0.3.0-candidates.md)):
  candidate triage, evidence bars, and kill criteria for a release cycle.
  Historical once the cycle closes.

Rustdoc (`cargo doc`) is the API contract surface; `AGENTS.md` at the repo
root is the working guidance for changing any of this.
