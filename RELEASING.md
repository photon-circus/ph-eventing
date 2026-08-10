# Releasing

A checklist for cutting a ph-eventing release. Work top to bottom; every step
is a command you can run, not a judgement call, except where marked.

Three things about this repo shape the process:

- **Remote CI is a subset.** GitHub Actions runs on push and PR, but it has no
  coverage, Miri, or Loom job. Those are exactly the checks a release needs
  most, so a green check never stands in for the local runs in step 6.
- **`cargo publish` is irreversible.** You can yank a version, but a yank does
  not delete it and does not free the version number. Everything that can be
  checked should be checked *before* the publish step.
- **Releases are cut on a `release/X.Y.Z` branch, not on `master`.** A PR that
  passed on its own branch is not evidence that it still passes alongside the
  other PRs in the same release. The release branch is where that *combination*
  is assembled and given the full local matrix — including the coverage, Miri,
  and Loom runs that remote CI does not do — before any of it reaches `master`.
  It also lets work aimed at a later version keep landing on `master`
  meanwhile. `v0.1.2` and `v0.1.3` predate this and were tagged directly on
  `master`.

---

## 1. Cut the release branch

```bash
git switch master && git pull
git status --short          # must be empty before you branch
git switch -c release/X.Y.Z
git push -u origin release/X.Y.Z
gh pr create --base master --head release/X.Y.Z --draft --title "release: X.Y.Z"
```

A dirty tree risks publishing files that are not in any commit —
`--allow-dirty` exists but should not be used for a real release.

Open the merge-back PR now, as a draft, rather than at the end. It is where the
release's verification output goes as you accumulate it, it makes the release
visible to anyone looking at the PR list, and creating it early means step 10
cannot forget it. Draft is load-bearing: this PR must not merge until the tag
exists.

## 2. Route the PRs — judgement call

Retarget every PR that belongs in this release onto the branch:

```bash
gh pr edit <N> --base release/X.Y.Z
gh pr list --json number,baseRefName,mergeable   # confirm the routing
```

**Retarget; do not rebase or cherry-pick.** Changing a PR's base moves the same
commits, review threads, and PR number wholesale. Cherry-picking duplicates the
commits and leaves the original PR to be closed rather than merged, which loses
the link between the review and the merged code.

PRs that do *not* belong in this release stay based on `master`. That is the
whole point of the branch: work that needs a larger version bump keeps landing
on `master` while this release stabilises. Use the table in step 3 to decide
which is which — if a PR needs a bigger bump than the one you are cutting, it
does not belong here.

**Upstream-first.** A fix that applies to both this release and future work
lands on `master` first and is cherry-picked onto the release branch, never the
reverse. A fix applied only to the release branch is a bug that reappears in the
next minor version, and nothing will catch it.

Merge the PRs into the release branch in order, resolving conflicts as they
arise. Expect them: sibling branches cut from the same commit routinely collide
in `CHANGELOG.md`, `README.md`, and `AGENTS.md` even when the code does not.

## 3. Choose the version — judgement call

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

## 4. Update the manifest and changelog

```bash
# Cargo.toml: version = "X.Y.Z"
# CHANGELOG.md: rename the Unreleased heading to "## X.Y.Z - YYYY-MM-DD"
```

The changelog entry should carry a **Known issues** section when a documented
deviation still applies — `SeqRing`'s seqlock data race is one, and dropping it
silently would be a regression in honesty, not just in docs.

**Do this step after every PR in step 2 has merged, not before.** The manifest
bump is safe at any time, but renaming the heading is not: an open PR's
changelog entry is anchored on the `## Unreleased` line, so closing that heading
early turns every one of them `CONFLICTING` for a reason unrelated to its
content. Bumping `Cargo.toml` early is a reasonable way to make the branch
self-describing; closing the heading early only creates work.

## 5. Refresh anything version-sensitive

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

## 6. Verify

All three must be clean, locally. Remote CI overlaps the first one but skips its
coverage gate, and does not run the other two at all.

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

## 7. Check what will actually ship

```bash
cargo package --list        # expect: Cargo.toml, LICENSE, README.md, src/**
cargo publish --dry-run     # packages and does a verification build
cargo tree                  # must print the crate alone — zero dependencies
```

The manifest uses an `include` allowlist, so a new file is unpublished by
default. If you added something that *should* ship, add it to `include`; the
dry-run listing is where you find out you forgot.

## 8. Tag and push

```bash
git add -A
git commit -m "Release X.Y.Z"
git tag -a vX.Y.Z -m "vX.Y.Z"
git push origin release/X.Y.Z --follow-tags
```

Tag after the verification passes, so the tag names a state you actually
checked.

Tag on the release branch, not on `master`. The release branch tip is the commit
you just verified in steps 6 and 7; `master` at this moment is a different tree
and may carry work aimed at a later version. The tag becomes reachable from
`master` when the branch merges back in step 10.

## 9. Publish

```bash
cargo publish
```

Needs a crates.io token (`cargo login`). This is the irreversible step.

## 10. After publishing

- **Check the docs.rs build.** It runs independently and can fail even when
  `cargo publish` succeeded — a feature combination that does not compile on
  docs.rs's target, for example. Watch <https://docs.rs/ph-eventing>; a failure
  shows in the build log there, not in your terminal.
- **Create the GitHub release** against the tag, pasting the changelog section.
- **Merge the release branch back into `master`** via its PR. Do not skip or
  defer this. Until it merges, the released state exists only on a branch: the
  tag is unreachable from `master`, `master`'s `Cargo.toml` still names the
  previous version, and every PR still based on `master` is being reviewed
  against a tree that is missing the release.
- **Open the next `## Unreleased`** heading in `CHANGELOG.md`, on `master`,
  after the merge-back.
- **Delete the release branch.** It has served its purpose and a stale release
  branch is an invitation to commit to the wrong one.

```bash
gh pr merge <N> --merge          # the release/X.Y.Z -> master PR
git switch master && git pull
git branch -d release/X.Y.Z && git push origin --delete release/X.Y.Z
```

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
