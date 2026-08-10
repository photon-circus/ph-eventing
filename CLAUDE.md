# CLAUDE.md

See **[AGENTS.md](AGENTS.md)** — the canonical guidance for agents working in
this repository. This file exists only so tools that look for `CLAUDE.md` by
name find their way there; it intentionally holds no content of its own, so
there is nothing here to drift out of sync.

Two things worth knowing before you touch anything:

- **Remote CI is a subset, not the gate.** GitHub Actions runs on push and PR,
  but it has no coverage, Miri, or Loom job. Run `./scripts/ci.sh` locally regardless
  (Git Bash on Windows); a green check does not mean a clean matrix.
- **`SeqRing` and `EventBuf` are lock-free.** A green `cargo test` on x86 is
  weak evidence for them. After changing any atomic, ordering, fence, or unsafe
  block, run `./scripts/miri.sh` and `./scripts/loom.sh` as well.

Details, invariants, and the traps that silently invalidate the checkers are in
[AGENTS.md](AGENTS.md).
