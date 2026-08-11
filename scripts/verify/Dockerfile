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
# Build (from the repo root -- the context must include rust-toolchain.toml):
#
#   docker build -t ph-eventing-verify -f scripts/verify/Dockerfile .
#
# Run any script inside it, or everything:
#
#   ./scripts/verify.sh            # ci.sh + miri.sh + loom.sh + cycles.sh
#   ./scripts/verify.sh cycles     # one of them
#   ./scripts/verify.sh shell      # poke around
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
RUN cargo install --locked cargo-deny@0.20.2 cargo-llvm-cov@0.8.7 \
 && rm -rf "$CARGO_HOME/registry"

WORKDIR /work
