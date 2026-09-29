#!/usr/bin/env python3
"""Add THIRD-PARTY-NOTICES entries for crates linked into vktr that have none.

Walks the normal (non-dev, non-build) dependency graph of xai-grok-pager-bin for every
shipped target, finds registry and git crates without a `name version` entry in Part I,
and inserts entries in the upstream format, sorted with the existing ones. Existing
entries are left untouched; the curated sections (themes, in-tree ports, Part II) are
kept, and a missing Part II license text is appended from the crate's own license file.

Usage: scripts/update-third-party-notices.py [--check]
  --check  list missing crates and exit 1 if there are any; write nothing.
"""

import json
import re
import subprocess
import sys
import tomllib
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
NOTICES = ROOT / "THIRD-PARTY-NOTICES"
TARGETS = ["x86_64-unknown-linux-gnu", "aarch64-unknown-linux-gnu"]
BIN_PACKAGE = "xai-grok-pager-bin"
RULE = "-" * 80
HASH = "#" * 80
PART1_END = "=" * 80 + "\nPART I (continued)"
PART2_END = "\n\n" + "=" * 80 + "\nEND OF THIRD-PARTY NOTICES"
# Order in which a license is chosen from an OR expression (upstream prefers MIT).
PREFERENCE = ["MIT", "Apache-2.0", "BSD-3-Clause", "BSD-2-Clause", "ISC", "Zlib", "BSL-1.0",
              "Unicode-3.0", "CC0-1.0", "MPL-2.0"]
PLACEHOLDER = re.compile(r"\[yyyy\]|<year>|\{yyyy\}|\[name of copyright owner\]|<copyright holders>|"
                         r"\[fullname\]|<OWNER>", re.I)
# A real notice names a year or a holder right after the marker; license boilerplate such as
# "copyright license to reproduce" or "(c) You must retain" does not.
NOTICE_LINE = re.compile(r"^(?i:copyright\s*(\(c\)|©)?|\(c\)|©)\s*(?!You\b)(\d{4}|[A-Z][\w.-]*\s)")


def shipped_packages():
    pkgs = {}
    for target in TARGETS:
        md = json.loads(subprocess.check_output(
            ["cargo", "metadata", "--format-version", "1", "--locked", "--filter-platform", target],
            cwd=ROOT, stderr=subprocess.DEVNULL))
        by_id = {p["id"]: p for p in md["packages"]}
        nodes = {n["id"]: n for n in md["resolve"]["nodes"]}
        root = next(p["id"] for p in md["packages"] if p["name"] == BIN_PACKAGE)
        seen, stack = set(), [root]
        while stack:
            pid = stack.pop()
            if pid in seen:
                continue
            seen.add(pid)
            for dep in nodes[pid]["deps"]:
                if any(k["kind"] in (None, "normal") for k in dep["dep_kinds"]):
                    stack.append(dep["pkg"])
        for pid in seen:
            if by_id[pid].get("source"):  # skip workspace and path crates
                pkgs[pid] = by_id[pid]
    return list(pkgs.values())


def normalise(expr):
    return (expr or "").replace("/", " OR ").replace("MPL-2.0+", "MPL-2.0")


def choose_license(expr):
    if " AND " in expr or "WITH" in expr:
        return expr
    options = [o.strip("() ") for o in expr.split(" OR ")]
    for pref in PREFERENCE:
        if pref in options:
            return pref
    return options[0]


def license_files(pkg):
    crate_dir = Path(pkg["manifest_path"]).parent
    files = sorted(p for p in crate_dir.iterdir()
                   if p.is_file() and re.match(r"(LICEN[CS]E|COPYING|NOTICE|UNLICENSE)", p.name, re.I))
    if pkg.get("license_file"):
        extra = crate_dir / pkg["license_file"]
        if extra.is_file() and extra not in files:
            files.append(extra)
    return files


def copyright_lines(pkg):
    lines = []
    for f in license_files(pkg):
        for line in f.read_text(errors="replace").splitlines():
            s = line.strip()
            if NOTICE_LINE.match(s) and not PLACEHOLDER.search(s) and len(s) < 200 \
                    and s not in lines:
                lines.append(s)
    if not lines:
        authors = ", ".join(a for a in pkg.get("authors") or [] if a)
        lines.append(f"Copyright (c) {authors}" if authors else f"Copyright (c) The {pkg['name']} developers")
    return lines


def source_url(pkg):
    if pkg.get("repository"):
        return pkg["repository"]
    src = pkg.get("source") or ""
    if src.startswith("git+"):
        return src[4:].split("?")[0].split("#")[0]
    return f"https://crates.io/crates/{pkg['name']}/{pkg['version']}"


def entry(pkg):
    declared = pkg.get("license") or "see license file"
    expr = normalise(declared)
    chosen = choose_license(expr)
    head = f"License: {chosen}" + (f"  (upstream declares: {declared})" if declared != chosen else "")
    body = [RULE, f"{pkg['name']} {pkg['version']}", RULE, f"Source:  {source_url(pkg)}", head, "",
            "Copyright notice:", *[f"  {c}" for c in copyright_lines(pkg)], "",
            f"License text: see Part II — {chosen}", ""]
    if declared != chosen:
        body += ["Additional requirements / notices:",
                 f"  Upstream license expression: {declared}. For this distribution, "
                 f"obligations are satisfied under: {chosen}.", ""]
    return "\n".join(body) + "\n", chosen


def split_entries(section):
    parts = re.split(rf"(?m)^(?={RULE}\n\S+ \S+\n{RULE}\nSource:)", section)
    return parts[0], [p for p in parts[1:] if p.strip()]


def entry_key(block):
    name, version = block.split("\n")[1].split(" ", 1)
    return (name.lower(), version)


def license_line(block):
    chosen = next(line for line in block.split("\n") if line.startswith("License:"))
    return chosen.split("(")[0].strip()


def insert_sorted(blocks, block):
    """Insert after the last entry of the same crate, else before the first later name.

    Upstream's order is not strictly sorted, so existing entries are never reordered.
    """
    name = entry_key(block)[0]
    same = [i for i, b in enumerate(blocks) if entry_key(b)[0] == name]
    if same:
        blocks.insert(same[-1] + 1, block)
        return
    later = next((i for i, b in enumerate(blocks) if entry_key(b)[0] > name), len(blocks))
    blocks.insert(later, block)


def main():
    check = "--check" in sys.argv
    text = NOTICES.read_text()
    part1_start = text.index(RULE)
    part1_end = text.index(PART1_END)
    preamble, blocks = split_entries(text[part1_start:part1_end])
    have = {entry_key(b) for b in blocks}

    missing = sorted((p for p in shipped_packages() if (p["name"].lower(), p["version"]) not in have),
                     key=lambda p: (p["name"].lower(), p["version"]))
    for p in missing:
        print(f"missing: {p['name']} {p['version']} ({p.get('license')})")
    if check:
        sys.exit(1 if missing else 0)
    if not missing:
        print("THIRD-PARTY-NOTICES already covers every shipped crate")
        return

    needed_texts = {}
    for p in missing:
        template = next((b for b in reversed(blocks) if entry_key(b)[0] == p["name"].lower()), None)
        if template and license_line(template) == license_line(entry(p)[0]):
            # Same crate, same license: keep the curated entry (bundled-code notices and
            # all) and only move the version.
            lines = template.split("\n")
            lines[1] = f"{p['name']} {p['version']}"
            insert_sorted(blocks, "\n".join(lines))
            continue
        block, chosen = entry(p)
        insert_sorted(blocks, block)
        if f"\n{HASH}\n# {chosen}\n{HASH}\n" not in text and " " not in chosen:
            needed_texts.setdefault(chosen, p)
    blocks = [b if b.endswith("\n\n") else b.rstrip("\n") + "\n\n" for b in blocks]
    text = text[:part1_start] + preamble + "".join(blocks) + text[part1_end:]

    for lic, p in needed_texts.items():
        files = license_files(p)
        if not files:
            sys.exit(f"no license file in {p['name']} {p['version']} to supply the {lic} text")
        section = f"\n\n\n{HASH}\n# {lic}\n{HASH}\n\n{files[0].read_text(errors='replace').rstrip()}\n"
        cut = text.index(PART2_END)
        text = text[:cut] + section + text[cut:]
        print(f"added Part II text: {lic} (from {p['name']} {p['version']})")

    NOTICES.write_text(text)
    print(f"added {len(missing)} entries")


if __name__ == "__main__":
    main()
