# LatestBuf target, payload, and empty-poll measurements

- **Measured:** 2026-08-11
- **Source:** `candidate/latest-buf` from `72cb22f`, with the isolated
  measurement harness and optimizations recorded with this document.
- **Compiler:** `rustc 1.92.0 (ded5c06cf 2025-12-08)`.
- **Cycle reference:** `QEMU emulator version 10.0.11`, from the repository's
  `ph-eventing-verify` image.
- **Payloads:** `u32`, 16 bytes (`[u32; 4]`), and a representative 128-byte
  complete payload (`[u32; 32]`).

This is the cost evidence requested by issue #27 and
[latest-buf-evaluation.md](latest-buf-evaluation.md) section 6. The 128-byte
array measures the transport's complete-payload scaling without importing the
BlockBuf candidate. The subsequent joint `Block<T, N>` campaign is recorded in
[`latest-block-composition-measurements.md`](latest-block-composition-measurements.md).

Reproduce the final implementation with:

```sh
XTENSA=1 ./scripts/codesize.sh latest-matrix
./scripts/verify.sh cycles latest-matrix
```

## Reference instruction counts

| Payload | Publish first | Publish replacement | Take pending | Take empty |
|---|---:|---:|---:|---:|
| `u32` | 41 | 40 | 46 | 15 |
| 16 bytes | 70 | 70 | 51 | 17 |
| 128 bytes | 482 | 483 | 185 | 18 |

Channel-resident stateless role handles cost 5 instructions to drop and 15
to reacquire for the producer; the consumer costs 5 and 14 respectively.
First and replacement publication differ by at most one instruction. Payload
copy cost scales with `size_of::<T>()`; neither path depends on lag, occupancy,
or history.

## Code size across the target and payload matrix

Each `P/T` entry is emitted flash bytes for one `publish` / `take_latest`
monomorph. The final `roles P/C` columns measure producer / consumer
claim-and-release functions. Xtensa totals include both `.text` and `.literal`.

| Target | `u32` P/T | 16 B P/T | 128 B P/T | roles P/C |
|---|---:|---:|---:|---:|
| thumbv6m | 76 / 96 | 98 / 114 | 94 / 118 | 42 / 42 |
| thumbv8m.base | 78 / 100 | 100 / 116 | 96 / 120 | 34 / 34 |
| thumbv7m | 88 / 102 | 116 / 122 | 112 / 134 | 46 / 46 |
| thumbv7em | 88 / 102 | 116 / 122 | 112 / 134 | 46 / 46 |
| thumbv8m.main | 80 / 102 | 108 / 122 | 102 / 124 | 36 / 36 |
| armv7r | 112 / 152 | 124 / 168 | 128 / 172 | 64 / 64 |
| armv7a | 112 / 152 | 124 / 168 | 128 / 172 | 64 / 64 |
| riscv32imac | 72 / 106 | 100 / 130 | 124 / 158 | 34 / 48 |
| ESP32 | 101 / 139 | 119 / 155 | 125 / 161 | 76 / 104 |
| ESP32-S2 | 79 / 114 | 97 / 130 | 103 / 136 | 47 / 47 |
| ESP32-S3 | 101 / 139 | 119 / 155 | 125 / 161 | 76 / 104 |

The non-monotonic payload rows are normal optimized codegen: larger copies
may lower to shared copy routines instead of more inline instructions. That is
why emitted flash and retired instructions are both recorded.

## A.1: empty-poll Acquire-load fast path

The reference prototype swapped unconditionally. The selected implementation
first Acquire-loads the ready bit; a false load returns without transferring a
slot, while a true load still performs the same `AcqRel` ownership exchange.
The focused Loom model explores publication before, during, and after the
load. If the load returns false, the publication remains pending for the next
poll.

Isolated Cortex-M3 comparison, before the role-index layout optimization:

| Payload | Pending, swap -> load+swap | Empty, swap -> load |
|---|---:|---:|
| `u32` | 36 -> 42 (+6) | 22 -> 15 (-7) |
| 16 bytes | 41 -> 47 (+6) | 24 -> 17 (-7) |
| 128 bytes | 178 -> 183 (+5) | 26 -> 18 (-8) |

The isolated `take_latest` flash deltas stayed inside the existing +16-byte
gate tolerance on every target and payload:

| Target family | `u32` | 16 B | 128 B |
|---|---:|---:|---:|
| thumbv6m | +8 | +14 | +8 |
| thumbv8m.base | +6 | +10 | +6 |
| thumbv7m / thumbv7em | +10 | +10 | +10 |
| thumbv8m.main | +8 | +8 | +8 |
| armv7r / armv7a | +16 | +16 | +16 |
| riscv32imac | +10 | +10 | +10 |
| ESP32 / ESP32-S3 | +11 | +11 | +11 |
| ESP32-S2 | +8 | +8 | +8 |

The Cortex-M3 count understates the named hazard-row benefit. On thumbv6m and
ESP32-S2 the eliminated empty `swap` is a portable-atomic critical section, so
the fast path removes interrupt-disable latency from every idle poll. The
pending path pays one bounded load and branch. A.1 therefore closes in favour
of the Acquire-load fast path.

## Static layout and role-index encoding

The first matrix run found that literal initial roles (`back = 1`, `front = 2`)
placed the entire const-initialized channel in `.data`. That charged flash and
startup copy for all three payload slots. The final implementation XOR-encodes
the private role indices so the initial representation is all zero:

| Payload | Channel RAM | Before | Final | Flash/startup-copy removed |
|---|---:|---|---|---:|
| `u32` | 48 B | `.data` | `.bss` | 48 B |
| 16 bytes | 84 B | `.data` | `.bss` | 84 B |
| 128 bytes | 420 B | `.data` | `.bss` | 420 B |

On every shipped target a producer or consumer handle is one pointer (4 B);
the marker is zero-sized. The channel is three `Entry<T>` slots plus 24 bytes
of exchange, role, generation, and acquisition state for these 4-byte-aligned
payloads.

Encoding adds at most 10 bytes to either measured operation across the matrix.
On the reference Cortex-M3 it adds 1-2 instructions to publication, 2-4 to a
pending take, and zero to an empty take. That bounded hot-path cost removes a
payload-proportional flash and startup-copy cost, so the encoded `.bss` layout
is retained.

## Result and follow-on

The soundness-stage implementation now has the requested target/payload code
size, pinned instruction regions, A.1 comparison, and explicit handle/channel
RAM record. Neither constrained kill row shows a code-size outlier, and idle
polling no longer enters a portable-atomic critical section.

The joint sample-versus-`Block<T, N>` composition follow-on is now complete in
the linked record. Development gates 1-3 are therefore complete; maintainer
closure of D1-D3 and A.3 remains.
