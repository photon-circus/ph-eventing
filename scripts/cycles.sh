#!/usr/bin/env sh
# Instruction-cost measurements for ph-eventing, under QEMU.
#
# Code size (scripts/codesize.sh) says nothing about *time*. This measures the
# other half of the determinism claim: that the hot paths cost a bounded,
# knowable number of instructions regardless of buffer state. A `push` into an
# empty buffer and into a full one should differ by a small constant -- not
# scale with `N`, and not vary run to run.
#
# ## How it measures
#
# QEMU models neither DWT_CYCCNT nor SysTick on lm3s6965evb or mps2-an385 --
# both read zero, verified on QEMU 10.2. Ubuntu's qemu-system-arm ships no TCG
# plugins either. What works without either is QEMU's own execution trace:
#
#   -accel tcg,one-insn-per-tb=on   one guest instruction per translation block
#   -d exec                          log every block executed
#
# which yields exactly one line per retired instruction. The probe calls a
# never-inlined `mark()` between operations; this script finds its address with
# `nm` and counts instructions between hits. The first segment is two adjacent
# `mark()` calls, so its count is the marker overhead, subtracted from the rest.
#
# `-icount shift=0` pins one instruction to one clock tick so nothing depends on
# host speed. The result is deterministic: the same ELF gives the same counts on
# any machine.
#
# ## Requirements and the reference environment
#
# qemu-system-arm >= 8.1 (for one-insn-per-tb), and the thumbv7m-none-eabi
# target. Unlike the rest of the tooling this is NOT satisfied by
# rust-toolchain.toml alone -- QEMU is a system package the toolchain file
# cannot pin, and the counts are stable per QEMU build, not across builds:
# two of the eighteen regions were observed to differ by one instruction
# between a 10.0 and a 10.2 build. scripts/verify/Dockerfile pins the
# environment the documented numbers were measured in; run this script inside
# it via  ./scripts/verify.sh cycles  when comparing against those numbers.
#
# Usage:
#   ./scripts/cycles.sh            # local qemu-system-arm
#   ./scripts/cycles.sh latest-matrix
#   ./scripts/cycles.sh latest-block-matrix
#   ./scripts/verify.sh cycles     # same, inside the reference image

set -u

cd "$(dirname "$0")/.." || exit 1

CARGO_INCREMENTAL=0
export CARGO_INCREMENTAL

PROBE_FEATURES=""
for arg in "$@"; do
    case "$arg" in
        latest-matrix) PROBE_FEATURES="latest-matrix" ;;
        latest-block-matrix) PROBE_FEATURES="latest-block-matrix" ;;
        *) printf 'unknown argument: %s\n' "$arg" >&2; exit 64 ;;
    esac
done

PROBE_DIR="scripts/cycles"
ELF="$PROBE_DIR/target/thumbv7m-none-eabi/release/ph-eventing-cycles"
LOG="$PROBE_DIR/target/qemu-exec.log"

if ! command -v qemu-system-arm >/dev/null 2>&1; then
    printf 'SKIP: qemu-system-arm not installed.\n'
    printf '  Debian/Ubuntu:  sudo apt-get install qemu-system-arm\n'
    printf '  or use the reference image:  ./scripts/verify.sh cycles\n'
    printf 'A SKIP is not a pass -- see RELEASING.md.\n'
    # Exit 2 for "could not run", same convention as codesize.sh: a SKIP must
    # be distinguishable from a pass, or a wrapper that only checks the exit
    # code -- scripts/verify.sh's loop is exactly that -- reports the matrix
    # as passed with a measurement missing. Inside the reference image this
    # branch cannot fire (QEMU is baked in and guarded), so there a 2 means
    # the environment itself is broken.
    exit 2
fi

# Counts are stable per QEMU build, not across builds, so every run records
# the build it came from.
printf '==> %s\n' "$(qemu-system-arm --version | head -1)"

if ! rustup target list --installed 2>/dev/null | grep -qx thumbv7m-none-eabi; then
    printf 'SKIP: rustup target add thumbv7m-none-eabi\n'
    printf 'A SKIP is not a pass -- see RELEASING.md.\n'
    exit 2
fi

SYSROOT="$(rustc --print sysroot)" || exit 1
HOST="$(rustc -vV | sed -n 's/^host: //p')"
NM="$SYSROOT/lib/rustlib/$HOST/bin/llvm-nm"
[ -x "$NM" ] || NM="${NM}.exe"
if [ ! -x "$NM" ]; then
    printf 'error: llvm-nm not found (rustup component add llvm-tools)\n' >&2
    exit 1
fi

printf '==> building probe\n'
# Must build from inside the probe directory: cargo resolves .cargo/config.toml
# from the working directory, not from --manifest-path. Building from the repo
# root silently picks up the root config instead, targets the host, and fails to
# link against libc.
if [ -n "$PROBE_FEATURES" ]; then
    build_probe() { ( cd "$PROBE_DIR" && cargo build --release --features "$PROBE_FEATURES" ); }
else
    build_probe() { ( cd "$PROBE_DIR" && cargo build --release ); }
fi
if ! build_probe >/dev/null 2>&1; then
    printf 'error: probe failed to build\n' >&2
    build_probe 2>&1 | tail -20 >&2
    exit 1
fi

# Regions are labelled by symbol name, not by position, so adding a measurement
# to the probe needs no change here. Build addr -> label from the symbol table.
# Thumb function addresses have bit 0 set to mark Thumb state; the PC in the
# trace does not, so clear it before comparing.
SYMS="$PROBE_DIR/target/markers.txt"
"$NM" "$ELF" 2>/dev/null | awk '$3 ~ /^m_/ { print $1, $3 }' > "$SYMS"
if [ ! -s "$SYMS" ]; then
    printf 'error: no `m_*` markers found in %s\n' "$ELF" >&2
    exit 1
fi

# Guard against identical code folding. Markers with identical bodies get merged
# by the linker onto one address; the runner then sees a single label and prints
# nothing, which reads as "the probe measured nothing" rather than as a bug.
# Each marker embeds a unique immediate to prevent it -- this checks that it
# actually worked.
total_marks="$(wc -l < "$SYMS")"
uniq_marks="$(awk '{ print $1 }' "$SYMS" | sort -u | wc -l)"
if [ "$total_marks" -ne "$uniq_marks" ]; then
    printf 'error: %s markers share only %s distinct addresses.\n' \
        "$total_marks" "$uniq_marks" >&2
    printf 'The linker folded identical marker bodies. Each marker in\n' >&2
    printf 'scripts/cycles/src/main.rs must embed a unique immediate.\n' >&2
    exit 1
fi

printf '==> tracing under qemu\n'
mkdir -p "$(dirname "$LOG")"
timeout 300 qemu-system-arm \
    -cpu cortex-m3 -machine lm3s6965evb -nographic \
    -semihosting-config enable=on,target=native \
    -accel tcg,one-insn-per-tb=on -icount shift=0 \
    -d exec -D "$LOG" -kernel "$ELF" >/dev/null 2>&1
qemu_status=$?

# A crash or timeout after some trace was already written leaves a non-empty
# log, so the emptiness check alone would let a partial run through.
if [ "$qemu_status" -ne 0 ]; then
    printf 'error: qemu exited %s (124 = timeout)
' "$qemu_status" >&2
    exit 1
fi

if [ ! -s "$LOG" ]; then
    printf 'error: qemu produced no trace\n' >&2
    exit 1
fi

# Every marker except `m_end` and `m_overhead` opens a region that must produce
# exactly one row. Capturing the report rather than streaming it is what lets
# both the parse status and the row count be checked -- otherwise a changed
# trace format, an unreached marker, or an awk error prints a short table under
# a success footer and exits 0, which is precisely the silent-measurement
# failure this probe exists to rule out.
expected_regions=$((total_marks - 2))

report="$(awk '
    # GNU awk has a strtonum builtin; mawk, the default awk on a stock
    # Debian/Ubuntu, does not -- verified. The parse then aborts and, with no
    # status check, the script printed its footer and exited 0 having measured
    # nothing. This is POSIX awk.
    function hex2dec(h,   i, c, d, v) {
        h = tolower(h); v = 0
        for (i = 1; i <= length(h); i++) {
            c = substr(h, i, 1)
            d = index("0123456789abcdef", c) - 1
            if (d < 0) return -1
            v = v * 16 + d
        }
        return v
    }
    NR == FNR {
        addr = hex2dec($1)
        addr = addr - (addr % 2)      # clear the Thumb bit
        label[addr] = $2
        next
    }
    /^Trace/ {
        if (!match($0, /\/[0-9a-f]{16}\//)) next
        pc = hex2dec(substr($0, RSTART + 1, 16))
        if (pc in label) {
            name = label[pc]
            if (name == "m_end") {
                if (open != "") {
                    n = count - overhead
                    if (open == "m_overhead") { overhead = count; n = count }
                    if (n < 0) n = 0
                    if (open != "m_overhead") {
                        pretty = substr(open, 3)
                        gsub(/_/, " ", pretty)
                        printf "  %-26s %5d\n", pretty, n
                    }
                }
                open = ""
            } else {
                open = name; count = 0
                if (name != "m_overhead" && group != substr(name, 3, 2)) {
                    group = substr(name, 3, 2)
                    if (group == "eb") printf "\nEventBuf  (backpressure SPSC)\n"
                    else if (group == "sr") printf "\nSeqRing   (overwrite SPSC)\n"
                    else if (group == "rb") printf "\nRingBuf   (single owner)\n"
                    else if (group == "lb") printf "\nLatestBuf (freshness-first SPSC)\n"
                    else if (group == "lc") printf "\nLatestBuf sample/block composition\n"
                }
            }
            next
        }
        if (open != "") count++
    }
' "$SYMS" "$LOG")"
awk_status=$?

if [ "$awk_status" -ne 0 ]; then
    printf 'error: trace parse failed (awk exited %s)\n' "$awk_status" >&2
    exit 1
fi

# `grep -c` exits non-zero on a count of zero, which would be swallowed here.
measured="$(printf '%s\n' "$report" | awk '/^  [a-z]/ { n++ } END { print n + 0 }')"
if [ "$measured" -ne "$expected_regions" ]; then
    printf 'error: measured %s regions, expected %s.\n' \
        "$measured" "$expected_regions" >&2
    printf 'A marker was never reached, or the QEMU trace format changed.\n' >&2
    exit 1
fi

printf '\n%s\n' "$report"

printf '\n'
printf 'Instructions retired on the guest, marker overhead subtracted.\n'
printf 'Deterministic per environment: -icount shift=0 pins one instruction to\n'
printf 'one tick, so the same ELF under the same QEMU build yields the same\n'
printf 'counts on any host. Different QEMU builds can shift region boundaries\n'
printf 'by an instruction -- compare inside the reference image (verify.sh).\n'

rm -f "$SYMS"
rm -f "$LOG"
