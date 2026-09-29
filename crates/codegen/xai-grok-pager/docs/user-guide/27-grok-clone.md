# vktr clone

`vktr clone` is not available in vktr. Upstream Grok Build fetched repositories into a Grove content store (an xAI-internal daemon that mounts a projected working tree over NFS or FUSE); vktr ships no Grove client and has no `clone` subcommand.

Use `git clone` instead. For isolated work on an existing repository, use git worktrees: `vktr -w` / `vktr --worktree=<name>` starts a session in a new worktree, and `vktr worktree` manages them.
