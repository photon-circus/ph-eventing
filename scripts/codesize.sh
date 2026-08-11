#!/usr/bin/env sh
# Code-size measurements for ph-eventing across every embedded target it claims
# to support.
#
# This exists because a design that wins on one MCU can lose badly on another,
# and measuring a single target hides that completely:
#
#   - Cortex-M0+ and Cortex-M23 have no native 32-bit atomics. Under
#     portable-atomic every read-modify-write becomes an interrupt-disable
#     critical section, so an extra RMW costs both flash and interrupt latency.
#   - RISC-V has single-instruction AMOs (`amoor.w`, `amoswap.w`) but compiles
#     `compare_exchange` to an LR/SC retry loop -- the opposite cost ordering
#     from ARM, where both are ldrex/strex.
#   - Xtensa splits code between `.text.<fn>` and `.literal.<fn>`. Counting only
#     `.text` undercounts it (212 vs 220 bytes for the same function on esp32).
#   - ESP32-S2/S3 are single-core Xtensa without the S32C1I compare-and-swap
#     instruction, so they need portable-atomic exactly like Cortex-M0+.
#
# Tooling is whatever rust-toolchain.toml pins. `llvm-size` ships with the
# `llvm-tools` component, which that file already declares, so there is nothing
# extra to install -- no cargo-binutils, no arm-none-eabi-gcc, no system
# binutils.
#
# Xtensa is the one exception and is opt-in: upstream rustc has no Xtensa
# backend, so those rows need the esp-rs fork plus `-Zbuild-std=core` because no
# precompiled core ships for them. They skip cleanly when the fork is absent.
#
# Usage:
#   ./scripts/codesize.sh                  # baseline API, upstream targets
#   ./scripts/codesize.sh split            # also measure try_split, where present
#   XTENSA=1 ./scripts/codesize.sh         # add ESP32 rows (needs esp-rs fork)
#
# Reading the output: each number is the byte size of one function, so it
# attributes to a single API shape rather than to a whole binary. `bss` is the
# `static EventBuf<u32, 64>`; it should be identical on every target and `data`
# should be 0 -- that pair is the const-`new` claim.

set -u

cd "$(dirname "$0")/.." || exit 1

# See scripts/ci.sh: incremental compilation makes Windows runs flaky with
# "Access is denied (os error 5)".
CARGO_INCREMENTAL=0
export CARGO_INCREMENTAL

PROBE_FEATURES=""
[ "${1:-}" = "split" ] && PROBE_FEATURES="split"

# llvm-size is not on PATH. Resolve it from the pinned toolchain's sysroot
# rather than hardcoding a path or assuming a channel -- the point is that
# anyone running this measures with exactly what rust-toolchain.toml pins.
SYSROOT="$(rustc --print sysroot)" || exit 1
HOST="$(rustc -vV | sed -n 's/^host: //p')"
SIZE="$SYSROOT/lib/rustlib/$HOST/bin/llvm-size"
[ -x "$SIZE" ] || SIZE="${SIZE}.exe"
if [ ! -x "$SIZE" ]; then
    printf 'error: llvm-size not found at %s\n' "$SIZE" >&2
    printf 'Install it with:  rustup component add llvm-tools\n' >&2
    exit 1
fi

# Fields: target|probe-features|toolchain|extra-cargo-args
# No spaces inside an entry -- the loop word-splits on whitespace.
TARGETS='
thumbv6m-none-eabi|cm0||
thumbv8m.base-none-eabi|cm0||
thumbv7m-none-eabi|||
thumbv7em-none-eabi|||
thumbv8m.main-none-eabi|||
armv7r-none-eabi|||
armv7a-none-eabi|||
riscv32imac-unknown-none-elf|||
'

XTENSA_TARGETS='
xtensa-esp32-none-elf||+esp|-Zbuild-std=core
xtensa-esp32s2-none-elf|cm0|+esp|-Zbuild-std=core
xtensa-esp32s3-none-elf|cm0|+esp|-Zbuild-std=core
'

[ "${XTENSA:-0}" = "1" ] && TARGETS="${TARGETS}${XTENSA_TARGETS}"

# Sum `.text.<fn>` and `.literal.<fn>`. The second is Xtensa-only and a no-op
# everywhere else, but omitting it silently undercounts ESP32.
fn_size() {
    "$SIZE" -A "$1" 2>/dev/null | awk -v f="$2" '
        $1 == ".text." f || $1 == ".literal." f { total += $2; found = 1 }
        END { if (found) print total }'
}

printf '%-30s %10s %8s %8s %6s\n' TARGET two_calls split bss data
printf '%-30s %10s %8s %8s %6s\n' '------------------------------' '---------' '-----' '---' '----'

skipped=0
failed=0

for entry in $TARGETS; do
    target="$(printf '%s' "$entry" | cut -d'|' -f1)"
    feat_extra="$(printf '%s' "$entry" | cut -d'|' -f2)"
    toolchain="$(printf '%s' "$entry" | cut -d'|' -f3)"
    extra_args="$(printf '%s' "$entry" | cut -d'|' -f4)"

    if [ -n "$toolchain" ]; then
        # A custom toolchain is not listed by `rustup target list`; ask the
        # compiler itself whether it knows the target.
        if ! rustc "$toolchain" --print target-list 2>/dev/null | grep -qx "$target"; then
            printf '%-30s %s\n' "$target" 'SKIP (needs the esp-rs toolchain)'
            skipped=$((skipped + 1))
            continue
        fi
    elif ! rustup target list --installed 2>/dev/null | grep -qx "$target"; then
        printf '%-30s %s\n' "$target" "SKIP (rustup target add $target)"
        skipped=$((skipped + 1))
        continue
    fi

    feats="$PROBE_FEATURES"
    [ -n "$feat_extra" ] && feats="$feats $feat_extra"
    feats="$(printf '%s' "$feats" | sed 's/^ *//;s/ *$//;s/  */,/g;s/ /,/g')"

    set -- build --release --target "$target" \
        --manifest-path scripts/codesize/Cargo.toml
    [ -n "$feats" ] && set -- "$@" --features "$feats"
    [ -n "$extra_args" ] && set -- "$@" "$extra_args"

    if [ -n "$toolchain" ]; then
        set -- "$toolchain" "$@"
    fi

    if ! cargo "$@" >/dev/null 2>&1; then
        printf '%-30s %s\n' "$target" 'BUILD FAILED'
        failed=$((failed + 1))
        continue
    fi

    ar="scripts/codesize/target/$target/release/libph_eventing_codesize.a"
    two="$(fn_size "$ar" bringup_two_calls)"
    spl="$(fn_size "$ar" bringup_split)"
    bss="$("$SIZE" -A "$ar" 2>/dev/null | awk '$1 ~ /^\.bss\..*3BUF/ { print $2; exit }')"
    dat="$("$SIZE" -A "$ar" 2>/dev/null | awk '$1 ~ /^\.data\./ { s += $2 } END { print s + 0 }')"

    printf '%-30s %10s %8s %8s %6s\n' \
        "$target" "${two:--}" "${spl:--}" "${bss:--}" "${dat:-0}"

    # An empty measurement means llvm-size could not read the archive, or a
    # toolchain change renamed a section. stderr is suppressed and the awk still
    # succeeds, so without this the row prints "-" and the run reports success --
    # an incomplete measurement treated as a valid one.
    if [ -z "$two" ] || [ -z "$bss" ]; then
        printf '%-30s %s\n' "" 'ERROR: required section missing (two_calls/bss)'
        failed=$((failed + 1))
        continue
    fi
done

printf '\n'
if [ "$skipped" -gt 0 ]; then
    printf '%s target(s) skipped. A partial matrix hides exactly the\n' "$skipped"
    printf 'per-architecture differences this script exists to find.\n\n'
fi
[ "$failed" -gt 0 ] && printf '%s target(s) failed to build.\n\n' "$failed"
printf 'split column is "-" unless run as: ./scripts/codesize.sh split\n'
printf 'Xtensa rows need: XTENSA=1 and the esp-rs toolchain.\n'

[ "$failed" -gt 0 ] && exit 1
exit 0
