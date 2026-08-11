# SlotPool target-cost measurements

- **Measured:** 2026-08-11
- **Compiler:** `rustc 1.92.0 (ded5c06cf 2025-12-08)`
- **Cycle reference:** `QEMU emulator version 10.0.11`, from the repository's
  `ph-eventing-verify` image
- **Shape:** `SlotPool<u32, 1>`; initialized and first-initialization paths

This is the target-cost evidence required by
[`slot-pool.md`](slot-pool.md). Capacity one deliberately isolates the
state-machine operations: reservation and claim contain no capacity-dependent
loop or scan, so a larger `N` changes inline RAM but not the algorithmic path.

## Reference instruction counts

Reproduce in the pinned environment with:

```sh
./scripts/verify.sh cycles slot-pool
```

| State-machine path | Retired instructions |
|---|---:|
| reserve available initialized slot | 14 |
| commit initialized slot | 4 |
| reserve while full | 13 |
| claim while empty | 14 |
| claim available slot | 16 |
| release claim | 3 |
| initialize vacant slot and commit | 13 |

The empty/full and available paths are separate runtime regions. Their costs
are fixed and contain no retry loop. Grant cancellation itself performs no
atomic operation; its cost is represented by leaving the reservation scope
after the 14-instruction available-reserve path.

## Code size across all gated targets

Reproduce with:

```sh
./scripts/codesize.sh slot-pool
```

Each cell is emitted flash bytes for one caller-visible API shape:

- `reserve_B`: reserve, distinguish initialized/vacant/full, then cancel;
- `commit_B`: reserve an initialized slot, mutate `u32`, and commit;
- `init_B`: reserve a vacant slot, `write(u32)`, and commit; and
- `claim_B`: distinguish available/empty, read, and release.

| Target | `reserve_B` | `commit_B` | `init_B` | `claim_B` |
|---|---:|---:|---:|---:|
| `thumbv6m-none-eabi` | 38 | 44 | 48 | 42 |
| `thumbv8m.base-none-eabi` | 36 | 40 | 44 | 40 |
| `thumbv7m-none-eabi` | 36 | 34 | 36 | 46 |
| `thumbv7em-none-eabi` | 36 | 34 | 36 | 46 |
| `thumbv8m.main-none-eabi` | 36 | 32 | 34 | 44 |
| `armv7r-none-eabi` | 56 | 72 | 84 | 72 |
| `armv7a-none-eabi` | 56 | 72 | 84 | 72 |
| `riscv32imac-unknown-none-elf` | 30 | 38 | 40 | 44 |

The first-initialization witness adds 2–12 bytes over initialized
reserve/mutate/commit in these shapes. The largest measured complete shape is
84 bytes on ARMv7-R/A; the no-native-atomic Cortex-M0/M23 rows remain 48 bytes
or less with the existing single-core portable-atomic backend.

## RAM and boundedness

The representation is:

```text
N * size_of::<T>() + head + tail + initialized-prefix count
                    + producer/consumer taken flags + alignment
```

Initialization tracking is one pool-wide `u32`, not a per-slot bitmap. FIFO
commit order means initialized slots form a prefix until every slot has been
constructed; after that all slots remain initialized until pool teardown.

The measurements establish the missing cost evidence but do not decide issue
#26's deferred **P** budget or **S** admission path. They make that eventual
reading concrete without inventing an application-wide instruction or RAM
threshold.
