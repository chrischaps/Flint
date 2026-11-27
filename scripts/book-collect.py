"""A package's book pages live in its folder; this collects them (#1563).

A page about a package is kept beside its code, in `packages/<name>/docs/`,
at the path it has in the book: `packages/music/docs/concepts/music-sessions.md`
is the book's `concepts/music-sessions.md`, so its URL and its relative links
are what they were when it lived in `docs/book/src/`. This copies every file
under each package's `docs/` into `docs/book/src/` before `mdbook build
docs/book` (`docs/publish.sh` runs it), and `SUMMARY.md` stays the book's one
table of contents.

The copies are build output: the root `.gitignore` lists each one between the
`# book-collect:begin` and `# book-collect:end` lines, which this writes. Edit
a page in its package's folder; a copy edited by hand is refused (newer than
its source and different), not overwritten.

    python scripts/book-collect.py           # copy, and write the .gitignore block
    python scripts/book-collect.py --check   # change nothing; exit 1 on a problem
    python scripts/book-collect.py --force   # overwrite a copy edited by hand

`--check` (in scripts/ci.sh; docs-check.py runs the same checks) fails on a
page two packages give the same book path, a package page whose book path is a
tracked file in docs/book/src (a page with two homes), a page SUMMARY.md
doesn't link (mdBook would leave it out) and a stale .gitignore block.

`pages()` is what docs-check.py reads the package pages through. The Rust
side, `flint script api --book` / `--check-book`, finds them the same way
(`crates/flint-cli/src/commands/script/book.rs`, `package_pages`).
"""

import argparse
import re
import shutil
import subprocess
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
BOOK = ROOT / "docs" / "book" / "src"
PACKAGES = ROOT / "packages"
GITIGNORE = ROOT / ".gitignore"
BEGIN = "# book-collect:begin"
END = "# book-collect:end"
HEADER = [
    "# The book's pages kept in a package's folder (#1563): scripts/book-collect.py",
    "# copies packages/<name>/docs/<path> to docs/book/src/<path> and writes this",
    "# list. Edit a page in its package's folder, never the copy.",
]
SUMMARY_LINK_RE = re.compile(r"\]\(([^)#\s]+\.md)(?:#[^)]*)?\)")


def pages(root=ROOT):
    """Every file under a package's `docs/`, as (book path, package, source):
    the book path is the file's path under `docs/`, with `/`, sorted by it."""
    out = []
    for pkg in sorted((root / "packages").iterdir()) if (root / "packages").is_dir() else []:
        docs = pkg / "docs"
        if not docs.is_dir():
            continue
        for f in sorted(docs.rglob("*")):
            if f.is_file():
                out.append((f.relative_to(docs).as_posix(), pkg.name, f))
    out.sort(key=lambda p: (p[0], p[1]))
    return out


def tracked_in_book(root=ROOT):
    """The files git tracks under docs/book/src, as paths under it."""
    out = subprocess.run(["git", "ls-files", "-z", "--", "docs/book/src"], cwd=root,
                         capture_output=True, text=True, encoding="utf-8")
    if out.returncode != 0:
        raise SystemExit(f"book-collect: git ls-files failed: {out.stderr.strip()}")
    prefix = "docs/book/src/"
    return {p[len(prefix):] for p in out.stdout.split("\0") if p.startswith(prefix)}


def summary_links(root=ROOT):
    """The pages SUMMARY.md links, as paths under src/."""
    summary = root / "docs" / "book" / "src" / "SUMMARY.md"
    text = summary.read_text(encoding="utf-8") if summary.is_file() else ""
    return {m.group(1).removeprefix("./") for m in SUMMARY_LINK_RE.finditer(text)}


def block_lines(found):
    return [BEGIN, *HEADER, *(f"/docs/book/src/{book}" for book, _, _ in found), END]


def split_gitignore(text):
    """(before, the block's lines or None, after), lines without endings."""
    lines = text.splitlines()
    if BEGIN not in lines:
        return lines, None, []
    a = lines.index(BEGIN)
    b = lines.index(END, a) if END in lines[a:] else len(lines) - 1
    return lines[:a], lines[a:b + 1], lines[b + 1:]


def problems(root=ROOT, found=None):
    """What --check fails on, as (file, message, hint)."""
    found = pages(root) if found is None else found
    out = []
    seen = {}
    for book, pkg, src in found:
        if book in seen:
            out.append((f"packages/{pkg}/docs/{book}", f"`{seen[book]}` gives the book `{book}` too",
                        "a book path has one home: move one of the two"))
        seen.setdefault(book, pkg)
    tracked = tracked_in_book(root)
    for book, pkg, _ in found:
        if book in tracked:
            out.append((f"docs/book/src/{book}", f"tracked here and in packages/{pkg}/docs/{book}",
                        f"the page lives in its package's folder: `git rm` this copy and edit packages/{pkg}/docs/{book}"))
    linked = summary_links(root)
    for book, pkg, _ in found:
        if book.endswith(".md") and book not in linked:
            out.append((f"packages/{pkg}/docs/{book}", "SUMMARY.md doesn't link it, so mdBook leaves it out",
                        f"add `[Title]({book})` to docs/book/src/SUMMARY.md, the book's one table of contents"))
    gi = root / ".gitignore"
    _, block, _ = split_gitignore(gi.read_text(encoding="utf-8") if gi.is_file() else "")
    if block != block_lines(found):
        out.append((".gitignore", "the book-collect block doesn't list the collected pages",
                    "run `python scripts/book-collect.py`, which writes it, and commit .gitignore"))
    return out


def write_gitignore(found, root=ROOT):
    """Write the block, in the file's own line endings; True when it changed."""
    gi = root / ".gitignore"
    raw = gi.read_bytes() if gi.is_file() else b""
    text = raw.decode("utf-8")
    nl = "\r\n" if "\r\n" in text else "\n"
    before, old, after = split_gitignore(text)
    if old is None and before and before[-1].strip():
        before = [*before, ""]
    lines = [*before, *block_lines(found), *after]
    new = (nl.join(lines) + nl).encode("utf-8")
    if new == raw:
        return False, old or []
    gi.write_bytes(new)
    return True, old or []


def collect(root=ROOT, force=False):
    found = pages(root)
    hard = [p for p in problems(root, found) if p[0] != ".gitignore"]
    if hard:
        return hard, []
    book = root / "docs" / "book" / "src"
    refused = []
    plan = []
    for rel, pkg, src in found:
        dest = book / rel
        if dest.is_file():
            same = dest.read_bytes() == src.read_bytes()
            if same:
                continue
            if dest.stat().st_mtime > src.stat().st_mtime and not force:
                refused.append((f"docs/book/src/{rel}", "the copy was edited after its source and differs from it",
                                f"move the edit to packages/{pkg}/docs/{rel} (the page's home), or run with --force to drop it"))
                continue
        plan.append((dest, src))
    if refused:
        return refused, []
    changed, old = write_gitignore(found, root)
    done = []
    for dest, src in plan:
        dest.parent.mkdir(parents=True, exist_ok=True)
        shutil.copyfile(src, dest)
        done.append(dest.relative_to(root).as_posix())
    # A copy whose page has left its package (the old block named it)
    now = {f"/docs/book/src/{rel}" for rel, _, _ in found}
    for line in old:
        if line.startswith("/docs/book/src/") and line not in now:
            stale = root / line.lstrip("/")
            if stale.is_file():
                stale.unlink()
                done.append(f"removed {line.lstrip('/')}")
    if changed:
        done.append("wrote .gitignore's book-collect block")
    return [], done


def main():
    ap = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    ap.add_argument("--check", action="store_true", help="change nothing; exit 1 on a problem")
    ap.add_argument("--force", action="store_true", help="overwrite a copy edited by hand")
    args = ap.parse_args()
    if args.check:
        found = problems()
        done = []
    else:
        found, done = collect(force=args.force)
    for f, message, hint in found:
        print(f"{f}: {message}\n    {hint}", file=sys.stderr)
    for d in done:
        print(f"book-collect: {d}")
    n = len(pages())
    if found:
        print(f"book-collect: {len(found)} problem(s)", file=sys.stderr)
        return 1
    print(f"book-collect: {n} file(s) from packages/*/docs/" + (" checked" if args.check else " in docs/book/src"))
    return 0


if __name__ == "__main__":
    sys.exit(main())
