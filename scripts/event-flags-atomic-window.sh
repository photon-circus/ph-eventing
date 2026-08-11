#!/usr/bin/env sh
# Measure EventFlags' interrupt-disabled window on the targets that carry its
# ISR-latency admission case.
#
# This complements cycles.sh. QEMU's Cortex-M3 is useful for whole-operation
# instruction counts, but it cannot exercise the portable-atomic fallback used
# by Cortex-M0 and ESP32-S2. This script builds the committed code-size probe,
# extracts its two EventFlags hot-path functions, and checks the exact masked
# instruction sequences in their disassembly.
#
# ESP32-S3 is deliberately included because the exploratory proposal grouped
# it with S2. The esp-rs target currently advertises native 32-bit atomics and
# emits S32C1I, so its measured interrupt-disabled window is zero. The check
# pins that compiler fact rather than repeating the old assumption.

set -u

cd "$(dirname "$0")/.." || exit 1

CARGO_INCREMENTAL=0
export CARGO_INCREMENTAL

host="$(rustc -vV | sed -n 's/^host: //p')"
sysroot="$(rustc --print sysroot)" || exit 1
llvm_ar="$sysroot/lib/rustlib/$host/bin/llvm-ar"
llvm_objdump="$sysroot/lib/rustlib/$host/bin/llvm-objdump"
[ -x "$llvm_ar" ] || llvm_ar="${llvm_ar}.exe"
[ -x "$llvm_objdump" ] || llvm_objdump="${llvm_objdump}.exe"

if [ ! -x "$llvm_ar" ] || [ ! -x "$llvm_objdump" ]; then
    printf 'error: llvm-tools missing (rustup component add llvm-tools)\n' >&2
    exit 1
fi

if ! rustc +esp --print target-list 2>/dev/null | grep -qx xtensa-esp32s2-none-elf; then
    printf 'SKIP: the esp-rs toolchain is not installed (install with espup).\n'
    printf 'A SKIP is not a pass -- this script carries the S2/S3 admission rows.\n'
    exit 2
fi

for tool in xtensa-esp32s2-elf-objdump xtensa-esp32s3-elf-objdump; do
    if ! command -v "$tool" >/dev/null 2>&1; then
        printf 'SKIP: %s is not on PATH (run espup install/export).\n' "$tool"
        printf 'A SKIP is not a pass -- this script carries the S2/S3 admission rows.\n'
        exit 2
    fi
done

printf '==> rustc %s\n' "$(rustc -vV | sed -n 's/^release: //p')"
printf '==> esp-rs %s\n' "$(rustc +esp -vV | sed -n 's/^release: //p')"
printf '==> building thumbv6m and ESP32-S2/S3 probes\n'

cargo build --release --target thumbv6m-none-eabi \
    --manifest-path scripts/codesize/Cargo.toml --features cm0 >/dev/null
cargo +esp build --release --target xtensa-esp32s2-none-elf \
    --manifest-path scripts/codesize/Cargo.toml --features cm0 \
    -Zbuild-std=core >/dev/null
cargo +esp build --release --target xtensa-esp32s3-none-elf \
    --manifest-path scripts/codesize/Cargo.toml --features cm0 \
    -Zbuild-std=core >/dev/null

tmp_dir="$(mktemp -d)"
trap 'rm -rf "$tmp_dir"' EXIT

extract_probe_object() {
    archive="$1"
    output="$2"
    member="$($llvm_ar t "$archive" | sed -n '/ph_eventing_codesize.*cgu\.0\.rcgu\.o/{p;q;}')"
    if [ -z "$member" ]; then
        printf 'error: probe object not found in %s\n' "$archive" >&2
        exit 1
    fi
    "$llvm_ar" p "$archive" "$member" > "$output"
}

arm_archive="scripts/codesize/target/thumbv6m-none-eabi/release/libph_eventing_codesize.a"
s2_archive="scripts/codesize/target/xtensa-esp32s2-none-elf/release/libph_eventing_codesize.a"
s3_archive="scripts/codesize/target/xtensa-esp32s3-none-elf/release/libph_eventing_codesize.a"

extract_probe_object "$arm_archive" "$tmp_dir/thumbv6m.o"
extract_probe_object "$s2_archive" "$tmp_dir/esp32s2.o"
extract_probe_object "$s3_archive" "$tmp_dir/esp32s3.o"

"$llvm_objdump" -d "$tmp_dir/thumbv6m.o" > "$tmp_dir/thumbv6m.txt"
xtensa-esp32s2-elf-objdump -d "$tmp_dir/esp32s2.o" > "$tmp_dir/esp32s2.txt"
xtensa-esp32s3-elf-objdump -d "$tmp_dir/esp32s3.o" > "$tmp_dir/esp32s3.txt"

# Count instructions after the disable instruction through the architectural
# restore/synchronization instruction. This is the maximum interval for which
# a previously-enabled interrupt remains masked by the operation.
masked_count() {
    file="$1"
    symbol="$2"
    start="$3"
    finish="$4"
    awk -v symbol="$symbol" -v start="$start" -v finish="$finish" '
        $0 ~ "<" symbol ">:" { in_fn = 1; next }
        in_fn && /^$/ { in_fn = 0 }
        in_fn && $0 ~ /^[[:space:]]*[0-9a-f]+:/ {
            if ($0 ~ start) { masked = 1; next }
            if (masked) {
                count++
                if ($0 ~ finish) { print count; exit }
            }
        }
    ' "$file"
}

arm_raise="$(masked_count "$tmp_dir/thumbv6m.txt" event_flags_raise 'cpsid[[:space:]]+i' 'msr[[:space:]]+primask')"
arm_take="$(masked_count "$tmp_dir/thumbv6m.txt" event_flags_take 'cpsid[[:space:]]+i' 'msr[[:space:]]+primask')"
s2_raise="$(masked_count "$tmp_dir/esp32s2.txt" event_flags_raise 'rsil.*15' 'rsync')"
s2_take="$(masked_count "$tmp_dir/esp32s2.txt" event_flags_take 'rsil.*15' 'rsync')"

if [ "$arm_raise" != 4 ] || [ "$arm_take" != 4 ]; then
    printf 'error: thumbv6m masked window changed (raise=%s take=%s, expected 4/4).\n' \
        "${arm_raise:--}" "${arm_take:--}" >&2
    exit 1
fi
if [ "$s2_raise" != 5 ] || [ "$s2_take" != 5 ]; then
    printf 'error: ESP32-S2 masked window changed (raise=%s take=%s, expected 5/5).\n' \
        "${s2_raise:--}" "${s2_take:--}" >&2
    exit 1
fi

# Both portable paths must remain straight-line. Any branch in a hot function
# means the claimed single critical section has acquired a retry/dispatch path.
if sed -n '/<event_flags_raise>:/,/^$/p; /<event_flags_take>:/,/^$/p' "$tmp_dir/thumbv6m.txt" \
    | grep -Eq '^[[:space:]]*[0-9a-f]+:.*[[:space:]]b[a-z]*[[:space:]]'; then
    printf 'error: thumbv6m EventFlags hot path contains a branch.\n' >&2
    exit 1
fi
if sed -n '/<event_flags_raise>:/,/^$/p; /<event_flags_take>:/,/^$/p' "$tmp_dir/esp32s2.txt" \
    | grep -Eq '^[[:space:]]*[0-9a-f]+:.*[[:space:]]b[a-z]*[[:space:]]'; then
    printf 'error: ESP32-S2 EventFlags hot path contains a branch.\n' >&2
    exit 1
fi

if grep -Eq 'rsil|wsr\.ps' "$tmp_dir/esp32s3.txt"; then
    printf 'error: ESP32-S3 unexpectedly masks interrupts in EventFlags.\n' >&2
    exit 1
fi
if [ "$(grep -c 's32c1i' "$tmp_dir/esp32s3.txt")" -lt 2 ]; then
    printf 'error: ESP32-S3 no longer emits native S32C1I for both hot paths.\n' >&2
    exit 1
fi

printf '\n%-18s %8s %8s  %s\n' TARGET raise take implementation
printf '%-18s %8s %8s  %s\n' '------------------' '-----' '----' '--------------'
printf '%-18s %8s %8s  %s\n' thumbv6m "$arm_raise" "$arm_take" \
    'PRIMASK critical section'
printf '%-18s %8s %8s  %s\n' esp32-s2 "$s2_raise" "$s2_take" \
    'PS.INTLEVEL=15 critical section'
printf '%-18s %8s %8s  %s\n' esp32-s3 0 0 \
    'native S32C1I; no interrupt masking'

printf '\nCounts are instructions after interrupt disable through restore/sync.\n'
printf 'Both portable paths are straight-line and contain exactly one read,\n'
printf 'one update/store sequence, and no branch or compare-exchange loop.\n'
