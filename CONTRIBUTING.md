# Contributing

Issues and pull requests are welcome.

- Behaviour is specified in `.facts`; a change that adds or alters behaviour should update the
  matching fact. `.facts` is a fact sheet, not a test runner: its commands are static checks
  (greps, file and syntax checks) and `facts check` finishes in seconds. A fact backed by a test
  checks that the test exists; `scripts/test.sh` runs those tests and the smoke scripts
  (after `cargo build --release -p xai-grok-pager-bin`), and `cargo test --workspace` runs the rest.
- Keep upstream crate names and layout (`crates/codegen/xai-grok-*`) so diffs against Grok Build
  stay reviewable.
- Upstream updates arrive as merges from the `vendor/grok-build-rebranded` branch through
  `scripts/upstream.py` (procedure: `.agents/skills/update-upstream/SKILL.md`). Don't squash or
  rebase `main` past those merges, and don't edit the vendor branches by hand.
- Report security issues privately as described in [`SECURITY.md`](SECURITY.md).

## Licensing of contributions

vktr is licensed under the Apache License, Version 2.0 (see [`LICENSE`](LICENSE)). Unless you
state otherwise, any contribution you submit is licensed under the same terms, as described in
section 5 of the license. No separate contributor license agreement is required.
