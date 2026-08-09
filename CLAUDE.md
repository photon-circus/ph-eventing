# CLAUDE.md

See **[AGENTS.md](AGENTS.md)** — the canonical guidance for agents working in
this repository. This file exists only so tools that look for `CLAUDE.md` by
name find their way there; it intentionally holds no content of its own, so
there is nothing here to drift out of sync.

Two things worth knowing before you touch anything:

- **CI runs locally.** The GitHub Actions workflow is manual-dispatch only, so
  nothing checks a branch on push. Run `./scripts/ci.sh` (or `ci.ps1`).
- **`SeqRing` and `EventBuf` are lock-free.** A green `cargo test` on x86 is
  weak evidence for them. After changing any atomic, ordering, fence, or unsafe
  block, run `./scripts/miri.sh` and `./scripts/loom.sh` as well.

Details, invariants, and the traps that silently invalidate the checkers are in
[AGENTS.md](AGENTS.md).
