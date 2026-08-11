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
# attributes to a single API shape rather than to a whole binary. `cs_incr` and
# `cs_take` isolate the two CountedSignal hot paths. `bss` is the `static
# EventBuf<u32, 64>`; it should be identical on every target and `data` should
# be 0 -- that pair is the const-`new` claim.

set -u

cd "$(dirname "$0")/.." || exit 1

# See scripts/ci.sh: incremental compilation makes Windows runs flaky with
# "Access is denied (os error 5)".
CARGO_INCREMENTAL=0
export CARGO_INCREMENTAL

PROBE_FEATURES=""
BLESS=0
for arg in "$@"; do
    case "$arg" in
        split)   PROBE_FEATURES="split" ;;
        --bless) BLESS=1 ;;
        # Not 2: that is reserved for "could not run", which ci.sh maps to SKIP.
        *) printf 'unknown argument: %s
' "$arg" >&2; exit 64 ;;
    esac
done

BASELINE="scripts/codesize/baseline.tsv"
# Growth beyond this many bytes on any row fails the gate. Absolute, not a
# percentage: at 100-200 bytes a percentage is noise, and the regression this
# exists to catch was +60 on Cortex-M0+ and +78 on ESP32-S2. Shrinkage never
# fails -- it is reported and prompts a re-bless.
TOLERANCE=16

# The baseline is only meaningful for the toolchain that produced it, which is
# why rust-toolchain.toml pinning 1.92.0 makes this workable at all. Verified
# byte-identical on x86_64-pc-windows-msvc and x86_64-unknown-linux-gnu for the
# same pinned rustc, so the baseline is host-independent and can be committed.
RUSTC_ID="$(rustc -vV | sed -n 's/^commit-hash: //p')"

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

RESULTS="$(mktemp)"
trap 'rm -f "$RESULTS"' EXIT

printf '%-30s %10s %8s %8s %8s %8s %6s\n' TARGET two_calls cs_incr cs_take split bss data
printf '%-30s %10s %8s %8s %8s %8s %6s\n' '------------------------------' '---------' '-------' '-------' '-----' '---' '----'

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
    cs_inc="$(fn_size "$ar" counted_increment)"
    cs_take="$(fn_size "$ar" counted_take)"
    spl="$(fn_size "$ar" bringup_split)"
    bss="$("$SIZE" -A "$ar" 2>/dev/null | awk '$1 ~ /^\.bss\..*3BUF/ { print $2; exit }')"
    dat="$("$SIZE" -A "$ar" 2>/dev/null | awk '$1 ~ /^\.data\./ { s += $2 } END { print s + 0 }')"

    printf '%-30s %10s %8s %8s %8s %8s %6s\n' \
        "$target" "${two:--}" "${cs_inc:--}" "${cs_take:--}" "${spl:--}" "${bss:--}" "${dat:-0}"

    # Xtensa is never gated: it needs a toolchain fork, so making it a hard gate
    # would make that fork mandatory for every contributor.
    case "$target" in xtensa-*) continue ;; esac
    [ -n "$two" ] && printf '%s\ttwo_calls\t%s\n' "$target" "$two" >> "$RESULTS"
    [ -n "$cs_inc" ] && printf '%s\tcounted_increment\t%s\n' "$target" "$cs_inc" >> "$RESULTS"
    [ -n "$cs_take" ] && printf '%s\tcounted_take\t%s\n' "$target" "$cs_take" >> "$RESULTS"
    [ -n "$bss" ] && printf '%s\tbss\t%s\n' "$target" "$bss" >> "$RESULTS"
    printf '%s\tdata\t%s\n' "$target" "${dat:-0}" >> "$RESULTS"
done

printf '\n'
if [ "$skipped" -gt 0 ]; then
    printf '%s target(s) skipped. A partial matrix hides exactly the\n' "$skipped"
    printf 'per-architecture differences this script exists to find.\n\n'
fi
[ "$failed" -gt 0 ] && printf '%s target(s) failed to build.\n\n' "$failed"
# ---------------------------------------------------------------------------
# ---------------------------------------------------------------------------
# Baseline gate
#
# Exit codes: 0 pass, 1 fail, 2 could-not-run. ci.sh maps 2 to SKIP -- and a
# SKIP is not a pass, so it must never be reported as one.
# ---------------------------------------------------------------------------

# A partial run must never become a baseline: blessing after a skipped or failed
# target silently deletes those rows, and the gate then cannot regress on them
# because it no longer knows they exist.
if [ "$BLESS" = "1" ]; then
    if [ "$failed" -gt 0 ] || [ "$skipped" -gt 0 ]; then
        printf 'refusing to bless: %s target(s) failed, %s skipped.
'             "$failed" "$skipped" >&2
        printf 'A baseline written from a partial run drops those rows, and a
' >&2
        printf 'dropped row cannot regress. Install the missing targets first.
' >&2
        exit 1
    fi
    {
        printf '# ph-eventing code-size baseline. Regenerate: ./scripts/codesize.sh --bless
'
        printf '# rustc-commit: %s
' "$RUSTC_ID"
        printf '#
'
        printf '# Host-independent: byte-identical on x86_64-pc-windows-msvc and
'
        printf '# x86_64-unknown-linux-gnu for the same pinned rustc, verified across all
'
        printf '# eight gated targets. That is what makes committing it sound.
'
        printf '#
'
        printf '# Xtensa is deliberately absent -- it needs the esp-rs fork, and gating it
'
        printf '# would make that fork mandatory for every contributor.
'
        sort "$RESULTS"
    } > "$BASELINE"
    printf 'Wrote %s for rustc %s
' "$BASELINE" "$RUSTC_ID"
    exit 0
fi

if [ "$failed" -gt 0 ]; then
    printf '
%s target(s) failed to build; not comparing against the baseline.
' "$failed"
    exit 1
fi

if [ ! -f "$BASELINE" ]; then
    printf '
SKIP baseline gate: %s does not exist yet.
' "$BASELINE"
    printf 'Create it with: ./scripts/codesize.sh --bless
'
    exit 2
fi

# tr -d '\r': .gitattributes pins this file to LF, but a checkout that predates
# that pin (or an overriding local config) can still hand us CRLF -- and a \r
# in the commit hash makes two identical rustc ids compare unequal, which
# surfaced as the gate skipping inside the Linux reference image against a
# Windows checkout. Belt and braces with the attribute.
BASE_ID="$(sed -n 's/^# rustc-commit: //p' "$BASELINE" | tr -d '\r')"
if [ "$BASE_ID" != "$RUSTC_ID" ]; then
    printf '
SKIP baseline gate: baseline was recorded with rustc %s, this is %s.
'         "$BASE_ID" "$RUSTC_ID"
    printf 'Codegen differs between compilers, so comparing them would be noise, not
'
    printf 'signal. Re-bless deliberately after reviewing the diff:
'
    printf '  ./scripts/codesize.sh --bless
'
    exit 2
fi

printf '
Baseline gate (tolerance +%s bytes, growth only)
' "$TOLERANCE"

# The comparison runs in a subshell (pipeline), so counts travel back via files
# next to $RESULTS -- which is a mktemp path, not the repo.
BAD="$RESULTS.bad"
MISSING="$RESULTS.missing"
rm -f "$BAD" "$MISSING"

grep -v '^#' "$BASELINE" | tr -d '\r' | while IFS="$(printf '	')" read -r bt bm bv; do
    [ -z "$bt" ] && continue
    cur="$(awk -F'	' -v t="$bt" -v m="$bm" '$1==t && $2==m {print $3; exit}' "$RESULTS")"
    if [ -z "$cur" ]; then
        printf '  MISSING    %-28s %-10s (baseline %s)
' "$bt" "$bm" "$bv"
        echo x >> "$MISSING"
        continue
    fi
    delta=$((cur - bv))
    if [ "$delta" -gt "$TOLERANCE" ]; then
        printf '  REGRESSION %-28s %-10s %s -> %s (+%s)
' "$bt" "$bm" "$bv" "$cur" "$delta"
        echo x >> "$BAD"
    elif [ "$delta" -lt 0 ]; then
        printf '  improved   %-28s %-10s %s -> %s (%s) -- re-bless to lock it in
'             "$bt" "$bm" "$bv" "$cur" "$delta"
    fi
done

regressions=0
missing=0
[ -f "$BAD" ] && regressions=$(wc -l < "$BAD")
[ -f "$MISSING" ] && missing=$(wc -l < "$MISSING")
rm -f "$BAD" "$MISSING"

if [ "${missing:-0}" -gt 0 ]; then
    printf '
%s baseline row(s) had no measurement.
' "$missing"
    printf 'A row that is not measured cannot regress, so this is a hole in the
'
    printf 'gate rather than a pass. Install the missing targets and re-run.
'
    exit 1
fi

if [ "${regressions:-0}" -gt 0 ]; then
    printf '
%s row(s) grew by more than %s bytes.
' "$regressions" "$TOLERANCE"
    printf 'If the growth is intended, review it and run: ./scripts/codesize.sh --bless
'
    exit 1
fi
printf '  ok -- no row grew by more than %s bytes\n' "$TOLERANCE"

printf 'split column is "-" unless run as: ./scripts/codesize.sh split\n'
printf 'Xtensa rows need: XTENSA=1 and the esp-rs toolchain.\n'

[ "$failed" -gt 0 ] && exit 1
exit 0
