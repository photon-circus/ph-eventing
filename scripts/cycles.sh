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

# Thumb symbol addresses have bit 0 set to mark Thumb state; the PC in the
# trace does not. Clear it before comparing.
MARK_ADDR="$("$NM" "$ELF" 2>/dev/null | awk '$3 == "mark" { print $1; exit }')"
if [ -z "$MARK_ADDR" ]; then
    printf 'error: could not find the `mark` symbol in %s\n' "$ELF" >&2
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
awk -v mark_hex="$MARK_ADDR" '
    BEGIN {
        # Strip the Thumb bit and normalise to a decimal address.
        mark = strtonum("0x" mark_hex)
        mark = int(mark / 2) * 2
        seg = -1; count = 0; overhead = 0
        split("marker-overhead push-into-empty (setup) push-into-nearly-full " \
              "push-into-full-rejected pop-from-full (drain) pop-from-empty len", \
              name, " ")
    }
    # Trace lines look like:  Trace 0: 0x... [flags/PC/...]
    /^Trace/ {
        if (match($0, /\/[0-9a-f]{16}\//)) {
            pc = strtonum("0x" substr($0, RSTART + 1, 16))
            if (pc == mark) {
                if (seg >= 0) {
                    n = count
                    if (seg == 0) { overhead = n }
                    else { n = n - overhead; if (n < 0) n = 0 }
                    label = (seg + 1 <= 9) ? name[seg + 1] : "seg" seg
                    if (label !~ /^\(/) printf "  %-28s %6d\n", label, n
                }
                seg++; count = 0; next
            }
            count++
        }
    }
' "$LOG"

printf '\n'
printf 'Instructions retired on the guest, marker overhead subtracted.\n'
printf 'Deterministic: -icount shift=0 pins one instruction to one tick, so the\n'
printf 'same ELF yields the same counts on any host.\n'

rm -f "$LOG"
