# The reference verification environment for ph-eventing.
#
# The crate's claims -- no UB, exhaustively-modelled interleavings, bounded
# instruction counts, flash cost per target -- are only claims if someone else
# can reproduce the evidence. rust-toolchain.toml pins the stable toolchain,
# but three things it cannot pin live here instead:
#
#   - QEMU. Instruction counts are deterministic per QEMU build, not across
#     builds: a 10.2 build differed from this image's 10.0.x on two of the
#     eighteen regions by exactly one instruction (trace boundary attribution,
#     not codegen). The documented numbers are measured in THIS image.
#   - The Miri nightly. A bare `+nightly` drifts daily; a Miri verdict is only
#     reproducible alongside the nightly that produced it. This image pins the
#     one the release evidence was gathered with (MIRI_TOOLCHAIN below).
#   - The optional ci.sh tools (cargo-deny, cargo-llvm-cov, a stable
#     toolchain). Absent, ci.sh reports SKIPs; inside this image the full
#     15-check matrix runs with none.
#
# A prebuilt copy is published at hub.docker.com/r/stevegiacomelli/ph-eventing-verify;
# each 0.x.y tag is the frozen environment of that release's evidence:
#
#   VERIFY_IMAGE=stevegiacomelli/ph-eventing-verify:0.2.0 ./scripts/verify.sh
#
# Build locally instead (from the repo root -- the context must include
# rust-toolchain.toml and the lockfiles):
#
#   docker build -t ph-eventing-verify -f scripts/verify/Dockerfile .
#
# Run any script inside it, or everything:
#
#   ./scripts/verify.sh            # ci + miri + loom + cycles + atomic-window
#   ./scripts/verify.sh cycles     # one of them
#   ./scripts/verify.sh atomic-window  # EventFlags thumbv6m interrupt-mask gate
#   ./scripts/verify.sh shell      # poke around
#
# Deliberately NO esp-rs here. EventFlags ESP32-S2/S3 interrupt-window rows are
# opt-in on hosts that have the fork (ESP=1); baking esp-rs into this image
# would make the fork mandatory for a green verify matrix.
#
# The stable toolchain baked here is whatever was current at image build time.
# That is deliberate: the stable check exists to catch regressions on a NEWER
# compiler than the pin, so freezing it would defeat its purpose. Every run
# prints its versions; cite those alongside any result.
FROM rust:1.92.0-slim-trixie

# git: cargo-deny shells out to the git CLI to fetch the advisory database.
RUN apt-get update \
 && apt-get install -y --no-install-recommends qemu-system-arm git ca-certificates \
 && rm -rf /var/lib/apt/lists/*

# Apt cannot pin QEMU: Debian's archive rotates point releases out, so
# `qemu-system-arm=<version>` breaks the build the day the version leaves the
# index, and snapshot.debian.org trades that for a slow, sometimes-absent
# mirror. The immutable pin is therefore the PUBLISHED image tag -- a release's
# `0.x.y` tag is never retagged, and rebuilding this file later is NOT
# guaranteed to reproduce it. This guard makes that loud instead of silent: if
# a rebuild resolves a different QEMU series than the one the release evidence
# used, the build fails here rather than producing an image that claims to be
# the reference but measures differently.
ARG QEMU_SERIES=10.0
RUN qemu-system-arm --version | head -1 | grep -qF "version $QEMU_SERIES." || { \
      echo "QEMU series drifted from $QEMU_SERIES -- this rebuild would not" ;  \
      echo "reproduce the release evidence. Use the published image tag, or" ;  \
      echo "re-measure and re-bless the documented counts with the new QEMU" ;  \
      echo "(then update QEMU_SERIES)." ; \
      qemu-system-arm --version | head -1 ; \
      exit 1 ; }

# The pinned toolchain, its components, and its targets, exactly as
# rust-toolchain.toml declares them. Copying the file in and running a cargo
# command against it keeps this image from drifting out of sync with the pin:
# there is no second list of targets to forget to update.
COPY rust-toolchain.toml /tmp/pin/rust-toolchain.toml
RUN cd /tmp/pin && rustup toolchain install && rm -rf /tmp/pin

# The Miri nightly. Pinned to a date, not `nightly`, so the evidence is
# replayable. The cross-targets mirror scripts/miri.sh: 32-bit pointer width
# (i686, armv7) and big-endian (s390x) proxies for the bare-metal targets.
ENV MIRI_TOOLCHAIN=nightly-2026-08-08
RUN rustup toolchain install "$MIRI_TOOLCHAIN" --profile minimal \
      --component miri,rust-src \
 && rustup target add --toolchain "$MIRI_TOOLCHAIN" \
      i686-unknown-linux-gnu armv7-unknown-linux-gnueabihf s390x-unknown-linux-gnu
# Deliberately NO `cargo miri setup` here. A sysroot baked at image-build time
# came out unusable at runtime: every later build aborted with "detected a
# concurrent sysroot build with different settings", including a fresh
# `miri setup` over the baked cache. The cross-target passes worked all along
# precisely because their sysroots were never baked. Building at runtime costs
# ~15s per target per container and always works.

# Current stable, for ci.sh's forward-compatibility checks.
RUN rustup toolchain install stable --profile minimal \
      --component clippy

# The optional ci.sh gates, pinned to the versions the release evidence used.
RUN cargo install --locked cargo-deny@0.20.2 cargo-llvm-cov@0.8.7

# Preload every crate the matrix compiles -- the root workspace (loom is a
# dev-dependency) and both probe workspaces -- so a container without outbound
# network can still build and run everything except the cargo-deny advisory
# refresh, which is time-varying by design and stays online. The manifests are
# copied into a stub layout because the probes reach the root crate through a
# `path = "../.."` dependency; `cargo fetch --locked` then resolves from the
# committed lockfiles, so what is cached is exactly what a run will use.
COPY Cargo.toml Cargo.lock /tmp/fetch/
COPY scripts/codesize/Cargo.toml scripts/codesize/Cargo.lock /tmp/fetch/scripts/codesize/
COPY scripts/cycles/Cargo.toml scripts/cycles/Cargo.lock /tmp/fetch/scripts/cycles/
RUN mkdir -p /tmp/fetch/src /tmp/fetch/scripts/codesize/src /tmp/fetch/scripts/cycles/src \
 && touch /tmp/fetch/src/lib.rs /tmp/fetch/scripts/codesize/src/lib.rs \
 && printf 'fn main() {}\n' > /tmp/fetch/scripts/cycles/src/main.rs \
 && cd /tmp/fetch && cargo fetch --locked \
 && cd /tmp/fetch/scripts/codesize && cargo fetch --locked \
 && cd /tmp/fetch/scripts/cycles && cargo fetch --locked \
 && rm -rf /tmp/fetch

# OCI metadata, so registries can link the image back to its source and
# licence without anyone having to pull it first.
LABEL org.opencontainers.image.title="ph-eventing-verify" \
      org.opencontainers.image.description="Reference verification environment for the ph-eventing crate: pinned rustc, Miri nightly, QEMU." \
      org.opencontainers.image.source="https://github.com/photon-circus/ph-eventing" \
      org.opencontainers.image.licenses="MIT"

WORKDIR /work
