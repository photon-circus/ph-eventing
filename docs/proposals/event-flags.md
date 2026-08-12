# EventFlags: coalesced condition notification

- **Status:** **SHIPPED in 0.3.0** — decisions, frozen contract,
  implementation, and admission evidence complete; accepted 2026-08-12 and
  merged via PR #36. This document remains the design-decision record — what was decided,
  why, and what was rejected; the enduring engineering briefing lives in
  [`records/event-flags.md`](../records/event-flags.md).
- **Origin:** substantive design text moved from the
  [bounded-handoff taxonomy](exploratory-primitives.md) §4; triaged Tier 1 in
  [`../0.3.0-candidates.md`](../0.3.0-candidates.md) §4.
- **Contract:** [`event-flags-contract.md`](event-flags-contract.md) — stable
  M/R/T/C/S/B/H/W/X clause IDs and the evidence map.
- **Related:** [`counted-signal.md`](counted-signal.md) — same bounded
  ISR-notification niche, but retains multiplicity instead of coalescing it.

## 1. Accepted design

EventFlags is a fixed SPSC condition set for communication that is not an item
stream. An interrupt can raise conditions such as FIFO watermark, DMA complete,
peripheral error, configuration changed, or shutdown requested. Repeating one
condition before the task observes it adds no information.

```rust
const DATA_READY: EventMask = EventMask::from_bits(1 << 0);
const OVERFLOW: EventMask = EventMask::from_bits(1 << 1);

producer.raise(DATA_READY);
producer.raise(OVERFLOW);

let pending = consumer.take_all();
```

The representation and operations are deliberately small:

- one transparent `EventMask(u32)`, exactly 32 conditions;
- one `AtomicU32` pending word;
- Release `fetch_or` to raise;
- Acquire `swap(0)` to take;
- one sole-role `Send + !Sync` producer and consumer, with `&self` hot paths;
- no allocation, queue capacity, non-clearing peek, stream traits, or signal
  traits.

Duplicate conditions coalesce. A raise racing a take is observed in that take
or remains pending for the next one; the atomic modification order cannot lose
it between the OR and clear. Release/Acquire additionally publishes
application memory written before the raise.

## 2. Decisions closed

### 2.1 Handles

Shared decision H was accepted for both signal lanes on issue #30: sole-role
`Send + !Sync` handles with `&self` operations. EventFlags' atomic OR could
support multiple raisers, but promising that here would force a different and
weaker CountedSignal algorithm. The conservative shared contract loses no SPSC
use case and leaves an independently-evidenced MPSC type possible later.

### 2.2 Condition representation and width

The transparent mask wins over raw `u32`, an enum trait, and a macro-generated
namespace. It keeps domains distinct at the API boundary while compiling to
the same word operation. `EventMask::from_index` checks external indices
without a shift panic; applications can define named `const` masks without a
runtime mapping.

The candidate stays exactly `u32`. A generic word trait would make atomic
availability, fallback behaviour, monomorphization, and code size part of the
public contract. `u64` in particular would not mean one native operation on
the 32-bit MCUs this crate supports.

### 2.3 Observation and traits

A non-clearing peek is omitted. Its result would be immediately advisory: a
take may clear it and a raise may follow it. That surface invites
check-then-act code with no guarantee behind it.

EventFlags does not implement stream `Sink`/`Source`/`Link`; destructive take
plus a rejecting downstream sink could lose the mask, and coalescing cannot be
reported through the stream vocabulary. The initial `SignalSink<S>` /
`SignalSource<S>` sketch is also deferred. CountedSignal's `increment` and
count-plus-saturation snapshot prove that one shared generic `S` is not honest
for both primitives.

## 3. Implementation and ordering

The only condition-state operations are:

```text
raise(mask): pending.fetch_or(mask, Release)
take_all():  EventMask(pending.swap(0, Acquire))
```

Each atomic RMW is a linearization point. For a racing pair, atomic
modification order is either OR-before-swap or swap-before-OR, giving the
current or following take window respectively. No read/compute/write split
exists that could clear a later raise.

Release/Acquire is load-bearing for publication, not conservation. Loom's
publication litmus passes with the accepted pair and fails with the expected
stale payload when either operation is independently weakened to Relaxed. The
two partition models continue to pass under Relaxed, which is why they cannot
stand in for the publication test.

## 4. Admission evidence

### 4.1 Behaviour, Loom, and Miri

- 77 unit tests exercise empty/all masks, bit 31, checked index construction,
  duplicate and multi-bit raises, clear-on-take, role acquisition, static
  construction, threaded take races, and publication.
- Three EventFlags Loom models exhaustively cover one-raise and two-distinct-
  raise take partitions plus the Release/Acquire publication litmus.
- Producer and consumer compile-fail doctests pin `!Sync`; ordinary type tests
  pin `Send`, container `Sync`, and the transparent four-byte mask.
- EventFlags contains no unsafe slot access and passes the normal detector-on
  Miri path, including the repository's 32-bit proxy target.

### 4.2 Code size

The committed gate measures acquisition plus the two hot functions under
rustc 1.92.0 (`ded5c06cf`). Take is branch-free, so one row is both its empty
and non-empty code size.

| Target | acquire roles | raise | take |
|---|---:|---:|---:|
| thumbv6m | 68 | 24 | 24 |
| thumbv8m.base | 64 | 22 | 22 |
| thumbv7m | 72 | 26 | 26 |
| thumbv7em | 72 | 26 | 26 |
| thumbv8m.main | 64 | 22 | 22 |
| armv7r | 116 | 32 | 32 |
| armv7a | 116 | 32 | 32 |
| riscv32imac | 66 | 8 | 8 |
| ESP32 (opt-in) | 145 | 37 | 36 |
| ESP32-S2 (opt-in) | 68 | 23 | 22 |
| ESP32-S3 (opt-in) | 145 | 37 | 36 |

`EventFlags` itself is 8 bytes on the host and gated targets: one `AtomicU32`
pending word plus two packed `AtomicBool` role claims (`size_of` is pinned by
unit test). The codesize probe's `bss` row still measures only the existing
`EventBuf` static (`.bss.*BUF`); it does not report the EventFlags object size.
The candidate adds no `.data`, runtime dependency, allocation, or panic string
to either hot path.

### 4.3 Cortex-M3 instruction counts

Measured by `./scripts/verify.sh cycles` in the pinned reference image: rustc
1.92.0, QEMU 10.0.11, Cortex-M3.

| Operation | State | Retired instructions |
|---|---|---:|
| `raise` | condition clear | 12 |
| `raise` | condition already set | 12 |
| `take_all` | non-empty | 10 |
| `take_all` | empty | 10 |

The equal state pairs pin the intended constant hot path: neither occupancy
nor number of set bits changes the executed work. Rows re-measured on the
assembled 0.3.0 release branch; the merged probe binary carries `take_all`
two instructions higher than the per-lane tree's 8 (codegen context, not an
algorithm change), while both `raise` rows are unchanged.

### 4.4 Portable-atomic interrupt window

`scripts/event-flags-atomic-window.sh` compiles and disassembles the committed
probe. Counts are instructions after the architectural disable instruction
through restore/synchronization.

| Target | raise | take | Gating | Generated path |
|---|---:|---:|---|---|
| thumbv6m | 4 | 4 | Always (`./scripts/verify.sh atomic-window`) | PRIMASK: load, update/store, restore; straight-line |
| ESP32-S2 | 5 | 5 | Opt-in (`ESP=1`, needs esp-rs) | PS.INTLEVEL=15: load, update/store, `wsr.ps`, `rsync`; straight-line |
| ESP32-S3 | 0 | 0 | Opt-in (`ESP=1`, needs esp-rs) | Native `s32c1i`; the esp-rs target advertises 32-bit atomics and does not mask interrupts |

The Docker verify matrix gates thumbv6m only — the reference image does not
ship esp-rs, and a SKIP is not a pass. ESP rows are measured on hosts with the
fork installed (`ESP=1`), the same present-tooling pattern as `XTENSA=1`
codesize. The thumbv6m and S2 paths each contain exactly one interrupt-masked
RMW and no branch or hidden compare-exchange loop. The S3 result corrects the
exploratory assumption that S2 and S3 share a fallback: under esp-rs
1.95.0-nightly, S3 is a native-atomic target, so its maximum interrupt-disabled
window is zero.

## 5. Promotion result

All four promotion-bar items are closed:

1. shared H and EventFlags' transparent-mask decision recorded;
2. 32 conditions, no peek, disjoint stream traits, and deferred signal traits
   explicitly confirmed;
3. accepted clauses frozen in `event-flags-contract.md`;
4. unit/threaded, Loom mutation, Miri, code-size, Cortex-M3, and portable-
   atomic window evidence implemented.

The lane is therefore **PROPOSED** and ready for candidate evaluation. This is
not acceptance into the release: evaluation may still reject the primitive on
API fit or measured cost, with the evidence retained either way.
