#!/usr/bin/env python3
"""The book and the CLAUDE.md files against the code (#375).

Three mechanical checks, the ones a page drifts on without anyone noticing:

  flags   every `--flag` on a `flint ...` command line (in a code block or a
          code span) is one that command's `--help` lists, and every
          subcommand named is real. In docs/book/src/cli-reference/, a table
          row whose first cell is a `--flag`, under a heading that names a
          command ("The `render` Command", "`flint play-chart`"), is checked
          against that command too.
  styles  every `[post_process.styles.<name>]` names a registered style
          (`flint render --list-styles --json`).
  images  every image a book page links (`![..](path)`, `<img src=..>`)
          exists on disk.

Usage:
  python scripts/docs-check.py [--flint <path to flint binary>] [--json]

Without --flint it looks for target/debug/flint, then target/release/flint;
build one with `cargo build -p flint-cli --no-default-features --bin flint`.
Exit 1 when anything is found. What isn't drift (a user style a page
invents to show the format) goes in docs/book/docs-check-allow.txt.
"""

import argparse
import json
import os
import re
import shlex
import subprocess
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
BOOK = ROOT / "docs" / "book" / "src"
ALLOW = ROOT / "docs" / "book" / "docs-check-allow.txt"

COUNTS = {"flags": 0, "styles": 0, "images": 0}
FLAG_RE = re.compile(r"^--[a-z0-9][a-z0-9-]*")
HELP_FLAG_RE = re.compile(r"^\s+(?:-[A-Za-z0-9], )?(--[a-z0-9][a-z0-9-]*)")
HELP_CMD_RE = re.compile(r"^  ([a-z][a-z0-9-]*)(?:\s|$)")
STYLE_RE = re.compile(r"\[post_process\.styles\.([A-Za-z0-9_]+)\]")
IMAGE_MD_RE = re.compile(r"!\[[^\]]*\]\(\s*<?([^)\s>]+)>?(?:\s+\"[^\"]*\")?\s*\)")
IMAGE_HTML_RE = re.compile(r"<img\b[^>]*?\bsrc\s*=\s*[\"']([^\"']+)[\"']", re.I)
SPAN_RE = re.compile(r"(`+)(.+?)\1")
FENCE_RE = re.compile(r"^\s*(```+|~~~+)(.*)$")
# A heading that names the command its tables describe.
HEADING_CMD_RES = [
    re.compile(r"^The `([a-z][a-z0-9 -]*)` Command"),
    re.compile(r"^`flint ([a-z][a-z0-9 -]*)`"),
]
# Shell words that end the flint invocation on a line.
STOP_WORDS = {"|", "||", "&&", ";", ">", ">>", "2>", "2>&1", "&"}
# A code block in one of these languages holds no shell commands.
NON_SHELL = {"toml", "rhai", "rust", "wgsl", "json", "text", "txt", "glsl", "ini", "yaml", "yml", "csv", "diff", "html", "js", "javascript", "python", "py"}


class Flint:
    """`flint ... --help`, parsed once per command path."""

    def __init__(self, binary):
        self.binary = binary
        self.cache = {}

    def help(self, path):
        key = tuple(path)
        if key not in self.cache:
            out = subprocess.run(
                [self.binary, *path, "--help"],
                capture_output=True,
                text=True,
                encoding="utf-8",
                errors="replace",
            )
            if out.returncode != 0:
                self.cache[key] = None
            else:
                self.cache[key] = parse_help(out.stdout)
        return self.cache[key]

    def styles(self):
        out = subprocess.run(
            [self.binary, "render", "--list-styles", "--json"],
            capture_output=True,
            text=True,
            encoding="utf-8",
            errors="replace",
        )
        if out.returncode != 0:
            raise SystemExit(f"docs-check: `flint render --list-styles --json` failed:\n{out.stderr}")
        data = json.loads(out.stdout)
        items = data if isinstance(data, list) else data.get("styles", [])
        return {s["name"] for s in items}


def parse_help(text):
    flags = {"--help", "--version"}
    commands = set()
    section = None
    usage = ""
    for line in text.splitlines():
        if line.startswith("Usage:"):
            usage = line
        if line and not line.startswith(" "):
            section = line.rstrip(":").strip().lower()
            continue
        if section == "commands":
            m = HELP_CMD_RE.match(line)
            if m and m.group(1) != "help":
                commands.add(m.group(1))
        else:
            m = HELP_FLAG_RE.match(line)
            if m:
                flags.add(m.group(1))
    # Only a command whose sole positional is its subcommand can say a word
    # is no subcommand of it; others take the word as an argument.
    only_subcommands = bool(commands) and re.search(r"<COMMAND>\s*$", usage) is not None
    return {"flags": flags, "commands": commands, "only_subcommands": only_subcommands}


def tokens_of(command):
    try:
        lexer = shlex.shlex(command, posix=True, punctuation_chars="|&;>")
        lexer.whitespace_split = True
        lexer.commenters = "#"
        return list(lexer)
    except ValueError:
        return command.split()


def flint_words(line):
    """The words after `flint` on a line that runs it, or None."""
    s = line.strip()
    s = re.sub(r"^\$\s+", "", s)
    for prefix in ("cargo run --bin flint -- ", "cargo run -p flint-cli --bin flint -- ", "flint "):
        if s.startswith(prefix):
            rest = s[len(prefix):]
            break
    else:
        if s in ("flint", "cargo run --bin flint --"):
            return []
        return None
    words = []
    for tok in tokens_of(rest):
        if tok in STOP_WORDS or set(tok) <= set("|&;>"):
            break
        words.append(tok)
    return words


def check_command(flint, words, where, problems, allow):
    path = []
    info = flint.help(path)
    i = 0
    while i < len(words):
        w = words[i]
        if not info["commands"] or w.startswith("-") or not re.fullmatch(r"[a-z][a-z0-9-]*", w):
            break
        # A hidden subcommand (`flint serve`) is in no help list but answers
        # its own --help.
        if w in info["commands"] or (not path and flint.help([w]) is not None):
            path.append(w)
            info = flint.help(path)
            if info is None:
                return
            i += 1
            continue
        if not path or info["only_subcommands"]:
            name = " ".join(["flint", *path, w])
            if not allowed(allow, where, w):
                problems.append(problem("flags", where, f"`{name}`: no such command", "see `flint --help`" if not path else f"see `flint {' '.join(path)} --help`"))
            return
        break
    for w in words[i:]:
        m = FLAG_RE.match(w)
        if not m:
            continue
        flag = m.group(0)
        COUNTS["flags"] += 1
        if flag not in info["flags"] and not allowed(allow, where, flag):
            cmd = " ".join(["flint", *path])
            problems.append(problem("flags", where, f"`{cmd} {flag}`: no such flag", f"`{cmd} --help` lists the flags"))


def problem(check, where, message, hint):
    page, line = where
    return {"check": check, "file": page, "line": line, "message": message, "hint": hint}


def allowed(allow, where, token):
    page = where[0]
    return token in allow.get(page, set()) or token in allow.get("*", set())


def load_allow():
    allow = {}
    if not ALLOW.exists():
        return allow
    for raw in ALLOW.read_text(encoding="utf-8").splitlines():
        line = raw.split("#", 1)[0].strip()
        if not line:
            continue
        if ": " in line:
            page, names = line.split(": ", 1)
        else:
            page, names = "*", line
        allow.setdefault(page.strip(), set()).update(names.split())
    return allow


def rel(path):
    return path.relative_to(ROOT).as_posix()


def book_page(path):
    """A page's name in the allow file: its path under src/, else the repo's."""
    try:
        return path.relative_to(BOOK).as_posix()
    except ValueError:
        return rel(path)


def scan(path, flint, styles, problems, allow):
    text = path.read_text(encoding="utf-8")
    page = book_page(path)
    in_book = path.is_relative_to(BOOK)
    in_cli_ref = path.is_relative_to(BOOK / "cli-reference")
    lines = text.splitlines()
    fence = None
    shell_block = False
    heading_cmd = None
    flag_tables = False
    pending = ""
    pending_line = 0
    for n, line in enumerate(lines, 1):
        where = (page, n)
        m = FENCE_RE.match(line)
        if m:
            if fence is None:
                fence = m.group(1)
                lang = m.group(2).strip().split(",")[0].split()[0].lower() if m.group(2).strip() else ""
                shell_block = lang not in NON_SHELL
            elif m.group(1).startswith(fence[0]) and len(m.group(1)) >= len(fence):
                fence = None
                pending = ""
            continue

        for name in STYLE_RE.findall(line):
            COUNTS["styles"] += 1
            if name not in styles and not allowed(allow, where, name):
                problems.append(problem("styles", where, f"`[post_process.styles.{name}]`: no style `{name}` is registered", "`flint render --list-styles` lists them; a style a page makes up goes in docs/book/docs-check-allow.txt"))

        if fence is not None:
            if not shell_block:
                continue
            if pending:
                command = pending + " " + line.strip()
            else:
                command = line
                pending_line = n
            if command.rstrip().endswith("\\"):
                pending = command.rstrip()[:-1]
                continue
            pending = ""
            words = flint_words(command)
            if words is not None:
                check_command(flint, words, (page, pending_line), problems, allow)
            continue

        if line.startswith("#"):
            title = line.lstrip("#").strip()
            level = len(line) - len(line.lstrip("#"))
            named = None
            for r in HEADING_CMD_RES:
                hm = r.match(title)
                if hm:
                    named = hm.group(1).split()
                    break
            if named is not None:
                heading_cmd = (level, named)
            elif heading_cmd is not None and level <= heading_cmd[0]:
                heading_cmd = None
            # Tables directly under the command's heading, or under one of
            # its "... Flags" headings, are its flags; another subsection's
            # (the standalone player's screenshot flags under `play`) may
            # not be.
            flag_tables = named is not None or title.endswith("Flags")

        for _, span in SPAN_RE.findall(line):
            words = flint_words(span)
            if words is not None:
                check_command(flint, words, where, problems, allow)

        if in_cli_ref and heading_cmd is not None and flag_tables and line.lstrip().startswith("|"):
            first = line.strip().strip("|").split("|")[0]
            for span in re.findall(r"`([^`]+)`", first):
                fm = FLAG_RE.match(span.strip())
                if fm:
                    check_command(flint, [*heading_cmd[1], fm.group(0)], where, problems, allow)

        if in_book:
            for target in IMAGE_MD_RE.findall(line) + IMAGE_HTML_RE.findall(line):
                if re.match(r"^[a-z]+:", target) or target.startswith("#"):
                    continue
                target = target.split("#", 1)[0].split("?", 1)[0]
                COUNTS["images"] += 1
                if not (path.parent / target).exists() and not allowed(allow, where, target):
                    problems.append(problem("images", where, f"image `{target}` does not exist", f"looked for {rel((path.parent / target).resolve())}"))


def sources():
    files = sorted(BOOK.rglob("*.md"))
    listed = subprocess.run(["git", "ls-files", "*CLAUDE.md"], cwd=ROOT, capture_output=True, text=True)
    for name in listed.stdout.split():
        p = ROOT / name
        if p.exists() and not name.startswith(("engine/", "examples/")):
            files.append(p)
    return files


def find_flint(given):
    if given:
        return given
    exe = ".exe" if os.name == "nt" else ""
    for profile in ("debug", "release"):
        p = ROOT / "target" / profile / f"flint{exe}"
        if p.exists():
            return str(p)
    raise SystemExit("docs-check: no flint binary; build one with `cargo build -p flint-cli --no-default-features --bin flint` or pass --flint")


def main():
    ap = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    ap.add_argument("--flint", help="the flint binary to ask (default: target/debug/flint)")
    ap.add_argument("--json", action="store_true", help="print the problems as JSON")
    args = ap.parse_args()

    flint = Flint(find_flint(args.flint))
    if flint.help([]) is None:
        raise SystemExit(f"docs-check: `{flint.binary} --help` failed")
    styles = flint.styles()
    allow = load_allow()
    problems = []
    files = sources()
    for f in files:
        scan(f, flint, styles, problems, allow)

    if args.json:
        print(json.dumps({"files": len(files), "commands": len(flint.cache), "styles": sorted(styles), "checked": COUNTS, "problems": problems}, indent=2))
    else:
        for p in problems:
            print(f"{p['file']}:{p['line']}: [{p['check']}] {p['message']}\n    {p['hint']}")
        print(f"docs-check: {len(files)} files; {COUNTS['flags']} flags on {len(flint.cache)} commands, "
              f"{COUNTS['styles']} style tables, {COUNTS['images']} images: "
              + (f"{len(problems)} problem(s)" if problems else "clean"))
    return 1 if problems else 0


if __name__ == "__main__":
    sys.exit(main())
