# LatestBuf sample/block composition matrix

- **Measured:** 2026-08-11.
- **LatestBuf source:** `candidate/latest-buf` at `6f46da3`, plus the isolated
  joint probe recorded with this document.
- **Block source:** `candidate/block-buf` at `bc54a9a`.
- **Compiler:** `rustc 1.92.0 (ded5c06cf 2025-12-08)`.
- **Cycle reference:** `QEMU emulator version 10.0.11`, from the repository's
  `ph-eventing-verify` image.
- **Shapes:** sample widths 2, 8, and 16 bytes; `N = 8, 32, 128`.

This is the joint composition evidence requested by issues #27 and #28 for
decision D3. It compares individual-sample publication with complete-block
publication through the same generic `LatestBuf<T>`, and uses the same final
`BlockBuilder::push` plus transport-publication boundary as #28's BlockBuf
matrix.

The candidate branches deliberately do not stack. The probe therefore carries
a private structural twin of #28's `Block<T, N>` and `BlockBuilder<T, N>` at
`bc54a9a`: the same field types, field order, completion path, sample widths,
and target layouts. Only the probe uses these twins; the public LatestBuf API
remains generic and the lane has no dependency on the BlockBuf branch.

Reproduce with:

```sh
XTENSA=1 ./scripts/codesize.sh latest-block-matrix
./scripts/verify.sh cycles latest-block-matrix
```

## Scheduling comparison

The table below compares continuous replacement publication. `N x sample` is
the cost of releasing every acquired sample separately. `Complete block` is
the final builder push plus one `LatestBuf<Block<T, N>>::publish`; the first
`N - 1` private builder pushes are acquisition work outside the publication
region, matching #28's boundary.

| Width | N | One sample | `N x sample` | Complete block | Change at release boundary |
|---:|---:|---:|---:|---:|---:|
| 2 | 8 | 41 | 328 | 173 | -47% |
| 2 | 32 | 41 | 1,312 | 525 | -60% |
| 2 | 128 | 41 | 5,248 | 1,136 | -78% |
| 8 | 8 | 44 | 352 | 541 | +54% |
| 8 | 32 | 44 | 1,408 | 1,152 | -18% |
| 8 | 128 | 44 | 5,632 | 3,640 | -35% |
| 16 | 8 | 69 | 552 | 723 | +31% |
| 16 | 32 | 69 | 2,208 | 1,970 | -11% |
| 16 | 128 | 69 | 8,832 | 6,958 | -21% |

Batching is not uniformly cheaper: at `N = 8`, the 8- and 16-byte shapes pay
more at the measured release boundary. It becomes cheaper for those widths at
`N >= 32`, while the 2-byte shapes benefit throughout this grid. This is a
scheduling trade-off over one generic transport, not evidence for a separate
latest-block synchronization primitive.

## Joint comparison with EventBuf block publication

#28 measures final builder completion plus accepted or rejected
`EventBuf<Block<T, N>, 1>::push`. The matching LatestBuf first/replacement rows
are shown beside it. LatestBuf publication always succeeds; replacement is
observable through `PublishReport` rather than returning the old block.

| Width | N | Block bytes | Latest first | Latest replacement | EventBuf accepted | EventBuf rejected |
|---:|---:|---:|---:|---:|---:|---:|
| 2 | 8 | 24 | 171 | 173 | 159 | 151 |
| 2 | 32 | 72 | 524 | 525 | 606 | 595 |
| 2 | 128 | 264 | 1,134 | 1,136 | 1,373 | 1,362 |
| 8 | 8 | 72 | 542 | 541 | 618 | 611 |
| 8 | 32 | 264 | 1,151 | 1,152 | 1,385 | 1,378 |
| 8 | 128 | 1,032 | 3,644 | 3,640 | 4,502 | 4,497 |
| 16 | 8 | 136 | 719 | 723 | 875 | 844 |
| 16 | 32 | 520 | 1,967 | 1,970 | 2,415 | 2,406 |
| 16 | 128 | 2,056 | 6,958 | 6,958 | 8,658 | 8,644 |

Except for the smallest 24-byte block, LatestBuf's completion-plus-publication
path is 12-20% cheaper than EventBuf's accepted queued publication in this
reference build. That difference reflects distinct overload contracts, not
interchangeability: LatestBuf replaces unread state, while EventBuf preserves
queue order and rejects when full.

## Raw LatestBuf block transport

These regions start with an already-complete block. They isolate the generic
transport from builder completion and include the consumer side needed to
state the full composition cost.

| Width | N | Publish first | Publish replacement | Take pending | Take empty |
|---:|---:|---:|---:|---:|---:|
| 2 | 8 | 102 | 100 | 133 | 96 |
| 2 | 32 | 432 | 430 | 272 | 135 |
| 2 | 128 | 1,206 | 1,204 | 584 | 291 |
| 8 | 8 | 347 | 343 | 282 | 143 |
| 8 | 32 | 857 | 853 | 594 | 299 |
| 8 | 128 | 2,969 | 2,965 | 1,843 | 923 |
| 16 | 8 | 423 | 422 | 387 | 192 |
| 16 | 32 | 1,238 | 1,237 | 1,011 | 504 |
| 16 | 128 | 4,505 | 4,503 | 3,499 | 1,752 |

The probe black-boxes the returned `Option<LatestItem<Block<...>>>`, so the
empty rows include the optimized caller-visible aggregate return path as well
as the channel's Acquire-load fast path. Their payload-size scaling is codegen
at that return boundary, not an atomic or payload read from a channel slot. As
in the standalone matrix, no path scales with lag, occupancy, or history.

## RAM

All channel images remain in `.bss`. `Combined` is one LatestBuf channel plus
one private BlockBuilder; applications may place the builder on a task stack
or in static RAM, but the bytes exist either way.

| Width | N | Block bytes | LatestBuf channel | Builder | Combined |
|---:|---:|---:|---:|---:|---:|
| 2 | 8 | 24 | 108 | 28 | 136 |
| 2 | 32 | 72 | 252 | 76 | 328 |
| 2 | 128 | 264 | 828 | 268 | 1,096 |
| 8 | 8 | 72 | 264 | 80 | 344 |
| 8 | 32 | 264 | 840 | 272 | 1,112 |
| 8 | 128 | 1,032 | 3,144 | 1,040 | 4,184 |
| 16 | 8 | 136 | 456 | 144 | 600 |
| 16 | 32 | 520 | 1,608 | 528 | 2,136 |
| 16 | 128 | 2,056 | 6,216 | 2,064 | 8,280 |

For comparison, individual-sample LatestBuf channels are 48, 72, and 96 bytes
for the 2-, 8-, and 16-byte sample types.

## Code size across all targets

Each `P/T` sample entry is one `publish` / `take_latest` monomorph. Each `C/T`
block entry is final builder completion plus publication / `take_latest`.
Xtensa totals include `.text` and `.literal`.

### 2-byte samples

| Target | Sample P/T | N=8 C/T | N=32 C/T | N=128 C/T |
|---|---:|---:|---:|---:|
| thumbv6m | 78 / 98 | 192 / 110 | 210 / 110 | 250 / 124 |
| thumbv8m.base | 80 / 102 | 192 / 114 | 200 / 114 | 228 / 132 |
| thumbv7m | 76 / 90 | 188 / 124 | 182 / 96 | 188 / 102 |
| thumbv7em | 76 / 90 | 188 / 124 | 182 / 96 | 188 / 102 |
| thumbv8m.main | 76 / 88 | 180 / 120 | 178 / 86 | 184 / 92 |
| armv7r | 116 / 160 | 284 / 172 | 304 / 176 | 304 / 180 |
| armv7a | 116 / 160 | 284 / 172 | 304 / 176 | 304 / 180 |
| riscv32imac | 76 / 112 | 200 / 116 | 200 / 102 | 218 / 106 |
| ESP32 | 102 / 141 | 233 / 156 | 276 / 159 | 280 / 161 |
| ESP32-S2 | 80 / 116 | 215 / 131 | 254 / 134 | 258 / 136 |
| ESP32-S3 | 102 / 141 | 233 / 156 | 276 / 159 | 280 / 161 |

### 8-byte samples

| Target | Sample P/T | N=8 C/T | N=32 C/T | N=128 C/T |
|---|---:|---:|---:|---:|
| thumbv6m | 80 / 118 | 242 / 130 | 274 / 146 | 276 / 148 |
| thumbv8m.base | 82 / 118 | 238 / 134 | 274 / 154 | 280 / 156 |
| thumbv7m | 78 / 102 | 202 / 108 | 202 / 112 | 230 / 116 |
| thumbv7em | 78 / 102 | 202 / 108 | 202 / 112 | 230 / 116 |
| thumbv8m.main | 78 / 100 | 202 / 104 | 202 / 108 | 230 / 112 |
| armv7r | 128 / 176 | 336 / 196 | 328 / 196 | 312 / 196 |
| armv7a | 128 / 176 | 336 / 196 | 328 / 196 | 312 / 196 |
| riscv32imac | 80 / 120 | 246 / 132 | 264 / 136 | 258 / 142 |
| ESP32 | 106 / 154 | 287 / 171 | 291 / 175 | 346 / 214 |
| ESP32-S2 | 85 / 133 | 266 / 150 | 268 / 152 | 315 / 185 |
| ESP32-S3 | 106 / 154 | 287 / 171 | 291 / 175 | 346 / 214 |

### 16-byte samples

| Target | Sample P/T | N=8 C/T | N=32 C/T | N=128 C/T |
|---|---:|---:|---:|---:|
| thumbv6m | 98 / 134 | 262 / 142 | 272 / 146 | 276 / 148 |
| thumbv8m.base | 100 / 130 | 270 / 148 | 278 / 154 | 284 / 156 |
| thumbv7m | 114 / 120 | 214 / 110 | 216 / 112 | 260 / 144 |
| thumbv7em | 114 / 120 | 214 / 110 | 216 / 112 | 260 / 144 |
| thumbv8m.main | 102 / 118 | 210 / 106 | 212 / 108 | 252 / 134 |
| armv7r | 128 / 192 | 312 / 196 | 312 / 196 | 332 / 204 |
| armv7a | 128 / 192 | 312 / 196 | 312 / 196 | 332 / 204 |
| riscv32imac | 102 / 140 | 248 / 136 | 250 / 136 | 300 / 164 |
| ESP32 | 124 / 168 | 311 / 175 | 320 / 179 | 383 / 226 |
| ESP32-S2 | 103 / 147 | 286 / 152 | 299 / 160 | 354 / 197 |
| ESP32-S3 | 124 / 168 | 311 / 175 | 320 / 179 | 383 / 226 |

The constrained kill rows do not show a code-size outlier: thumbv6m complete
publication is 192-276 bytes and ESP32-S2 is 215-354 bytes across the grid.
As in the earlier matrices, non-monotonic rows come from shared copy routines
and inlining choices, which is why retired instructions and RAM are reported
separately.

## Result for D3 and the remaining decisions

The evidence supports the coupled lanes' existing D3 recommendation:
`LatestBuf<T>` remains payload-agnostic, and sample versus complete block is an
application release-scheduling and RAM choice. Nothing in the grid establishes
a distinct latest-block synchronization or accounting contract, so the
measurement does not justify a `LatestBlockBuf` primitive.

That is a recommendation for maintainer closure, not an API decision taken by
the probe. D1, D2, D3, and A.3 still require the recorded maintainer closure.
The BlockBuf P/S decision also remains budget-gated: this matrix supplies the
LatestBuf-side rows and explicit 136-8,280-byte combined RAM range, but it does
not invent the named target instruction/time budget or RAM envelope that #28
requires before choosing Copy composition or SlotPool.
