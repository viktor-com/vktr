---
name: update-upstream
description: Move vktr's baseline to a newer upstream Grok Build sync without losing vktr's customisations, using scripts/upstream.py and the vendor branches. Use when asked to update, sync, re-vendor or rebase on upstream Grok Build, to check how far behind upstream vktr is, or when changing scripts/rebrand.pl or scripts/rebrand.exclude.
---

# Update the upstream Grok Build base

vktr is a fork of Grok Build (`github.com/xai-org/grok-build`). Upstream publishes the whole tree
as one "Synced from monorepo" commit every few days (each with a `Source-Revision:` trailer and a
list of changes). vktr follows it by merging, never by re-copying the tree or cherry-picking.

## How the tracking works

| Ref | Contents |
| --- | --- |
| `vendor/grok-build` | One commit per imported upstream sync. The tree is byte-identical to upstream's (the import verifies the tree hash). |
| `vendor/grok-build-rebranded` | The same trees after `scripts/rebrand.pl`, skipping the globs in `scripts/rebrand.exclude`. Trailers record `Upstream-Commit`, `Source-Revision`, `Pristine-Commit` and the hashes of the rebrand rules used. |
| `main` | Descends from `vendor/grok-build-rebranded` through merges. `SOURCE_REV` and `NOTICE` name the vendored `Source-Revision`. |

Merging the rebranded branch means git replays only what upstream changed since the last import.
Renames never show up as conflicts. Conflicts appear only where vktr changed the same lines.

`scripts/upstream.py` does the mechanics (Python stdlib, network only for fetching upstream).
It caches the upstream clone at `~/.cache/vktr-upstream/grok-build.git` (override with
`VKTR_UPSTREAM_CACHE`).

| Command | Does |
| --- | --- |
| `status` | Shows the imported base, the upstream head, the syncs not yet imported (with change counts), and whether `HEAD` contains the base |
| `import [--rev REV]` | Snapshots upstream `main` (or `REV`) onto both vendor branches. Refuses uncommitted rebrand rules, and refuses when upstream history no longer descends from the base |
| `merge [--branch NAME]` | Creates `upstream-sync/<date>-<rev>` off `HEAD`, merges the rebranded head, bumps `NOTICE`, and prepares a merge message listing upstream's changes. Commits when the merge is clean |
| `check-rebrand [-v]` | Compares the rebranded tree with `HEAD`. "Over-applied" files (the rebrand changes text `HEAD` keeps as upstream) must be 0 |
| `verify` | Offline check: `HEAD` contains the base, `SOURCE_REV` and `NOTICE` match it, nothing is over-applied. It backs a `.facts` entry |

## Procedure

1. **Start clean.** Run `git status` on an up-to-date `main`. If only `origin` has the vendor
   branches, create local ones: `git branch vendor/grok-build origin/vendor/grok-build`, and the
   same for `vendor/grok-build-rebranded`.
2. **See what's coming.** Run `python3 scripts/upstream.py status`, then read upstream's change
   lists with `git -C ~/.cache/vktr-upstream/grok-build.git log <imported base>..main`. The merge
   message repeats them later. Flag changes that touch areas vktr removed or replaced: login and auth,
   telemetry and analytics, self-update and installers, trace upload, remote or Agent Host
   features, toolset defaults, prompts.
3. **Import.** Run `python3 scripts/upstream.py import`. It imports upstream `main`, or pass
   `--rev <sha>` to stop at an older sync. Importing several syncs at once is fine: the merge
   message lists every skipped sync's changes.
4. **Merge.** Run `python3 scripts/upstream.py merge`. It lists conflicts by kind.
5. **Resolve conflicts** using the rules below, then `git commit` (the message is prepared).
6. **Audit for new xAI surfaces.** Upstream code merges in silently where vktr didn't touch the
   file. Review
   `git diff <old rebranded>..vendor/grok-build-rebranded | grep -nE 'x\.ai|grok\.com|googleapis|mixpanel|otel|telemetry|analytics|upload|auto.?update|winget|brew'`
   and neutralise anything that would contact xAI or run xAI-only flows. This follows the `.facts`
   domain rule: no code path reached in normal use contacts xAI services.
7. **Finish the bookkeeping** (details below): version, `Cargo.lock`, `THIRD-PARTY-NOTICES`,
   `CHANGELOG.md`, `.facts`.
8. **Run the checks** (below). Fix failures on the sync branch with ordinary commits after the merge.
9. **Hand over.** Report the conflicts and how you resolved them, audit findings, and check
   results. Merging the sync branch into `main` and pushing (`git push origin main
   vendor/grok-build vendor/grok-build-rebranded`) change shared state, so confirm with the user
   first. Always push the vendor branches together with the `main` that contains them.

## Resolving conflicts

Keep vktr's intent and upstream's new behaviour together. Taking one whole side is correct only
when the other side's change doesn't matter.

- **Lockstepped `version = "..."` lines** (`xai-grok-version`, `-shell`, `-pager`, `-pager-bin`
  `Cargo.toml`): keep vktr's scheme, `<upstream version>-vktr.<n>`. Take upstream's number and
  restart the suffix at `.1` for the next release, e.g. `1.0.41-vktr.1`. Then
  `git grep -n '<old version>'` and update the deliberate hits (`CHANGELOG.md` headings stay
  historical).
- **`Cargo.lock`:** take either side (`git checkout --theirs Cargo.lock`), then run
  `cargo metadata --format-version 1 >/dev/null` (or any build) to regenerate it. Review
  `git diff` for unexpected dependency changes.
- **Upstream added next to a vktr edit** (test lists in `Cargo.toml`, docs tables, test modules):
  keep both. Drop upstream additions only for things vktr deliberately removed. `.facts` lists the
  removed tests and features; keep them removed and delete upstream's new references to them.
- **Both changed the same logic:** rewrite the hunk so it has both behaviours, e.g. upstream's new
  `read_env_var` plus vktr's signed-in key fallback, or upstream's headless signal handling plus
  vktr's error-reported wrapper. Read the upstream change-list line that caused it (the merge
  message) to know what upstream intended.
- **Deleted by vktr, modified upstream:** keep it deleted unless the upstream change is something
  vktr now needs. Check `.facts` for why it was removed.
- **User guide** (`crates/codegen/xai-grok-pager/docs/user-guide/`): keep vktr's wording and
  merge in upstream's new facts. Drop sections about xAI-only features.
- **Brand residue:** upstream's new text is already rebranded. If a conflict shows `grok` or
  `Grok` in new upstream text, the rebrand rules missed a case. Fix the rules (next section)
  rather than hand-editing many files.

## Changing the rebrand rules

`scripts/rebrand.pl` (rename rules) and `scripts/rebrand.exclude` (paths left as upstream) must
reproduce `main`. When you change either one:

1. Commit the change on `main`.
2. Run `python3 scripts/upstream.py import --rev <current base Upstream-Commit>`. This adds a
   "Re-apply rebrand rules" commit on the rebranded branch only.
3. Run `python3 scripts/upstream.py check-rebrand`. Over-applied must be 0. Customised files should
   not grow; if they do, a glob or rule is too broad (`-v` lists them).
4. Merge it into `main` like an upstream update. Where `main` already has the new text, it merges
   cleanly.

License, NOTICE and attribution files are never rebranded.

## Checks before handing over

Build first: `cargo build -p xai-grok-pager-bin --release` produces `target/release/vktr`, the
default binary for the smoke scripts. Then run:

- `python3 scripts/upstream.py verify`
- `facts check` (the behavioural spec; every de-xAI decision is a fact here). It is static and
  takes seconds; a failure usually means a merge dropped a vktr string or a test the fact names
- `scripts/test.sh`: the targeted tests behind the facts, the mock-Viktor smoke scripts
  (`smoke-m1`..`m4`, `smoke-acp.py`) and `check-help-text.sh` (no help screen names grok or x.ai),
  all against `target/release/vktr`. The first run after a merge recompiles, so expect minutes
- `cargo clippy --workspace --all-targets` and `cargo test --workspace` (long; follow any
  test-worker limits in the repo's agent rules on shared hosts)
- `python3 scripts/update-third-party-notices.py --check`. If it fails, run it without `--check`
  and commit the new entries
- `scripts/smoke-release.sh <binary>` when the update is headed for a release

Add a `CHANGELOG.md` entry under "Unreleased", e.g. "Based on Grok Build <date> (Source-Revision
<short>)", plus user-visible upstream changes worth naming.

## Never

- **Squash or rebase `main` past a merge from the vendor branch.** That drops the ancestry and
  the next merge sees unrelated histories. If it happens anyway, and `main` already contains
  everything from the current rebranded head, re-link once without changing files:
  `git merge -s ours --allow-unrelated-histories vendor/grok-build-rebranded`. Then run `verify`.
- **Edit, rebase or force-push the vendor branches by hand.** Only `scripts/upstream.py import`
  writes them.
- **Merge `vendor/grok-build` (pristine) into `main`.** Only the rebranded branch is merged.
- **Copy upstream files over `main`, or cherry-pick hunks from upstream syncs.** That loses the
  merge base this setup exists for.
