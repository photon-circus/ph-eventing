# BlockBuf publication-cost matrix

- **Measured:** 2026-08-11
- **Source branch:** `candidate/block-buf` at `e21c7fa`, plus the isolated
  measurement harness recorded with this document.
- **Compiler:** `rustc 1.92.0 (ded5c06cf 2025-12-08)`.
- **Cycle reference:** `QEMU emulator version 10.0.11`, from the repository's
  `ph-eventing-verify` image.
- **Shapes:** sample widths 2, 8, and 16 bytes; `N = 8, 32, 128`.

This is the measurement required by [block-buf.md](block-buf.md) section 6 and
decision **P** on issue #26. It measures the final `BlockBuilder::push` that
completes a block together with `EventBuf<Block<...>, 1>::push`. The builder is
pre-filled with `N - 1` samples outside the measured region, so the acquisition
loop is not charged to publication.

Accepted and rejected publication are separate instruction regions. Rejection
preserves and returns the complete block, so both paths cross two logical
block-sized value boundaries after the final sample:

- accepted: builder completion, then slot publication;
- rejected: builder completion, then return of the rejected block.

The byte columns below are those conservative logical boundaries. Compiler
move elision is deliberately not claimed; the retired-instruction counts are
the measurement of the optimized result.

## Reference instruction counts

Run with:

```sh
./scripts/verify.sh cycles block-matrix
```

| Width | N | `Block` bytes | Logical bytes/path | Accepted instructions | Rejected instructions |
|---:|---:|---:|---:|---:|---:|
| 2 | 8 | 24 | 48 | 159 | 151 |
| 2 | 32 | 72 | 144 | 606 | 595 |
| 2 | 128 | 264 | 528 | 1,373 | 1,362 |
| 8 | 8 | 72 | 144 | 618 | 611 |
| 8 | 32 | 264 | 528 | 1,385 | 1,378 |
| 8 | 128 | 1,032 | 2,064 | 4,502 | 4,497 |
| 16 | 8 | 136 | 272 | 875 | 844 |
| 16 | 32 | 520 | 1,040 | 2,415 | 2,406 |
| 16 | 128 | 2,056 | 4,112 | 8,658 | 8,644 |

The same probe under local QEMU 10.2.1 reproduced all 18 rows exactly. The
decision record nevertheless uses the pinned 10.0.11 values above because the
runner's trace-boundary attribution is only guaranteed per environment.

## Code size across all gated targets

Run with:

```sh
./scripts/codesize.sh block-matrix
```

Each entry is emitted flash bytes for one final-completion-plus-publication API
shape. Accepted and rejected execution share the same function; their runtime
paths are separated by the instruction probe above.

| Width × N | M0 `thumbv6m` | M23 `thumbv8m.base` | M3 `thumbv7m` | M4 `thumbv7em` | M33 `thumbv8m.main` | ARMv7-R | ARMv7-A | RV32IMAC |
|---|---:|---:|---:|---:|---:|---:|---:|---:|
| 2 × 8 | 138 | 136 | 136 | 136 | 138 | 212 | 212 | 150 |
| 2 × 32 | 168 | 168 | 132 | 132 | 126 | 248 | 248 | 190 |
| 2 × 128 | 200 | 190 | 130 | 130 | 122 | 252 | 252 | 210 |
| 8 × 8 | 192 | 192 | 120 | 120 | 114 | 312 | 312 | 202 |
| 8 × 32 | 224 | 228 | 118 | 118 | 110 | 312 | 312 | 222 |
| 8 × 128 | 240 | 244 | 178 | 178 | 182 | 312 | 312 | 252 |
| 16 × 8 | 242 | 242 | 200 | 200 | 204 | 296 | 296 | 250 |
| 16 × 32 | 254 | 250 | 208 | 208 | 212 | 288 | 288 | 248 |
| 16 × 128 | 268 | 264 | 216 | 216 | 216 | 300 | 300 | 300 |

The non-monotonic flash rows are expected: larger payload moves often lower to
shared copy routines, so payload traffic grows without duplicating the same
amount of inline code. This is why flash and instructions are both required.

## Result for decisions P and S

The matrix establishes that Copy composition is bounded but not constant in
block size. On the reference Cortex-M3, accepted completion plus publication
ranges from 159 to 8,658 retired instructions; preserving a rejected complete
block is within 5–31 instructions of the accepted path rather than being a
cheap scalar error return.

The record still contains no named ISR/task instruction budget and no RAM
envelope. Therefore this measurement does **not** close decision **P** and does
not authorize either branch of **S**:

- if a selected shape's budget covers its accepted row and the extra
  builder-sized storage is acceptable, Copy composition remains the safe
  default;
- if the accepted row exceeds that budget, or the extra builder-sized storage
  is unacceptable, the next authorized work is the representative
  BlockBuf-over-SlotPool integration.

Choosing a threshold here would substitute an arbitrary library-wide number
for the application budget the proposal explicitly requires. The maintainer's
next input is a named instruction/time budget and RAM envelope for the shapes
the cycle intends to support.
