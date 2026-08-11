#!/usr/bin/env sh
# Miri (UB + concurrency) checks for ph-eventing.
#
# Runs the test suite under Miri, which interprets MIR and models the C++20
# weak-memory semantics. It catches undefined behaviour, data races, and
# ordering bugs that a native run on strongly-ordered x86 will never expose.
#
# Two passes are required, because SeqRing's seqlock has a known and
# unavoidable formal data race (see the SeqRing module docs):
#
#   1. Full checking, skipping the SeqRing overwrite stress test.
#   2. The SeqRing overwrite test alone, with the data-race detector off, so
#      its logic (no stale or torn payload, exact drop accounting) is still
#      verified under Miri's scheduler.
#
# Cross-target passes catch pointer-width and endianness bugs. The bare-metal
# targets cannot be run: Miri needs `std` for the test harness, and no_std
# targets have none. The 32-bit std targets stand in for them -- they share the
# pointer width and `usize` range that actually matter here.
#
# Usage:
#   ./scripts/miri.sh                # 16 seeds, host + cross-targets
#   SEEDS=64 ./scripts/miri.sh       # deeper schedule exploration
#   HOST_ONLY=1 ./scripts/miri.sh    # skip the cross-target passes
#   MIRI_TOOLCHAIN=nightly-2026-08-08 ./scripts/miri.sh   # pin a nightly
#
# Unlike everything else in scripts/, Miri cannot come from the toolchain
# rust-toolchain.toml pins -- it needs nightly, and a bare `+nightly` drifts
# daily. A verdict is only reproducible alongside the version that produced
# it, so the run prints its exact toolchain below and MIRI_TOOLCHAIN lets a
# reader replay under that same one. Record the printed version whenever a
# Miri result is cited as release evidence.

set -u

cd "$(dirname "$0")/.." || exit 1

# See scripts/ci.sh: incremental compilation makes Windows runs flaky with
# "Access is denied (os error 5)". There is no PowerShell twin of this script;
# on Windows run it under Git Bash.
CARGO_INCREMENTAL=0
export CARGO_INCREMENTAL

MIRI_TOOLCHAIN="${MIRI_TOOLCHAIN:-nightly}"
printf '==> miri toolchain: %s\n' "$(rustc "+$MIRI_TOOLCHAIN" --version)" || exit 1

SEEDS="${SEEDS:-16}"
BASE='-Zmiri-disable-isolation'
SEQLOCK_TEST='concurrent_overwrite_never_yields_a_mismatched_value'

# Proxies for the bare-metal targets: 32-bit pointers (i686, armv7) and
# big-endian (s390x). thumbv*/riscv32 themselves cannot host a test harness.
CROSS_TARGETS='i686-unknown-linux-gnu armv7-unknown-linux-gnueabihf s390x-unknown-linux-gnu'

failed=0
summary=""

run_miri() {
    name="$1"
    flags="$2"
    shift 2

    printf '\n==> %s\n' "$name"

    if MIRIFLAGS="$flags" cargo "+$MIRI_TOOLCHAIN" miri test "$@"; then
        summary="${summary}  PASS  ${name}\n"
    else
        summary="${summary}  FAIL  ${name}\n"
        failed=$((failed + 1))
    fi
}

run_miri 'host: full checking' "$BASE" -- --skip "$SEQLOCK_TEST"

run_miri 'host: seqlock logic (race detector off)' \
    "$BASE -Zmiri-disable-data-race-detector" "$SEQLOCK_TEST"

run_miri "host: $SEEDS scheduler seeds" \
    "$BASE -Zmiri-many-seeds=0..$SEEDS" -- concurrent_spsc len_stays

run_miri "host: SlotPool initialization, $SEEDS scheduler seeds" \
    "$BASE -Zmiri-many-seeds=0..$SEEDS" -- \
    slot_pool::tests::vacant_slots_require_write_before_publication

if [ "${HOST_ONLY:-0}" = "0" ]; then
    for target in $CROSS_TARGETS; do
        run_miri "$target" "$BASE" --target "$target" -- --skip "$SEQLOCK_TEST"
    done
fi

printf '\nSummary\n'
printf '%b' "$summary"

if [ "$failed" -gt 0 ]; then
    printf '\n%s Miri pass(es) failed.\n' "$failed"
    exit 1
fi

printf '\nAll Miri passes clean.\n'
