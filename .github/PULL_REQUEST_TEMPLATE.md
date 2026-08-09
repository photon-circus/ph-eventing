## What and why

<!-- What changes, and what problem it solves. Link an issue if there is one. -->

## Verification

**CI does not run automatically on this repository** — the workflow is
manual-dispatch only. These local runs are the only gate, so please paste or
confirm the results rather than assuming.

- [ ] `./scripts/ci.sh` (or `ci.ps1`) — all checks pass, **no `SKIP` lines**
      (a skipped check is not a passed check; install the tool and re-run)

If this touches atomics, orderings, fences, `unsafe`, or anything in
`seq_ring.rs` / `event_buf.rs` / `sync.rs`:

- [ ] `./scripts/miri.sh` — clean
- [ ] `./scripts/loom.sh` — all models verified
- [ ] Ordering changes are justified in a comment, not just in this PR

If this touches `Cargo.toml`:

- [ ] `cargo deny check` passes
- [ ] `cargo tree` still shows zero runtime dependencies
- [ ] `cargo package --list` contains what it should — the `include` allowlist
      means a new file is unpublished by default

## Notes for the reviewer

<!--
Anything worth flagging: a claim you could not verify, a caveat you chose to
document rather than fix, a trade-off you are unsure about. `SeqRing` has one
known, documented Miri deviation — see its module docs before reporting it as
new.
-->
