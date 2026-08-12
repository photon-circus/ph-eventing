#!/usr/bin/env sh
# Loom model checking for ph-eventing.
#
# Loom enumerates every legal execution of a model -- every thread
# interleaving, and every value each relaxed load may return under the C11
# memory model. Where Miri samples schedules, Loom is exhaustive: a clean run
# proves the absence of ordering bugs for the modelled size.
#
# Loom is a dev-dependency gated on `--cfg loom`, so it is only resolved and
# downloaded by this script. A normal build, test, or package never sees it and
# the library keeps zero runtime dependencies.
#
# Models live in src/loom_tests.rs. They are deliberately tiny (capacity 2, two
# or three operations per thread) because the state space grows exponentially.
#
# Usage:
#   ./scripts/loom.sh                     # run every model
#   ./scripts/loom.sh event_buf           # filter by name
#   LOOM_MAX_PREEMPTIONS=3 ./scripts/loom.sh   # deeper (much slower)

set -u

cd "$(dirname "$0")/.." || exit 1

# See scripts/ci.sh: incremental compilation makes Windows runs flaky with
# "Access is denied (os error 5)". There is no PowerShell twin of this script;
# on Windows run it under Git Bash.
CARGO_INCREMENTAL=0
export CARGO_INCREMENTAL

# Loom's default bound keeps the state space tractable. Raising it explores
# more preemption points at exponential cost.
LOOM_MAX_PREEMPTIONS="${LOOM_MAX_PREEMPTIONS:-2}"
export LOOM_MAX_PREEMPTIONS

printf '==> loom (max_preemptions=%s)\n' "$LOOM_MAX_PREEMPTIONS"

# A bare name becomes loom_tests::<name>. Leading Cargo/test flags (anything
# starting with '-') must pass through unchanged — rewriting them produced
# filters like loom_tests::--quiet that match nothing and look like a pass.
filter="loom_tests"
if [ "$#" -gt 0 ]; then
    case "$1" in
        -*) ;;
        *)
            filter="loom_tests::$1"
            shift
            ;;
    esac
fi

# Capture the run and decide from the harness's own summary. Cargo exits 0
# when a filter matches nothing, and test-binary selectors passed after `--`
# (--ignored, --skip, --exact) shrink the selection further -- an earlier
# guard that re-listed without "$@" vouched for models the actual run never
# executed. Parsing the run's own "test result:" line counts exactly what
# ran. Not a pipe: a pipeline would report tee's exit status, not cargo's.
log="$(mktemp)"
trap 'rm -f "$log"' EXIT
if RUSTFLAGS='--cfg loom' cargo test --lib "$filter" "$@" >"$log" 2>&1; then
    cat "$log"
    ran="$(sed -n 's/^test result: ok\. \([0-9][0-9]*\) passed.*/\1/p' "$log" | tail -n 1)"
    if [ "${ran:-0}" -eq 0 ]; then
        printf '\nerror: the invocation ran no Loom models -- nothing was verified.\n' >&2
        printf 'Check the filter and any selectors after "--" (--ignored, --skip, --exact).\n' >&2
        exit 1
    fi
    printf '\nAll %s Loom models that matched were verified.\n' "$ran"
else
    cat "$log"
    printf '\nLoom found a failing execution. The output above replays the\n'
    printf 'exact interleaving -- it is deterministic, so re-running reproduces it.\n'
    exit 1
fi
