#!/usr/bin/env sh
# Local CI for ph-eventing.
#
# Runs the full check matrix locally: formatting, clippy, host tests, docs, and
# the embedded target checks. This is the primary CI for this project -- the
# GitHub Actions workflow is manual-dispatch only and mirrors these same checks.
#
# Every check runs even if an earlier one fails, then a summary is printed.
# Exit code is non-zero if any check failed.
#
# Usage:
#   ./scripts/ci.sh                 # full matrix
#   SKIP_EMBEDDED=1 ./scripts/ci.sh # host checks only
#   FAIL_FAST=1 ./scripts/ci.sh     # stop at the first failure

set -u

cd "$(dirname "$0")/.." || exit 1

failed=0
summary=""

run_check() {
    name="$1"
    shift

    printf '\n==> %s\n' "$name"

    if "$@"; then
        summary="${summary}  PASS  ${name}\n"
    else
        summary="${summary}  FAIL  ${name}\n"
        failed=$((failed + 1))
        if [ "${FAIL_FAST:-0}" != "0" ]; then
            report
            exit 1
        fi
    fi
}

report() {
    printf '\nSummary\n'
    printf '%b' "$summary"
}

run_check 'fmt' cargo fmt --all -- --check
run_check 'clippy' cargo clippy --all-targets -- -D warnings
run_check 'test' cargo test
run_check 'doc' env RUSTDOCFLAGS='-D warnings' cargo doc --no-deps

if [ "${SKIP_EMBEDDED:-0}" = "0" ]; then
    run_check 'thumbv6m-none-eabi' \
        cargo check --target thumbv6m-none-eabi \
        --features portable-atomic-unsafe-assume-single-core
    run_check 'thumbv7em-none-eabi' cargo check --target thumbv7em-none-eabi
    run_check 'riscv32imac-unknown-none-elf' \
        cargo check --target riscv32imac-unknown-none-elf
fi

report

if [ "$failed" -gt 0 ]; then
    printf '\n%s check(s) failed.\n' "$failed"
    exit 1
fi

printf '\nAll checks passed.\n'
