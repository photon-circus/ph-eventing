#!/usr/bin/env sh
# Local CI for ph-eventing.
#
# Runs the full check matrix locally: formatting, clippy, host tests, docs, and
# the embedded target checks. The GitHub Actions workflow runs on push and PR
# and covers most of this, but not the coverage gate below -- and not Miri or
# Loom, which live in scripts/miri.sh and scripts/loom.sh. This remains the
# authoritative run.
#
# There is deliberately no PowerShell twin. On Windows, run this under the Git
# Bash that ships with Git for Windows. Two scripts that must be kept in step
# is a standing invitation for them to drift, and the one that drifts is the
# one you are not running today.
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

# Incremental compilation makes this gate flaky on Windows: rustc cannot
# finalize target/*/incremental when a scanner or a previous run still holds a
# handle, and prints "Access is denied (os error 5)". The check it lands on
# varies between runs, so it reads as a real, moving defect. A gate that goes
# red at random trains you to re-run it instead of read it. CI builds fresh and
# gains nothing from incremental anyway.
CARGO_INCREMENTAL=0
export CARGO_INCREMENTAL

failed=0
skipped=0
summary=""

run_check() {
    name="$1"
    shift

    printf '\n==> %s\n' "$name"

    "$@"
    status=$?

    # Exit 2 means the check could not run -- a missing baseline, or a toolchain
    # it cannot compare against. That is a SKIP, and a SKIP is not a pass:
    # reporting it as PASS is exactly how a gate stops gating without anyone
    # noticing.
    if [ "$status" -eq 2 ]; then
        summary="${summary}  SKIP  ${name}\n"
        skipped=$((skipped + 1))
        return 0
    fi

    if [ "$status" -eq 0 ]; then
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
    if [ "$skipped" -gt 0 ]; then
        printf '\n%s check(s) SKIPPED. A skipped check is not a passed check --\n' "$skipped"
        printf 'see RELEASING.md before treating this run as verified.\n'
    fi
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
    skipped=$((skipped + 1))
fi

# Supply chain: advisories, licences, bans, sources. Skipped with a notice
# rather than failing the run if the tool is absent, since it is a separate
# install (`cargo install cargo-deny`).
if command -v cargo-deny >/dev/null 2>&1; then
    run_check 'deny' cargo deny check
else
    printf '\n==> deny\ncargo-deny not installed; skipping (cargo install cargo-deny)\n'
    summary="${summary}  SKIP  deny (not installed)\n"
    skipped=$((skipped + 1))
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
    skipped=$((skipped + 1))
fi

if [ "${SKIP_EMBEDDED:-0}" = "0" ]; then
    # Code size is a guarantee like any other: unpinned, it drifts. The gate is
    # presence-gated on the baseline and skips loudly when the toolchain differs
    # from the one that produced it, so it cannot become a source of noise.
    #
    # It belongs under SKIP_EMBEDDED because it cross-compiles all eight gated
    # targets. Run outside this branch it would make the advertised host-only
    # escape hatch fail for the one reason a caller sets it: not having the
    # embedded toolchains.
    run_check 'codesize (baseline gate)' ./scripts/codesize.sh

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
