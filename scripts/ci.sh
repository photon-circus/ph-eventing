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

# Feature combinations, one at a time. `--all-features` cannot work here: the
# two portable-atomic backend features are mutually exclusive, so the supported
# set has to be enumerated rather than swept.
run_check 'features: default' cargo check --quiet
run_check 'features: portable-atomic' cargo check --quiet --features portable-atomic
run_check 'features: critical-section' \
    cargo check --quiet --features portable-atomic-critical-section

# Current stable. rust-toolchain.toml pins the MSRV, so every other check here
# runs 1.92.0 -- without this, nothing would catch a new stable lint or a
# regression that only appears on a newer compiler. `+stable` is explicit and
# overrides the pin.
if rustup toolchain list 2>/dev/null | grep -q '^stable'; then
    run_check 'stable: test' cargo +stable test --quiet
    run_check 'stable: clippy' cargo +stable clippy --all-targets --quiet -- -D warnings
else
    printf '\n==> stable\nstable toolchain not installed; skipping (rustup toolchain install stable)\n'
    summary="${summary}  SKIP  stable (not installed)\n"
fi

# Supply chain: advisories, licences, bans, sources. Skipped with a notice
# rather than failing the run if the tool is absent, since it is a separate
# install (`cargo install cargo-deny`).
if command -v cargo-deny >/dev/null 2>&1; then
    run_check 'deny' cargo deny check
else
    printf '\n==> deny\ncargo-deny not installed; skipping (cargo install cargo-deny)\n'
    summary="${summary}  SKIP  deny (not installed)\n"
fi

# Coverage floor. A gate, not a vanity number -- it fails only on a real drop,
# so it cannot rot into decoration. cargo-llvm-cov uses its own target dir, so
# this does not invalidate the normal build cache.
COVERAGE_FLOOR="${COVERAGE_FLOOR:-90}"
if command -v cargo-llvm-cov >/dev/null 2>&1; then
    run_check "coverage (>=${COVERAGE_FLOOR}% lines)" \
        cargo llvm-cov --summary-only --fail-under-lines "$COVERAGE_FLOOR"
else
    printf '\n==> coverage\ncargo-llvm-cov not installed; skipping (cargo install cargo-llvm-cov)\n'
    summary="${summary}  SKIP  coverage (not installed)\n"
fi

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
