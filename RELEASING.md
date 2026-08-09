# Releasing

A checklist for cutting a ph-eventing release. Work top to bottom; every step
is a command you can run, not a judgement call, except where marked.

Two things about this repo shape the process:

- **Nothing is checked remotely.** The GitHub Actions workflow is
  manual-dispatch only, so no CI will catch a mistake after you push. The local
  runs below are the only gate.
- **`cargo publish` is irreversible.** You can yank a version, but a yank does
  not delete it and does not free the version number. Everything that can be
  checked should be checked *before* the publish step.

---

## 1. Preconditions

```bash
git switch master && git pull
git status --short          # must be empty
```

Release from `master` with a clean tree. A dirty tree risks publishing files
that are not in any commit — `--allow-dirty` exists but should not be used for
a real release.

## 2. Choose the version — judgement call

This crate is pre-1.0, so under Cargo's semver rules the **minor** position is
the breaking one:

| Change | Bump |
|--------|------|
| Bug fix, doc fix, internal change, new private item | `0.1.2` → `0.1.3` |
| New public API, added trait impl, relaxed bound | `0.1.2` → `0.1.3` (still compatible) |
| Removed or renamed public item, tightened bound, changed behaviour a caller could rely on | `0.1.2` → `0.2.0` |
| MSRV increase | `0.2.0` — treat as breaking |

When unsure, take the larger bump. A needless minor bump costs nothing; a
breaking change shipped as a patch breaks builds downstream.

Adding a `#[non_exhaustive]`-free public struct field, or a new variant to a
public enum, is breaking. So is anything that changes documented behaviour for
the SPSC types — callers write ordering-sensitive code against them.

## 3. Update the manifest and changelog

```bash
# Cargo.toml: version = "X.Y.Z"
# CHANGELOG.md: rename the Unreleased heading to "## X.Y.Z - YYYY-MM-DD"
```

The changelog entry should carry a **Known issues** section when a documented
deviation still applies — `SeqRing`'s seqlock data race is one, and dropping it
silently would be a regression in honesty, not just in docs.

## 4. Refresh anything version-sensitive

These drift silently because nothing regenerates them:

```bash
cargo test 2>&1 | grep "^test result"      # README + AGENTS.md test counts
cargo llvm-cov --summary-only              # README coverage figures
```

- **Test counts** appear in `README.md` (per-module table) and `AGENTS.md`.
- **Coverage** appears in `README.md`. It is not deterministic here — the
  threaded stress tests take different paths per run — so state a rounded
  figure, not an exact one.
- **MSRV** appears in `Cargo.toml` (`rust-version`), `rust-toolchain.toml`,
  the README badge, and `CONTRIBUTING.md`. If it moved, all four change, and
  the version bump is breaking (see above).

## 5. Verify

All three must be clean. None of them run remotely.

```bash
./scripts/ci.sh          # fmt, clippy, tests, docs, cargo-deny, coverage floor, 3 embedded targets
./scripts/miri.sh        # UB, data races, weak memory, 32-bit and big-endian
./scripts/loom.sh        # exhaustive model checking
```

`scripts/ci.sh` reports `SKIP` for checks whose tool is not installed. **A run
with SKIPs is not a verified run** — install the tool and re-run before
releasing:

```bash
cargo install cargo-deny cargo-llvm-cov
rustup component add --toolchain nightly miri
```

## 6. Check what will actually ship

```bash
cargo package --list        # expect: Cargo.toml, LICENSE, README.md, src/**
cargo publish --dry-run     # packages and does a verification build
cargo tree                  # must print the crate alone — zero dependencies
```

The manifest uses an `include` allowlist, so a new file is unpublished by
default. If you added something that *should* ship, add it to `include`; the
dry-run listing is where you find out you forgot.

## 7. Commit, tag, push

```bash
git add -A
git commit -m "Release X.Y.Z"
git tag -a vX.Y.Z -m "vX.Y.Z"
git push origin master --follow-tags
```

Tag after the verification passes, so the tag names a state you actually
checked.

## 8. Publish

```bash
cargo publish
```

Needs a crates.io token (`cargo login`). This is the irreversible step.

## 9. After publishing

- **Check the docs.rs build.** It runs independently and can fail even when
  `cargo publish` succeeded — a feature combination that does not compile on
  docs.rs's target, for example. Watch <https://docs.rs/ph-eventing>; a failure
  shows in the build log there, not in your terminal.
- **Create the GitHub release** against the tag, pasting the changelog section.
- **Open the next `## Unreleased`** heading in `CHANGELOG.md`.

## If something is wrong after publishing

```bash
cargo yank --version X.Y.Z            # discourage new use
cargo yank --version X.Y.Z --undo     # reverse the yank
```

A yank stops new dependents resolving to that version. It does **not** delete
it, does not break existing `Cargo.lock` files that already pin it, and does
not let you reuse the version number. The fix for a bad release is a new
release; yanking is damage control alongside it, not instead of it.

Do not attempt to re-publish a corrected artifact under the same version — the
registry rejects it, and any consumer who already fetched the bad one keeps it.
