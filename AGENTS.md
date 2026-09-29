# Agent notes

## The fact sheet

`.facts` is the behavioural spec: a fact sheet, not a test runner.

- Start and end work with it: `facts list` (or `facts list --section <s>`) to orient, then add or
  edit the facts your change makes true, and tag them `@implemented` when done.
- Fact commands are static checks (grep, file and syntax checks) that finish in milliseconds.
  They never build, test or run vktr: no `cargo`, no smoke scripts, no `target/` binary. A fact
  backed by a test checks that the test exists; it does not run it. A guard fact (`ntp`) fails if
  a slow command is added.
- Verify only the facts you touched: `facts get <id>`, or `facts check --tags <tag>`. A bare
  `facts check` audits the whole sheet; it is cheap, but it is not how you prove behaviour.
- This overrides the facts CLI's `facts-implement` skill where it says "validation commands are
  the tests" and "do not write separate tests": here behaviour is proven by real tests.

## Proving behaviour

- `scripts/test.sh [pattern]` runs the targeted cargo tests and mock-Viktor smoke scripts behind
  the facts. The smoke steps need `cargo build --release -p xai-grok-pager-bin` first.
- `cargo test -p <crate>` for the crate you changed; `cargo test --workspace` only before a
  release or after an upstream merge (it is long).
- After touching many files (an upstream merge, a branch switch) the first build recompiles for
  minutes. That is expected, not a hang.

## Upstream

Upstream Grok Build arrives by merge from `vendor/grok-build-rebranded`; see
`.agents/skills/update-upstream/SKILL.md`. Never squash or rebase `main` past those merges.
