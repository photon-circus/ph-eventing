#!/usr/bin/env sh
# Run the local verification matrix inside the reference environment.
#
# "Local", not "offline": crate sources are preloaded into the image, so the
# builds run without network, but the cargo-deny advisory refresh is
# time-varying by design and still wants one.
#
# Local runs of ci.sh / miri.sh / loom.sh / cycles.sh are only as reproducible
# as the machine they run on: QEMU builds differ, `+nightly` drifts daily, and
# the optional gates skip when their tools are absent. This wraps the same
# scripts in the image scripts/verify/Dockerfile pins, which is the
# environment the documented numbers come from. See that file for exactly
# what is pinned and why.
#
# Usage:
#   ./scripts/verify.sh            # ci + miri + loom + cycles, in order
#   ./scripts/verify.sh ci         # one script
#   ./scripts/verify.sh miri
#   ./scripts/verify.sh loom
#   ./scripts/verify.sh cycles
#   ./scripts/verify.sh cycles slot-pool
#   ./scripts/verify.sh shell      # interactive shell in the image
#
# Requires Docker. Everything else is inside the image.
#
# This matrix is DELIBERATELY not remote CI. The expensive checks were taken
# off the hosted pipeline on purpose -- the reasoning is documented (AGENTS.md:
# remote CI is a subset, not the gate; cycles.sh: why it is not in ci.sh) --
# and the guard below keeps a well-meaning workflow edit from quietly
# reversing that decision. If that trade-off is ever revisited, remove the
# guard in the same change that adds the workflow, so the reversal is explicit
# and reviewed rather than incidental.

set -u

cd "$(dirname "$0")/.." || exit 1

if [ -n "${GITHUB_ACTIONS:-}" ]; then
    printf 'error: verify.sh is the local reference matrix, not a hosted job.\n' >&2
    printf 'Keeping it out of remote CI is a documented decision -- see the\n' >&2
    printf 'header of this script. Remove this guard only in a change that\n' >&2
    printf 'deliberately revisits that decision.\n' >&2
    exit 1
fi

if ! command -v docker >/dev/null 2>&1; then
    printf 'error: docker not found -- the reference environment needs it.\n' >&2
    printf 'Each script also runs directly (./scripts/ci.sh etc.) if your\n' >&2
    printf 'machine has the tools, but numbers may differ; see AGENTS.md.\n' >&2
    exit 1
fi

# VERIFY_IMAGE selects a prebuilt image (e.g. one published to a registry)
# instead of building locally. The local build and the published image are the
# same Dockerfile; publishing just saves everyone the build.
IMAGE="${VERIFY_IMAGE:-ph-eventing-verify}"

if [ -z "${VERIFY_IMAGE:-}" ]; then
    printf '==> building %s (cached after the first time)\n' "$IMAGE"
    docker build -t "$IMAGE" -f scripts/verify/Dockerfile . || exit 1
fi

# MSYS_NO_PATHCONV: Git Bash on Windows otherwise rewrites /work into a host
# path and -w arrives mangled. Harmless elsewhere.
run() {
    env MSYS_NO_PATHCONV=1 docker run --rm ${DOCKER_TTY:+-it} \
        -v "$(pwd):/work" -w /work "$IMAGE" "$@"
}

case "${1:-all}" in
    shell)
        DOCKER_TTY=1 run bash
        ;;
    ci)     run sh scripts/ci.sh ;;
    miri)   run sh scripts/miri.sh ;;
    loom)   run sh scripts/loom.sh ;;
    cycles)
        shift
        run sh scripts/cycles.sh "$@"
        ;;
    all)
        # Version stamp first, so any pasted output carries its environment.
        run sh -c 'rustc --version; rustc "+$MIRI_TOOLCHAIN" --version; qemu-system-arm --version | head -1'
        failed=0
        for s in ci miri loom cycles; do
            printf '\n########## %s ##########\n' "$s"
            run sh "scripts/$s.sh" || failed=$((failed + 1))
        done
        if [ "$failed" -gt 0 ]; then
            printf '\n%s script(s) failed.\n' "$failed"
            exit 1
        fi
        printf '\nFull matrix passed in the reference environment.\n'
        ;;
    *)
        printf 'unknown argument: %s (ci|miri|loom|cycles|shell)\n' "$1" >&2
        exit 1
        ;;
esac
