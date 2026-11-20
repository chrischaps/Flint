#!/usr/bin/env python3
"""The root CLAUDE.md's budget, and its map of the scoped files (#992). `scripts/ci.sh` runs it.

    python3 scripts/claude-md-check.py            # check; exit 1 on a breach
    python3 scripts/claude-md-check.py --report   # every section's size too

Claude Code reads the root CLAUDE.md into every session in this repo, so every
byte of it is paid for by every session, whatever the session is about. A
scoped CLAUDE.md (one in the directory it describes) is read only when a
session works on files under it. #755 moved the root's feature notes into
scoped files (96 KB to 18 KB); with nothing holding the total it was back at
35 KB within two weeks, a bullet at a time, each right for its ticket.

1. **The root is under `BUDGET` bytes**, counted as git stores it (LF, so a
   Windows checkout's CRLF doesn't count). Over it, move detail into the
   scoped file for the code and leave a line in the root; don't raise the
   number in passing. `scripts/CLAUDE.md` says how the number was chosen.
2. **Every scoped CLAUDE.md is in the root's "Where the detail lives" table**,
   and every path the table names exists, so what moved out stays findable.
   A row may name several with braces: `crates/flint-{physics,audio}/CLAUDE.md`.

Standard library only: CI runs it before anything is installed.
"""

from __future__ import annotations

import re
import subprocess
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
FILE = ROOT / "CLAUDE.md"
INDEX = "## Where the detail lives"

# What every session needs, measured at #992: the claims section (vendored,
# kept byte for byte) 3.5 KB, the crate map 2.8 KB, this index 1.8 KB and
# the overview 0.4 KB make 8.5 KB; build commands, the edit -> validate ->
# play loop, the cross-cutting rules and gotchas fill the rest. #755's target.
BUDGET = 15_000

ROW_RE = re.compile(r"^\|\s*`([^`]+CLAUDE\.md)`\s*\|")
# A row may name a file in another repository cloned into the checkout: the
# examples' (#984, gitignored here). It need not be cloned for the check to pass.
OTHER_REPOS = ("flint-examples/",)


def expand(path: str) -> list[str]:
    """`a/{b,c}/d` -> [`a/b/d`, `a/c/d`] (one brace group is all the table uses)."""
    m = re.search(r"\{([^{}]*)\}", path)
    if not m:
        return [path]
    out = []
    for part in m.group(1).split(","):
        out += expand(path[: m.start()] + part.strip() + path[m.end() :])
    return out


def sections(text: str) -> list[tuple[str, int]]:
    parts = re.split(r"(?m)^(?=## )", text)
    return [(p.splitlines()[0] if p else "", len(p.encode("utf-8"))) for p in parts]


def index_section(text: str) -> list[str]:
    lines = text.splitlines()
    try:
        start = lines.index(INDEX)
    except ValueError:
        return []
    body = []
    for line in lines[start + 1 :]:
        if line.startswith("## "):
            break
        body.append(line)
    return body


def scoped_files() -> list[str]:
    out = subprocess.run(["git", "ls-files", "*CLAUDE.md"], cwd=ROOT, capture_output=True, text=True, check=True)
    # A game kept here with the engine as a subtree (engine/) is not this repo's map.
    return [f for f in out.stdout.split() if f != "CLAUDE.md" and not f.startswith("engine/")]


def main() -> int:
    report = "--report" in sys.argv[1:]
    text = FILE.read_bytes().replace(b"\r\n", b"\n").decode("utf-8")
    size = len(text.encode("utf-8"))
    problems = []

    if size > BUDGET:
        problems.append(
            f"CLAUDE.md is {size:,} bytes, {size - BUDGET:,} over its budget of {BUDGET:,} (#992).\n"
            "  Every session reads all of it. Move the detail into the scoped CLAUDE.md for the code it\n"
            "  describes (the table under \"Where the detail lives\") and leave a line here pointing at it;\n"
            "  don't raise BUDGET in passing (scripts/CLAUDE.md, \"The root CLAUDE.md's budget\")."
        )
    if report or size > BUDGET:
        print(f"claude-md-check: CLAUDE.md {size:,} / {BUDGET:,} bytes, by section:", file=sys.stderr if size > BUDGET else sys.stdout)
        for heading, n in sorted(sections(text), key=lambda s: -s[1]):
            print(f"  {n:6,}  {heading}", file=sys.stderr if size > BUDGET else sys.stdout)

    rows = [m.group(1) for line in index_section(text) if (m := ROW_RE.match(line))]
    if not rows:
        problems.append(f"CLAUDE.md has no `{INDEX}` table of scoped files")
    listed = set()
    for row in rows:
        for path in expand(row):
            listed.add(path)
            if not path.startswith(OTHER_REPOS) and not (ROOT / path).is_file():
                problems.append(f"CLAUDE.md's `{INDEX}` table names {path}, which isn't there")
    for path in scoped_files():
        if path not in listed:
            problems.append(
                f"{path} is not in CLAUDE.md's `{INDEX}` table: add a row saying what it covers,\n"
                "  so a session that hasn't opened a file under it knows it is there"
            )

    if problems:
        for p in problems:
            print(f"claude-md-check: {p}", file=sys.stderr)
        return 1
    print(f"claude-md-check: CLAUDE.md {size:,} / {BUDGET:,} bytes; {len(listed)} scoped files, all in its table")
    return 0


if __name__ == "__main__":
    sys.exit(main())
