#!/usr/bin/env python3
"""Track upstream Grok Build through two vendor branches and merge it into vktr.

Branches (local and on origin):
  vendor/grok-build            one commit per imported upstream sync, tree byte-identical to upstream
  vendor/grok-build-rebranded  the same trees after scripts/rebrand.pl, minus scripts/rebrand.exclude

main merges vendor/grok-build-rebranded, so a merge only replays what upstream changed since the
last import, and conflicts appear only where vktr customised the same lines.

Commands:
  status                 current base, upstream head, and syncs not yet imported
  import [--rev REV]     snapshot an upstream commit onto both vendor branches (default: upstream main)
  check-rebrand [--rev]  compare a rebranded vendor commit with HEAD; lists files the rebrand gets wrong
  merge [--branch NAME]  branch off HEAD, merge the rebranded vendor head, bump NOTICE
  verify                 offline check that HEAD contains the vendor base and records it in SOURCE_REV/NOTICE

The upstream clone is cached (blobless) at $VKTR_UPSTREAM_CACHE or ~/.cache/vktr-upstream/grok-build.git.
"""

import argparse
import datetime
import fnmatch
import os
import re
import subprocess
import sys
import tempfile
from pathlib import Path

UPSTREAM_URL = "https://github.com/xai-org/grok-build.git"
UPSTREAM_BRANCH = "main"
PRISTINE = "vendor/grok-build"
REBRANDED = "vendor/grok-build-rebranded"
REBRAND_SCRIPT = "scripts/rebrand.pl"
REBRAND_EXCLUDE = "scripts/rebrand.exclude"

ROOT = Path(__file__).resolve().parent.parent
CACHE = Path(os.environ.get("VKTR_UPSTREAM_CACHE", Path.home() / ".cache/vktr-upstream/grok-build.git"))


def git(*args, cwd=ROOT, env=None, check=True, input=None):
    res = subprocess.run(
        ["git", *args], cwd=cwd, env=env, input=input, capture_output=True, text=True
    )
    if check and res.returncode != 0:
        sys.exit(f"git {' '.join(args)} failed:\n{res.stderr.strip()}")
    return res.stdout.strip()


def up(*args, check=True):
    return git(*args, cwd=CACHE, check=check)


def ref(name, remote=True):
    """Local branch, falling back to origin's copy (fresh clones only fetch the vendor branches)."""
    for full in [f"refs/heads/{name}"] + ([f"refs/remotes/origin/{name}"] if remote else []):
        sha = git("rev-parse", "--verify", "-q", f"{full}^{{commit}}", check=False)
        if sha:
            return sha
    return None


def is_ancestor(a, b, cwd=ROOT):
    return subprocess.run(["git", "merge-base", "--is-ancestor", a, b], cwd=cwd).returncode == 0


def trailers(commit, cwd=ROOT):
    out = git("show", "-s", "--format=%(trailers:only,unfold)", commit, cwd=cwd)
    result = {}
    for line in out.splitlines():
        if ": " in line:
            key, value = line.split(": ", 1)
            result[key] = value
    return result


def fetch_upstream():
    if not CACHE.exists():
        CACHE.parent.mkdir(parents=True, exist_ok=True)
        print(f"cloning {UPSTREAM_URL} (blobless) into {CACHE}")
        subprocess.run(
            ["git", "clone", "--bare", "--filter=blob:none", UPSTREAM_URL, str(CACHE)], check=True
        )
    else:
        up("fetch", "--quiet", "origin", f"+refs/heads/{UPSTREAM_BRANCH}:refs/heads/{UPSTREAM_BRANCH}")


def current_base():
    head = ref(REBRANDED, remote=False) or ref(REBRANDED)
    return trailers(head) if head else {}


def upstream_info(commit):
    fmt = "%H%n%cs%n%(trailers:key=Source-Revision,valueonly,separator=)"
    sha, date, source_rev = (up("show", "-s", f"--format={fmt}", commit).splitlines() + [""])[:3]
    return {"commit": sha, "date": date, "source_rev": source_rev.strip()}


def pending_syncs(base_commit, target):
    rng = f"{base_commit}..{target}" if base_commit else target
    out = up("log", "--reverse", "--format=%H", rng)
    return [upstream_info(c) for c in out.splitlines() if c]


def change_notes(base_commit, target):
    """Upstream's per-sync change lists between base and target, oldest first."""
    rng = f"{base_commit}..{target}" if base_commit else f"{target}~1..{target}"
    notes = []
    for commit in up("log", "--reverse", "--format=%H", rng).splitlines():
        info = upstream_info(commit)
        body = up("show", "-s", "--format=%b", commit)
        items = [l for l in body.splitlines() if l.startswith("- ")]
        notes.append(f"Upstream sync {info['date']} (Source-Revision {info['source_rev'][:12]}):")
        notes.extend(items or ["(no change list)"])
        notes.append("")
    return "\n".join(notes).strip()


# ---------------------------------------------------------------- status

def cmd_status(_args):
    fetch_upstream()
    base = current_base()
    head = upstream_info(UPSTREAM_BRANCH)
    source_rev = (ROOT / "SOURCE_REV").read_text().strip()
    rebranded = ref(REBRANDED)
    print(f"upstream {UPSTREAM_URL} {UPSTREAM_BRANCH}: {head['commit'][:12]} "
          f"({head['date']}, Source-Revision {head['source_rev'][:12]})")
    if not base:
        print(f"no {REBRANDED} branch yet: run `import --rev <upstream commit>` for the current base")
        return
    print(f"imported base: {base['Upstream-Commit'][:12]} (Source-Revision {base['Source-Revision'][:12]})")
    merged = is_ancestor(rebranded, "HEAD")
    print(f"HEAD contains {REBRANDED}: {'yes' if merged else 'NO (run merge)'}")
    print(f"SOURCE_REV in the working tree: {source_rev[:12]}"
          + ("" if source_rev == base["Source-Revision"] or not merged else "  (does not match the base!)"))
    syncs = pending_syncs(base["Upstream-Commit"], UPSTREAM_BRANCH)
    if not syncs:
        print("up to date with upstream")
    for s in syncs:
        n = len([l for l in up("show", "-s", "--format=%b", s["commit"]).splitlines() if l.startswith("- ")])
        print(f"  pending: {s['commit'][:12]} {s['date']} Source-Revision {s['source_rev'][:12]} ({n} changes)")


# ---------------------------------------------------------------- import

def load_excludes(path):
    patterns = []
    for line in Path(path).read_text().splitlines():
        line = line.strip()
        if line and not line.startswith("#"):
            patterns.append(line)
    return patterns


def is_binary(path):
    with open(path, "rb") as f:
        return b"\0" in f.read(8192)


def rebrand_tree(tree_dir, script, excludes):
    targets = []
    for dirpath, dirnames, filenames in os.walk(tree_dir):
        dirnames[:] = [d for d in dirnames if d != ".git"]
        for name in filenames:
            full = os.path.join(dirpath, name)
            rel = os.path.relpath(full, tree_dir)
            if os.path.islink(full) or any(fnmatch.fnmatch(rel, p) for p in excludes):
                continue
            if not is_binary(full):
                targets.append(rel)
    for i in range(0, len(targets), 500):
        subprocess.run(["perl", script, *targets[i:i + 500]], cwd=tree_dir, check=True)
    return len(targets)


def write_tree(tree_dir, index_file):
    env = dict(os.environ, GIT_INDEX_FILE=str(index_file))
    if index_file.exists():
        index_file.unlink()
    git(f"--work-tree={tree_dir}", "add", "-A", "-f", ".", env=env)
    return git("write-tree", env=env)


def commit_tree(tree, parent, message, date):
    env = dict(os.environ, GIT_AUTHOR_DATE=f"{date}T12:00:00Z")
    args = ["commit-tree", tree, "-F", "-"] + (["-p", parent] if parent else [])
    return git(*args, env=env, input=message)


def update_branch(name, new, old):
    git("update-ref", "-m", "upstream.py import", f"refs/heads/{name}", new, old or "0" * 40)


def cmd_import(args):
    fetch_upstream()
    target = upstream_info(args.rev or UPSTREAM_BRANCH)
    up_tree = up("rev-parse", f"{target['commit']}^{{tree}}")
    script, exclude = ROOT / REBRAND_SCRIPT, ROOT / REBRAND_EXCLUDE
    script_hash = git("hash-object", str(script))
    exclude_hash = git("hash-object", str(exclude))
    dirty = git("status", "--porcelain", "--", REBRAND_SCRIPT, REBRAND_EXCLUDE)
    if dirty and not args.allow_dirty:
        sys.exit(f"{REBRAND_SCRIPT} or {REBRAND_EXCLUDE} has uncommitted changes; commit them "
                 "first (or pass --allow-dirty for an experiment)")

    old_p, old_r = ref(PRISTINE, remote=False), ref(REBRANDED, remote=False)
    if not old_r and ref(REBRANDED):
        sys.exit(f"origin has {REBRANDED} but there is no local branch; run "
                 f"`git branch {PRISTINE} origin/{PRISTINE} && git branch {REBRANDED} origin/{REBRANDED}`")
    base = current_base()
    same_upstream = base.get("Upstream-Commit") == target["commit"]
    same_rules = base.get("Rebrand-Script") == script_hash and base.get("Rebrand-Exclude") == exclude_hash
    if same_upstream and same_rules:
        print(f"{target['commit'][:12]} is already imported with the current rebrand rules")
        return
    if base and not same_upstream:
        if not is_ancestor(base["Upstream-Commit"], target["commit"], cwd=CACHE):
            sys.exit(f"{target['commit'][:12]} does not descend from the imported base "
                     f"{base['Upstream-Commit'][:12]}; upstream history was rewritten, check by hand")

    subject_date = target["date"]
    short = target["source_rev"][:12] or target["commit"][:12]
    notes = change_notes(base.get("Upstream-Commit"), target["commit"])
    trailer = f"Upstream-Commit: {target['commit']}\nSource-Revision: {target['source_rev']}\n"

    with tempfile.TemporaryDirectory(prefix="vktr-upstream-") as tmp:
        tree_dir = Path(tmp) / "tree"
        tree_dir.mkdir()
        print(f"exporting upstream {target['commit'][:12]} ({subject_date})")
        archive = subprocess.Popen(["git", "archive", target["commit"]], cwd=CACHE, stdout=subprocess.PIPE)
        subprocess.run(["tar", "-x", "-C", str(tree_dir)], stdin=archive.stdout, check=True)
        if archive.wait() != 0:
            sys.exit("git archive failed")
        index = Path(tmp) / "index"
        pristine_tree = write_tree(tree_dir, index)
        if pristine_tree != up_tree:
            sys.exit(f"exported tree {pristine_tree} differs from upstream tree {up_tree}; aborting")

        new_p = old_p
        if not same_upstream:
            msg = f"Import upstream Grok Build {subject_date} (Source-Revision {short})\n\n"
            msg += (notes + "\n\n" if notes else "") + trailer
            new_p = commit_tree(pristine_tree, old_p, msg, subject_date)

        count = rebrand_tree(tree_dir, str(script), load_excludes(exclude))
        rebranded_tree = write_tree(tree_dir, index)
        what = "Rebrand" if not same_upstream else "Re-apply rebrand rules to"
        msg = f"{what} upstream Grok Build {subject_date} (Source-Revision {short})\n\n"
        msg += f"scripts/rebrand.pl over {count} text files of {PRISTINE} {new_p[:12]}.\n\n"
        msg += trailer + f"Pristine-Commit: {new_p}\nRebrand-Script: {script_hash}\nRebrand-Exclude: {exclude_hash}\n"
        new_r = commit_tree(rebranded_tree, old_r, msg, subject_date)

    if new_p != old_p:
        update_branch(PRISTINE, new_p, old_p)
    update_branch(REBRANDED, new_r, old_r)
    print(f"{PRISTINE}: {new_p[:12]}\n{REBRANDED}: {new_r[:12]}")
    print("next: `python3 scripts/upstream.py merge`" if base else
          "next: link main to this base once (see the update-upstream skill)")


# ---------------------------------------------------------------- check-rebrand

def ls_tree(commit):
    out = git("ls-tree", "-r", "-z", commit)
    entries = {}
    for item in out.split("\0"):
        if item:
            meta, path = item.split("\t", 1)
            entries[path] = meta.split()[2]
    return entries


def cmd_check_rebrand(args):
    rebranded = args.rev or ref(REBRANDED) or sys.exit(f"no {REBRANDED} branch")
    pristine = trailers(rebranded).get("Pristine-Commit")
    if not pristine:
        sys.exit(f"{rebranded} has no Pristine-Commit trailer")
    p, r, h = ls_tree(pristine), ls_tree(rebranded), ls_tree("HEAD")
    shared = [f for f in p if f in h]
    identical = [f for f in shared if r[f] == h[f]]
    over = [f for f in shared if h[f] == p[f] and r[f] != p[f]]
    custom = [f for f in shared if h[f] != r[f] and h[f] != p[f]]
    removed = [f for f in p if f not in h]
    print(f"upstream files: {len(p)}; in HEAD: {len(shared)}; removed by vktr: {len(removed)}")
    print(f"identical to the rebranded tree: {len(identical)}")
    print(f"customised on top of the rebranded tree: {len(custom)}")
    print(f"over-applied (HEAD keeps upstream text, the rebrand changes it): {len(over)}")
    for f in over[: args.limit]:
        print(f"  {f}")
    if args.verbose:
        for f in custom:
            print(f"  customised: {f}")
        for f in removed:
            print(f"  removed: {f}")
    sys.exit(1 if over else 0)


# ---------------------------------------------------------------- verify

def cmd_verify(_args):
    """Offline invariants: HEAD descends from the rebranded base and records it everywhere."""
    head = ref(REBRANDED)
    if not head:
        sys.exit(f"no {REBRANDED} branch (fetch it: `git fetch origin {PRISTINE} {REBRANDED}`)")
    t = trailers(head)
    problems = []
    if not is_ancestor(head, "HEAD"):
        problems.append(f"HEAD does not contain {REBRANDED} {head[:12]}; run `scripts/upstream.py merge`")
    source_rev = (ROOT / "SOURCE_REV").read_text().strip()
    if source_rev != t["Source-Revision"]:
        problems.append(f"SOURCE_REV {source_rev[:12]} != vendor Source-Revision {t['Source-Revision'][:12]}")
    if t["Source-Revision"] not in (ROOT / "NOTICE").read_text():
        problems.append("NOTICE does not name the vendored Source-Revision")
    p, r, h = ls_tree(t["Pristine-Commit"]), ls_tree(head), ls_tree("HEAD")
    over = [f for f in p if f in h and h[f] == p[f] and r[f] != p[f]]
    if over:
        problems.append(f"{len(over)} files are over-applied by the rebrand (see check-rebrand)")
    for line in problems:
        print(f"FAIL: {line}")
    if problems:
        sys.exit(1)
    print(f"ok: HEAD contains {REBRANDED} {head[:12]} (Source-Revision {t['Source-Revision'][:12]})")


# ---------------------------------------------------------------- merge

NOTICE_RE = re.compile(r"(Vendored from upstream commit )[0-9a-f]{40}(\s+\(upstream SOURCE_REV, )\d{4}-\d{2}-\d{2}(\))")


def cmd_merge(args):
    if git("status", "--porcelain", "--untracked-files=no"):
        sys.exit("working tree has uncommitted changes; commit or stash them first")
    head = ref(REBRANDED)
    if not head:
        sys.exit(f"no {REBRANDED} branch; run import first")
    if is_ancestor(head, "HEAD"):
        print(f"HEAD already contains {REBRANDED}; nothing to merge")
        return
    base = trailers(head)
    source_rev = base["Source-Revision"]
    fetch_upstream()
    date = upstream_info(base["Upstream-Commit"])["date"]
    branch = args.branch or f"upstream-sync/{date}-{source_rev[:8]}"
    git("switch", "-c", branch)

    prev = subprocess.run(["git", "merge-base", head, "HEAD"], cwd=ROOT, capture_output=True, text=True).stdout.strip()
    prev_upstream = trailers(prev).get("Upstream-Commit") if prev else None
    notes = change_notes(prev_upstream, base["Upstream-Commit"]) if prev_upstream else ""
    msg = f"Merge upstream Grok Build {date} (Source-Revision {source_rev[:12]})\n\n" + (notes + "\n" if notes else "")
    res = subprocess.run(["git", "merge", "--no-ff", "--no-commit", head], cwd=ROOT, capture_output=True, text=True)

    notice = ROOT / "NOTICE"
    text = notice.read_text()
    new_text, n = NOTICE_RE.subn(rf"\g<1>{source_rev}\g<2>{date}\g<3>", text)
    if n != 1:
        print("warning: NOTICE revision line not found; update it by hand")
    elif new_text != text:
        notice.write_text(new_text)
        git("add", "NOTICE")

    (ROOT / ".git" / "MERGE_MSG").write_text(msg)
    conflicts = git("diff", "--name-only", "--diff-filter=U").splitlines()
    if not conflicts and res.returncode == 0:
        git("commit", "-q", "-F", str(ROOT / ".git" / "MERGE_MSG"))
        print(f"merged cleanly on {branch}: {git('rev-parse', '--short', 'HEAD')}")
    else:
        print(f"merge on {branch} stopped with {len(conflicts)} conflicted files:")
        status = git("status", "--porcelain")
        for line in status.splitlines():
            code, path = line[:2], line[3:]
            if code in ("UU", "AA", "DU", "UD", "AU", "UA", "DD"):
                kind = {"UU": "both modified", "AA": "both added", "DU": "deleted by vktr",
                        "UD": "deleted upstream", "AU": "added by vktr", "UA": "added upstream",
                        "DD": "both deleted"}[code]
                print(f"  {kind:17} {path}")
        print("resolve them, then `git commit` (the merge message is prepared)")
    print("then run the checks listed in the update-upstream skill before merging into main")


def main():
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    sub = parser.add_subparsers(dest="cmd", required=True)
    sub.add_parser("status").set_defaults(fn=cmd_status)
    p = sub.add_parser("import")
    p.add_argument("--rev", help="upstream commit or ref (default: upstream main)")
    p.add_argument("--allow-dirty", action="store_true", help="use uncommitted rebrand rules")
    p.set_defaults(fn=cmd_import)
    p = sub.add_parser("check-rebrand")
    p.add_argument("--rev", help=f"rebranded vendor commit (default: {REBRANDED})")
    p.add_argument("--limit", type=int, default=50)
    p.add_argument("-v", "--verbose", action="store_true")
    p.set_defaults(fn=cmd_check_rebrand)
    sub.add_parser("verify").set_defaults(fn=cmd_verify)
    p = sub.add_parser("merge")
    p.add_argument("--branch", help="branch to create for the merge")
    p.set_defaults(fn=cmd_merge)
    args = parser.parse_args()
    args.fn(args)


if __name__ == "__main__":
    main()
