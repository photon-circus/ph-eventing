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
# ## Requirements
#
# qemu-system-arm, and the thumbv7m-none-eabi target. Unlike the rest of the
# tooling this is NOT satisfied by rust-toolchain.toml alone -- QEMU is a system
# package. On Debian/Ubuntu:  sudo apt-get install qemu-system-arm
#
# Usage:
#   ./scripts/cycles.sh

set -u

cd "$(dirname "$0")/.." || exit 1

CARGO_INCREMENTAL=0
export CARGO_INCREMENTAL

PROBE_DIR="scripts/cycles"
ELF="$PROBE_DIR/target/thumbv7m-none-eabi/release/ph-eventing-cycles"
LOG="$PROBE_DIR/target/qemu-exec.log"

if ! command -v qemu-system-arm >/dev/null 2>&1; then
    printf 'SKIP: qemu-system-arm not installed.\n'
    printf '  Debian/Ubuntu:  sudo apt-get install qemu-system-arm\n'
    printf 'A SKIP is not a pass -- see RELEASING.md.\n'
    exit 0
fi

if ! rustup target list --installed 2>/dev/null | grep -qx thumbv7m-none-eabi; then
    printf 'SKIP: rustup target add thumbv7m-none-eabi\n'
    exit 0
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
if ! ( cd "$PROBE_DIR" && cargo build --release >/dev/null 2>&1 ); then
    printf 'error: probe failed to build\n' >&2
    ( cd "$PROBE_DIR" && cargo build --release 2>&1 | tail -20 >&2 )
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

if [ ! -s "$LOG" ]; then
    printf 'error: qemu produced no trace\n' >&2
    exit 1
fi

printf '\n'
awk '
    NR == FNR {
        addr = strtonum("0x" $1)
        addr = int(addr / 2) * 2      # clear the Thumb bit
        label[addr] = $2
        next
    }
    /^Trace/ {
        if (!match($0, /\/[0-9a-f]{16}\//)) next
        pc = strtonum("0x" substr($0, RSTART + 1, 16))
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
                }
            }
            next
        }
        if (open != "") count++
    }
' "$SYMS" "$LOG"

printf '\n'
printf 'Instructions retired on the guest, marker overhead subtracted.\n'
printf 'Deterministic: -icount shift=0 pins one instruction to one tick, so the\n'
printf 'same ELF yields the same counts on any host.\n'

rm -f "$SYMS"
rm -f "$LOG"
