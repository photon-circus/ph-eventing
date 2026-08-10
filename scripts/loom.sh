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

if RUSTFLAGS='--cfg loom' cargo test --lib loom_tests "$@"; then
    printf '\nAll Loom models verified.\n'
else
    printf '\nLoom found a failing execution. The output above replays the\n'
    printf 'exact interleaving -- it is deterministic, so re-running reproduces it.\n'
    exit 1
fi
