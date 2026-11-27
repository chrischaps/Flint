#!/usr/bin/env python3
"""The book and the CLAUDE.md files against the code (#375).

Three mechanical checks, the ones a page drifts on without anyone noticing:

  flags   every `--flag` on a `flint ...` command line (in a code block or a
          code span) is one that command's `--help` lists, and every
          subcommand named is real. In docs/book/src/cli-reference/, a table
          row whose first cell is a `--flag`, under a heading that names a
          command ("The `render` Command", "`flint music chart play`"), is checked
          against that command too.
  styles  every `[post_process.styles.<name>]` names a registered style
          (`flint render --list-styles --json`).
  images  every image a book page links (`![..](path)`, `<img src=..>`)
          exists on disk.
  maps    the maps an agent reads first (#756): a count of crates ("31-crate",
          "31 crates under `crates/`") or workspace members ("32-member") is
          the real one, written in digits; every crate under crates/ is named
          in the root CLAUDE.md's Architecture tree and in the book's
          architecture overview (its tree and a `### <crate>` section); and a
          "Commands: a, b, ..." list names only real commands and every one
          `flint --help` shows (hidden ones may be listed, need not be).
  examples
          every example a page cites (#984): a `flint-examples/<path>` (the
          path in a clone of the examples' repository, which the book's
          commands use) and every link into
          github.com/chrischaps/flint-examples names a file or folder that
          repository has. Its file list comes from --examples <clone>, else a
          clone at flint-examples/ in the checkout, else GitHub's API.
  packages
          every engine package a page names (#1337) is one `flint package
          list --json` lists: "the `living` package", "package `music`", the
          span a `**Package:**` line opens with, `living = "*"`, a `[packages]`
          table's keys in a TOML block, `packages = [..]`; and every page
          under concepts/ states its package on a `**Package:**` line. The
          init templates' Markdown is read for this check too. And each
          concepts page's Package lines are held to the manifests (#1535):
          an item a line gives to a package (a component, a `[table]`, a
          "`x` module", each going to the first package its sentence
          names) must be that package's; and each item the page documents
          (the component of an `[entities.<e>.<component>]` header and the
          table a `[<table>]` header opens, in its TOML blocks; the module
          of a `<!-- script-api:begin -->` region, or of each function it
          names) must be the kernel's, or a package's that a Package line
          over it names. A Package line under the title covers the page;
          one under another heading covers that heading's section.
          A package's pages live in its folder (#1563): every file under
          packages/<name>/docs/ is the book's page at the same path (what
          scripts/book-collect.py copies into docs/book/src), read here from
          its package's folder, never from the copy. Such a page's title
          Package line names that package first; a book page in
          docs/book/src whose title line names a package other than the
          kernel belongs in that package's folder. And what book-collect.py
          --check holds (no page with two homes, every page in SUMMARY.md,
          the .gitignore list) is a `book` problem here too.

Usage:
  python scripts/docs-check.py [--flint <path to flint binary>] [--examples <clone>] [--json]

Without --flint it looks for target/debug/flint, then target/release/flint;
build one with `cargo build -p flint-cli --no-default-features --bin flint`.
Exit 1 when anything is found. What isn't drift (a user style a page
invents to show the format) goes in docs/book/docs-check-allow.txt.
"""

import argparse
import importlib.util
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

COUNTS = {"flags": 0, "styles": 0, "images": 0, "maps": 0, "examples": 0, "packages": 0, "owners": 0, "homes": 0}
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
# The maps (#756): counts of the workspace, and command lists.
CRATE_COUNT_RES = [re.compile(r"\b(\d+)-crate\b"), re.compile(r"\b(\d+) crates under `crates/`")]
MEMBER_COUNT_RE = re.compile(r"\b(\d+)-member\b")
SPELLED_COUNT_RE = re.compile(r"\b((?:twenty|thirty|forty)(?:-[a-z]+)?)[- ](?:crates?|members?)\b", re.I)
COMMANDS_RE = re.compile(r"\bCommands: (?=`?[a-z])")
# The examples' repository (#984), and how a page cites a file in it.
EXAMPLES_REPO = "chrischaps/flint-examples"
EXAMPLE_URL_RE = re.compile(r"github\.com/chrischaps/flint-examples/(?:blob|tree)/main/([^)\s#?`\"'>]+)")
EXAMPLE_PATH_RE = re.compile(r"(?<![A-Za-z0-9_./-])flint-examples/([A-Za-z0-9_./*{}<>,-]*)")
TREE_GUTTER = "│├└─ "
# How a page names an engine package (#1337). Each group is one name.
PACKAGE_RES = [
    re.compile(r"`([a-z][a-z0-9-]*)` packages?\b"),          # the `living` package
    re.compile(r"\bpackages? `([a-z][a-z0-9-]*)`"),          # package `music`
    re.compile(r"`([a-z][a-z0-9-]*) = \"\*\"`"),             # `living = "*"`
]
# A page's own statement of its package: `**Package:** `music` ...`
PACKAGE_LINE_RE = re.compile(r"^\s*>?\s*\*\*Packages?:\*\*(.*)$")
# In a TOML block: a key of a `[packages]` table, and `packages = [..]`
TOML_TABLE_RE = re.compile(r"^\s*\[+\s*([^\]]+?)\s*\]+")
TOML_KEY_RE = re.compile(r"^\s*([A-Za-z0-9_-]+)\s*=")
PACKAGES_ARRAY_RE = re.compile(r"^\s*packages\s*=\s*\[([^\]]*)\]")
# Every page here states its package
STATES_PACKAGE = BOOK / "concepts"
# The init templates (#707): what a new project's agent reads first
TEMPLATES = ROOT / "crates" / "flint-cli" / "templates"
# Where each crate under crates/ must be named: (file, the section it's
# under, whether that section must hold a `### <crate>` heading for it).
CRATE_MAPS = [
    ("CLAUDE.md", "## Architecture", False),
    ("docs/book/src/architecture/overview.md", "## Workspace Structure", False),
    ("docs/book/src/architecture/overview.md", "## Crate Details", True),
]


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

    def packages(self):
        """The engine's packages: `flint package list --json`, from the
        manifests of this checkout (`--dir packages/`), as its list."""
        out = subprocess.run(
            [self.binary, "--json", "package", "list", "--dir", str(ROOT / "packages")],
            capture_output=True,
            text=True,
            encoding="utf-8",
            errors="replace",
        )
        try:
            data = json.loads(out.stdout)
        except ValueError:
            raise SystemExit(f"docs-check: `flint package list --json` failed:\n{out.stderr}")
        listed = data.get("packages", [])
        if not listed:
            raise SystemExit(f"docs-check: `flint package list --json` listed no package:\n{out.stdout}{out.stderr}")
        return listed

    def script_modules(self):
        """Each script function's module, from `flint script api --json`
        ({} when it fails: a region of function names then goes unowned)."""
        out = subprocess.run(
            [self.binary, "--json", "script", "api"],
            capture_output=True,
            text=True,
            encoding="utf-8",
            errors="replace",
        )
        try:
            data = json.loads(out.stdout)
        except ValueError:
            return {}
        return {f["name"]: f["module"] for f in data.get("functions", []) if f.get("module")}


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


def collector():
    """scripts/book-collect.py, which says where a package's pages go (#1563)."""
    spec = importlib.util.spec_from_file_location("book_collect", ROOT / "scripts" / "book-collect.py")
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


_PACKAGE_PAGES = None


def package_pages():
    """{source path: (path in the book, package)} for every file under a
    package's docs/ (#1563)."""
    global _PACKAGE_PAGES
    if _PACKAGE_PAGES is None:
        _PACKAGE_PAGES = {src.resolve(): (book, pkg) for book, pkg, src in collector().pages(ROOT)}
    return _PACKAGE_PAGES


def in_book(path):
    """Where a page sits in the book: a package's page at its book path under
    src/ (#1563), any other path as it is."""
    found = package_pages().get(path.resolve())
    return BOOK / found[0] if found else path


def home_of(path):
    """The package whose folder holds a page, or None."""
    found = package_pages().get(path.resolve())
    return found[1] if found else None


def exists_in_book(path):
    """A file the built book has: on disk, or a package's file collected to
    that path."""
    if path.exists():
        return True
    try:
        book = path.resolve().relative_to(BOOK.resolve()).as_posix()
    except ValueError:
        return False
    return any(b == book for b, _ in package_pages().values())


def book_page(path):
    """A page's name in the allow file: its path under src/ (a package's page
    by where it is collected to), else the repo's."""
    try:
        return in_book(path).relative_to(BOOK).as_posix()
    except ValueError:
        return rel(path)


def example_files(given):
    """Every file and folder in the examples' repository, as paths from its
    root: from a clone (`--examples`, else `flint-examples/` here), else the
    GitHub API. (None, None) when there is no way to know."""
    clone = Path(given) if given else ROOT / "flint-examples"
    if (clone / "index.toml").is_file():
        out = subprocess.run(["git", "ls-files"], cwd=clone, capture_output=True, text=True)
        files = out.stdout.split() if out.returncode == 0 else [
            p.relative_to(clone).as_posix() for p in clone.rglob("*") if p.is_file()]
        source = str(clone)
    elif given:
        raise SystemExit(f"docs-check: --examples {given} has no index.toml")
    else:
        import urllib.request
        url = f"https://api.github.com/repos/{EXAMPLES_REPO}/git/trees/main?recursive=1"
        req = urllib.request.Request(url, headers={"Accept": "application/vnd.github+json", "User-Agent": "flint-docs-check"})
        token = os.environ.get("GITHUB_TOKEN")
        if token:
            req.add_header("Authorization", f"Bearer {token}")
        try:
            with urllib.request.urlopen(req, timeout=30) as r:
                tree = json.load(r)
        except Exception as e:  # offline, rate-limited
            print(f"docs-check: can't list {EXAMPLES_REPO} ({e}); the example citations go unchecked. "
                  "Clone it at flint-examples/ or pass --examples <clone>", file=sys.stderr)
            return None, None
        files = [e["path"] for e in tree["tree"] if e["type"] == "blob"]
        source = f"github.com/{EXAMPLES_REPO}@main"
    names = set(files)
    for f in files:
        parts = f.split("/")
        for i in range(1, len(parts)):
            names.add("/".join(parts[:i]))
    return names, source


def check_examples(line, where, examples, source, problems, allow):
    if examples is None:
        return
    for name in EXAMPLE_URL_RE.findall(line) + EXAMPLE_PATH_RE.findall(line):
        name = name.rstrip(".,:;)]").rstrip("/")
        if not name or re.search(r"[<>*{}\[\]|]", name) or "..." in name:
            continue  # the repository itself, or a pattern
        COUNTS["examples"] += 1
        if name not in examples and not allowed(allow, where, name):
            problems.append(problem("examples", where, f"`flint-examples/{name}` is not a file or folder of {EXAMPLES_REPO}",
                                    f"looked in {source}; the examples moved there in #984, scenes at its root"))


def check_package_name(name, where, packages, problems, allow):
    COUNTS["packages"] += 1
    if name in packages or allowed(allow, where, name):
        return
    import difflib

    near = difflib.get_close_matches(name, sorted(packages), n=1)
    hint = f"did you mean `{near[0]}`? " if near else ""
    problems.append(problem("packages", where, f"`{name}`: no engine package is named that",
                            f"{hint}`flint package list` lists them: {', '.join(sorted(packages))}"))


def check_package_line(line, where, packages, problems, allow, toml_table):
    """The package names one line of prose (or of a TOML block, when
    `toml_table` is the table it is under) holds."""
    names = []
    if toml_table is not None:
        if toml_table == "packages":
            m = TOML_KEY_RE.match(line)
            if m:
                names.append(m.group(1))
        m = PACKAGES_ARRAY_RE.match(line)
        if m:
            names += re.findall(r"\"([^\"]+)\"", m.group(1))
    else:
        for r in PACKAGE_RES:
            names += r.findall(line)
        # The line's first span is the page's package; the rest of it is
        # prose (a module, a component) the patterns above read
        m = PACKAGE_LINE_RE.match(line)
        if m:
            names += re.findall(r"^\s*`([a-z][a-z0-9-]*)`", m.group(1))
    for name in dict.fromkeys(names):
        check_package_name(name, where, packages, problems, allow)


class Owners:
    """Who provides what (#1535): each component, scene table and script
    module to the package whose manifest lists it, from `flint package list
    --json`, and each script function to its module."""

    KINDS = {"components": "component", "scene_tables": "table", "script_modules": "module"}

    def __init__(self, listed, functions):
        self.names = {p["name"] for p in listed}
        self.kernel = {p["name"] for p in listed if p.get("tier") == "kernel"}
        self.files = {p["name"]: p.get("file", f"packages/{p['name']}/package.toml") for p in listed}
        self.of = {}
        for p in listed:
            for key, kind in self.KINDS.items():
                for item in p.get(key) or []:
                    self.of[(kind, item)] = p["name"]
        self.functions = functions

    def owner(self, kind, item):
        return self.of.get((kind, item))

    @staticmethod
    def shown(kind, item):
        return f"`[{item}]`" if kind == "table" else f"the `{item}` module" if kind == "module" else f"`{item}`"


# How a Package line attributes an item: a run of spans ("`living` and
# `debug`") followed by "module(s)" or "script module(s)" are modules, a
# `[name]` span is a scene table, any other span that is a component's name
# is that component. Each goes to the first package its sentence names.
SENTENCE_SPLIT_RE = re.compile(r"(?<=[.!?])\s+")
SPAN_POS_RE = re.compile(r"`([^`]+)`")
SPAN_JOIN_RE = re.compile(r"^(?:,\s*|\s+and\s+|,\s+and\s+|\s+or\s+)$")
MODULE_AFTER_RE = re.compile(r"^\s+(?:script\s+)?modules?\b")
TABLE_SPAN_RE = re.compile(r"^\[+\s*([a-z][a-z0-9_]*)(?:\.[^\]]*)?\s*\]+$")
# What a TOML block documents: `[entities.<e>.<component>]`, a prefab
# override's `[prefabs.<p>.overrides.<e>.<component>]`, else the table its
# header opens. And a `<!-- script-api:begin ... -->` region's selectors.
REGION_RE = re.compile(r"<!--\s*script-api:begin\s+(.*?)\s*-->")


def line_claims(text, owners):
    """(package, kind, item) for each item a Package line's text names, and
    every package the line names."""
    claims = []
    named = []
    lead = re.match(r"^\s*`([a-z][a-z0-9-]*)`", text)
    for i, sentence in enumerate(SENTENCE_SPLIT_RE.split(text)):
        found = [m for r in PACKAGE_RES for m in r.finditer(sentence)]
        found.sort(key=lambda m: m.start())
        pkgs = [m.group(1) for m in found if m.group(1) in owners.names]
        if i == 0 and lead and lead.group(1) in owners.names:
            pkgs.insert(0, lead.group(1))
        named += pkgs
        if not pkgs:
            continue
        spans = list(SPAN_POS_RE.finditer(sentence))
        modules = set()
        run = []
        for k, s in enumerate(spans):
            if run and not SPAN_JOIN_RE.match(sentence[spans[k - 1].end():s.start()]):
                run = []
            run.append(k)
            if MODULE_AFTER_RE.match(sentence[s.end():]):
                modules.update(run)
                run = []
        for k, s in enumerate(spans):
            word = s.group(1).strip()
            tm = TABLE_SPAN_RE.match(word)
            if k in modules:
                kind = "module"
            elif tm:
                kind, word = "table", tm.group(1)
            elif owners.owner("component", word):
                kind = "component"
            else:
                continue
            if owners.owner(kind, word):
                claims.append((pkgs[0], kind, word))
    return claims, list(dict.fromkeys(named))


def documented(lines, owners):
    """(line number, kind, item) for each item a page documents: the
    components and tables its TOML blocks' headers open, and the modules of
    its script-api regions (a function named there counts as its module)."""
    out = []
    fence = None
    toml_block = False
    for n, line in enumerate(lines, 1):
        m = FENCE_RE.match(line)
        if m:
            if fence is None:
                fence = m.group(1)
                toml_block = (m.group(2).strip().split() or [""])[0].lower() == "toml"
            elif m.group(1).startswith(fence[0]) and len(m.group(1)) >= len(fence):
                fence = None
            continue
        if fence is not None:
            if toml_block:
                tm = TOML_TABLE_RE.match(line)
                if tm:
                    parts = [p.strip().strip("\"'") for p in tm.group(1).split(".")]
                    if parts[0] == "entities" and len(parts) >= 3:
                        out.append((n, "component", parts[2]))
                    elif parts[0] == "prefabs" and len(parts) >= 5 and parts[2] == "overrides":
                        out.append((n, "component", parts[4]))
                    elif parts[0] not in ("entities", "prefabs"):
                        out.append((n, "table", parts[0]))
            continue
        for selectors in REGION_RE.findall(line):
            for sel in selectors.split():
                if sel.startswith("module:"):
                    out.append((n, "module", sel[len("module:"):]))
                elif sel in owners.functions:
                    out.append((n, "module", owners.functions[sel]))
    return [d for d in dict.fromkeys(out) if owners.owner(d[1], d[2])]


def package_scopes(lines):
    """(first line, last line, Package line number, its text) for each
    `**Package:**` line: under the title it covers the page, under another
    heading that heading's section."""
    headings = []
    fence = None
    for n, line in enumerate(lines, 1):
        m = FENCE_RE.match(line)
        if m:
            if fence is None:
                fence = m.group(1)
            elif m.group(1).startswith(fence[0]) and len(m.group(1)) >= len(fence):
                fence = None
            continue
        if fence is None and re.match(r"^#{1,6}\s", line):
            headings.append((n, len(line) - len(line.lstrip("#"))))
    scopes = []
    for n, line in enumerate(lines, 1):
        m = PACKAGE_LINE_RE.match(line)
        if not m:
            continue
        above = [h for h in headings if h[0] < n]
        if not above or above[-1][1] == 1:
            scopes.append((1, len(lines), n, m.group(1)))
            continue
        start, level = above[-1]
        end = next((h[0] - 1 for h in headings if h[0] > start and h[1] <= level), len(lines))
        scopes.append((start, end, n, m.group(1)))
    return scopes


def check_page_owners(path, lines, owners, problems, allow):
    """A concepts page's Package lines against the manifests (#1535): an
    item a line gives to a package is that package's, and each item the page
    documents is the kernel's or a package's that a Package line over it
    names. Either names the package the page says and the one that owns it."""
    page = book_page(path)
    scopes = package_scopes(lines)
    reported = set()
    read = []
    for start, end, n, text in scopes:
        claims, named = line_claims(text, owners)
        read.append((start, end, n, named))
        for said, kind, item in claims:
            COUNTS["owners"] += 1
            actual = owners.owner(kind, item)
            if actual == said or allowed(allow, (page, n), item):
                continue
            reported.add((kind, item))
            problems.append(problem("packages", (page, n),
                                    f"the Package line gives {Owners.shown(kind, item)} to `{said}`, but the `{actual}` package provides it",
                                    f"{owners.files.get(actual, actual)} lists it: give it to `{actual}` on the line, or list it in `{said}`'s manifest again"))
    for n, kind, item in documented(lines, owners):
        if (kind, item) in reported:
            continue
        COUNTS["owners"] += 1
        actual = owners.owner(kind, item)
        over = [s for s in read if s[0] <= n <= s[1]]
        said = [p for s in over for p in s[3]]
        if actual in owners.kernel or actual in said or allowed(allow, (page, n), item):
            continue
        reported.add((kind, item))
        stated = ", ".join(f"`{p}`" for p in dict.fromkeys(said)) or "no package"
        problems.append(problem("packages", (page, n),
                                f"{Owners.shown(kind, item)} is the `{actual}` package's, but the Package lines over it say {stated}",
                                f"{owners.files.get(actual, actual)} provides it: name `{actual}` on the page's `**Package:**` line, "
                                f"or put a `> **Package:** `{actual}`` line under the heading of the section that documents it"))


def check_home(path, lines, owners, problems, allow):
    """Where a page lives against its title's Package line (#1563): a page in
    packages/<name>/docs/ is `<name>`'s, and a page in docs/book/src that
    says a package other than the kernel belongs in that package's folder."""
    page = book_page(path)
    home = home_of(path)
    title = [s for s in package_scopes(lines) if s[0] == 1]
    if not title:
        if home is not None:
            COUNTS["homes"] += 1
            problems.append(problem("packages", (page, 1), f"the page lives in packages/{home}/docs/ but doesn't state its package",
                                    f"a page in a package's folder is that package's: a `> **Package:** `{home}`` line under its title"))
        return
    _, _, n, text = title[0]
    lead = re.match(r"^\s*`([a-z][a-z0-9-]*)`", text)
    said = lead.group(1) if lead else None
    if said is None or said not in owners.names:
        return  # the packages check names a line with no package, or a name that isn't one
    COUNTS["homes"] += 1
    if allowed(allow, (page, n), said):
        return
    if home is not None and said != home:
        where = f"docs/book/src/{page}" if said in owners.kernel else f"packages/{said}/docs/{page}"
        problems.append(problem("packages", (page, n), f"the page lives in packages/{home}/docs/, but its Package line says `{said}`",
                                f"a page is kept with the package it documents: move it to {where}, or name `{home}` first on the line"))
    elif home is None and said not in owners.kernel:
        problems.append(problem("packages", (page, n), f"the page's Package line says `{said}`, but it lives in docs/book/src",
                                f"a package's page is kept in its folder (#1563): `git mv` it to packages/{said}/docs/{page} "
                                f"and run `python scripts/book-collect.py`"))


def self_test():
    """The owner check on made-up manifests and pages, with no binary
    (`--self-test`, in scripts/ci.sh): what a Package line gives to whom,
    and which line covers what."""
    def pkg(name, tier="feature", **provides):
        return {"name": name, "tier": tier, "file": f"packages/{name}/package.toml", **provides}

    def owners_with(**moved):
        listed = [
            pkg("engine", "kernel", components=["transform", "asset_def"], scene_tables=["post_process"], script_modules=["math"]),
            pkg("debug-hud", "standard"),
            pkg("procgen", "standard"),
            pkg("living", components=["living_prop", "shallow_wooze"], scene_tables=["living", "wooze"], script_modules=["living", "wooze", "debug"]),
            pkg("music", components=["music_session"], script_modules=["music"]),
            pkg("rhythm", script_modules=["conducted"]),
        ]
        for item, to in moved.items():
            for p in listed:
                for key in ("components", "scene_tables", "script_modules"):
                    if item in p.get(key, []):
                        p[key] = [i for i in p[key] if i != item]
                        next(q for q in listed if q["name"] == to).setdefault(key, []).append(item)
        return Owners(listed, {"tune_living": "living", "set_wooze": "wooze", "sin": "math"})

    music = ("`music`, a feature package: a game has `music_session` and the `music` script module only when its "
             "`[packages]` lists `music = \"*\"`. Charts judged against the music, and the `conducted` module, are the "
             "`rhythm` package's, which brings `music` with it ([Packages](packages.md)).")
    procgen = ("`procgen`, a standard package: a game has it unless its `[packages]` sets `procgen = false`. Living props "
               "and the wooze field (`living_prop`, `shallow_wooze`, `[living]`, `[wooze]` and their script modules) are "
               "the `living` package's, a feature a game lists as `living = \"*\"` ([Packages](packages.md)).")
    props = ("`living`, a feature package: a game has the `living` and `debug` modules only when its `[packages]` lists "
             "`living = \"*\"`. Of the two, `debug` runs only where the `debug-hud` package is in the build too.")
    o = owners_with()
    cases = [
        (line_claims(music, o), ([("music", "component", "music_session"), ("music", "module", "music"), ("rhythm", "module", "conducted")], ["music", "rhythm"])),
        (line_claims(procgen, o), ([("living", "component", "living_prop"), ("living", "component", "shallow_wooze"), ("living", "table", "living"), ("living", "table", "wooze")], ["procgen", "living"])),
        (line_claims(props, o), ([("living", "module", "living"), ("living", "module", "debug")], ["living", "debug-hud"])),
    ]
    failed = [f"line_claims: got {got}, want {want}" for got, want in cases if got != want]

    def run(text, owners):
        problems = []
        check_page_owners(BOOK / "concepts" / "t.md", text.splitlines(), owners, problems, {})
        return [(p["line"], p["message"]) for p in problems]

    page = "\n".join([
        "# T", "", "> **Package:** `engine`, the kernel.", "",
        "```toml", "# a comment, not a heading", "[entities.e.transform]", "[entities.e.asset_def]", "```", "",
        "## Boil", "", "{line}", "", "```toml", "[wooze]", "[post_process]", "```", "",
        "### Deeper", "", "<!-- script-api:begin tune_living sin -->", "",
        "## After", "", "```toml", "[prefabs.p.overrides.e.shallow_wooze]", "```",
    ])
    covered = page.replace("{line}", "> **Package:** `living`: the boil reads the wooze field.")
    bare = page.replace("{line}", "Prose.")
    want_bare = [(16, "`[wooze]` is the `living` package's, but the Package lines over it say `engine`"),
                 (22, "the `living` module is the `living` package's, but the Package lines over it say `engine`"),
                 (27, "`shallow_wooze` is the `living` package's, but the Package lines over it say `engine`")]
    for got, want in [
        (run(covered, o), want_bare[2:]),  # a section's line covers its subsections, not the next section
        (run(bare, o), want_bare),
        (run("# M\n\n> **Package:** " + music + "\n", owners_with(music_session="rhythm")),
         [(3, "the Package line gives `music_session` to `music`, but the `rhythm` package provides it")]),
        (run("# M\n\n> **Package:** " + music + "\n", owners_with(music_session="engine")),
         [(3, "the Package line gives `music_session` to `music`, but the `engine` package provides it")]),
        # The kernel's items in passing, on a feature's page
        (run("# M\n\n> **Package:** " + music + "\n\n```toml\n[entities.s.transform]\n[entities.s.music_session]\n```\n", o), []),
    ]:
        if got != want:
            failed.append(f"check_page_owners: got {got}, want {want}")
    # Where a page lives (#1563): a music page whose title line says another
    # package fails; so does a book page naming a feature package
    global _PACKAGE_PAGES
    kept = _PACKAGE_PAGES
    music_page = ROOT / "packages" / "music" / "docs" / "concepts" / "t.md"
    _PACKAGE_PAGES = {music_page.resolve(): ("concepts/t.md", "music")}

    def home(path, text):
        problems = []
        check_home(path, text.splitlines(), o, problems, {})
        return [(p["line"], p["message"]) for p in problems]

    for got, want in [
        (home(music_page, "# M\n\n> **Package:** " + music + "\n"), []),
        (home(music_page, "# M\n\n> **Package:** " + procgen + "\n"),
         [(3, "the page lives in packages/music/docs/, but its Package line says `procgen`")]),
        (home(music_page, "# M\n\n> **Package:** `engine`, the kernel.\n"),
         [(3, "the page lives in packages/music/docs/, but its Package line says `engine`")]),
        (home(music_page, "# M\n\nNo line.\n"),
         [(1, "the page lives in packages/music/docs/ but doesn't state its package")]),
        # A section's line may name another package; only the title's says whose the page is
        (home(music_page, "# M\n\n> **Package:** " + music + "\n\n## Props\n\n> **Package:** " + props + "\n"), []),
        (home(BOOK / "concepts" / "t.md", "# M\n\n> **Package:** " + music + "\n"),
         [(3, "the page's Package line says `music`, but it lives in docs/book/src")]),
        (home(BOOK / "concepts" / "t.md", "# M\n\n> **Package:** `engine`, the kernel.\n"), []),
    ]:
        if got != want:
            failed.append(f"check_home: got {got}, want {want}")
    _PACKAGE_PAGES = kept
    for f in failed:
        print(f"docs-check --self-test: {f}")
    print(f"docs-check --self-test: {'ok' if not failed else f'{len(failed)} failed'}")
    return 1 if failed else 0


def scan(path, flint, styles, ws, problems, allow, examples=None, source=None, packages=None, owners=None):
    text = path.read_text(encoding="utf-8")
    page = book_page(path)
    # A package's page is read where it sits in the book (#1563)
    at = in_book(path)
    is_book = at.is_relative_to(BOOK)
    in_cli_ref = at.is_relative_to(BOOK / "cli-reference")
    lines = text.splitlines()
    if packages is not None and at.parent == STATES_PACKAGE:
        if not any(PACKAGE_LINE_RE.match(l) for l in lines):
            problems.append(problem("packages", (page, 1), "the page doesn't state its package",
                                    "a concepts page says what it documents is whose: a `> **Package:** `engine`` line under its title"))
        elif owners is not None:
            check_page_owners(path, lines, owners, problems, allow)
    if owners is not None and (is_book or home_of(path)):
        check_home(path, lines, owners, problems, allow)
    fence = None
    shell_block = False
    toml_block = False
    toml_table = ""
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
                toml_block = lang == "toml"
                toml_table = ""
            elif m.group(1).startswith(fence[0]) and len(m.group(1)) >= len(fence):
                fence = None
                pending = ""
            continue

        if packages is not None:
            if fence is not None and toml_block:
                tm = TOML_TABLE_RE.match(line)
                if tm:
                    toml_table = tm.group(1).strip()
                else:
                    check_package_line(line, where, packages, problems, allow, toml_table)
            elif fence is None:
                check_package_line(line, where, packages, problems, allow, None)
        if path.is_relative_to(TEMPLATES):
            continue  # the templates are read for their packages only

        for name in STYLE_RE.findall(line):
            COUNTS["styles"] += 1
            if name not in styles and not allowed(allow, where, name):
                problems.append(problem("styles", where, f"`[post_process.styles.{name}]`: no style `{name}` is registered", "`flint render --list-styles` lists them; a style a page makes up goes in docs/book/docs-check-allow.txt"))

        check_counts(line, where, ws, problems, allow)
        check_examples(line, where, examples, source, problems, allow)
        if COMMANDS_RE.search(line):
            check_commands(flint, command_list(lines, n - 1), where, problems, allow)

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

        if is_book:
            for target in IMAGE_MD_RE.findall(line) + IMAGE_HTML_RE.findall(line):
                if re.match(r"^[a-z]+:", target) or target.startswith("#"):
                    continue
                target = target.split("#", 1)[0].split("?", 1)[0]
                COUNTS["images"] += 1
                if not exists_in_book(at.parent / target) and not allowed(allow, where, target):
                    problems.append(problem("images", where, f"image `{target}` does not exist", f"looked for {rel((at.parent / target).resolve())}"))


def workspace():
    """The engine's crates (crates/ and a package's packages/*/crates/, #1554)
    and the workspace's member count, globs expanded as cargo does, from disk."""
    import tomllib

    under = [p for p in (ROOT / "crates").iterdir() if (p / "Cargo.toml").is_file()]
    # A package's own crates (#1554, #1557): engine crates in
    # `packages/<name>/crates/`, named in every map as the ones under crates/
    # are, and counted with them ("N-crate") or apart ("N crates under `crates/`")
    dirs = under + list(ROOT.glob("packages/*/crates/*"))
    crates = sorted(p.name for p in dirs if (p / "Cargo.toml").is_file())
    with open(ROOT / "Cargo.toml", "rb") as f:
        members = tomllib.load(f)["workspace"]["members"]
    count = sum(len([p for p in ROOT.glob(m) if (p / "Cargo.toml").is_file()]) if "*" in m else 1 for m in members)
    return {"crates": crates, "under_crates": len(under), "members": count}


def check_counts(line, where, ws, problems, allow):
    # "N-crate" counts every engine crate, a package's own (#1557) too;
    # "N crates under `crates/`" that folder's alone
    whole, under = CRATE_COUNT_RES
    for r, n_real, how in [
        (whole, len(ws["crates"]), "the directories under crates/ and packages/*/crates/ with a Cargo.toml"),
        (under, ws["under_crates"], "the directories under crates/ with a Cargo.toml"),
    ]:
        for m in r.finditer(line):
            COUNTS["maps"] += 1
            n = int(m.group(1))
            if n != n_real and not allowed(allow, where, m.group(0)):
                problems.append(problem("maps", where, f"`{m.group(0)}`: the workspace has {n_real} such crates", f"count {how}"))
    for m in MEMBER_COUNT_RE.finditer(line):
        COUNTS["maps"] += 1
        if int(m.group(1)) != ws["members"] and not allowed(allow, where, m.group(0)):
            problems.append(problem("maps", where, f"`{m.group(0)}`: the workspace has {ws['members']} members", "count `members` in the root Cargo.toml"))
    for m in SPELLED_COUNT_RE.finditer(line):
        COUNTS["maps"] += 1
        problems.append(problem("maps", where, f"`{m.group(0)}`: a count of the workspace in words", f"write it in digits ({len(ws['crates'])} crates, {ws['members']} members) so this check can hold it"))


def command_list(lines, i):
    """The command names of a "Commands: ..." list starting on lines[i].

    The list runs to the end of its sentence; a line ending in a comma
    continues on the next (a code block's tree gutter is stripped). Where
    the list uses code spans, only the spans are names."""
    text = COMMANDS_RE.split(lines[i], maxsplit=1)[1]
    j = i
    while text.rstrip().endswith(",") and j + 1 < len(lines):
        j += 1
        text += " " + lines[j].lstrip(TREE_GUTTER)
    text = re.split(r"\.(?:\s|$)", text, maxsplit=1)[0]
    if "`" in text:
        return re.findall(r"`([a-z][a-z0-9-]*)`", text)
    return [w for w in (t.strip() for t in text.split(",")) if re.fullmatch(r"[a-z][a-z0-9-]*", w)]


def check_commands(flint, names, where, problems, allow):
    COUNTS["maps"] += 1
    for name in names:
        if flint.help([name]) is None and not allowed(allow, where, name):
            problems.append(problem("maps", where, f"`{name}` in a command list: `flint {name} --help` fails", "`flint --help` lists the commands"))
    for name in sorted(flint.help([])["commands"] - set(names)):
        if not allowed(allow, where, name):
            problems.append(problem("maps", where, f"command list leaves out `{name}`", "a list of the CLI's commands names every one `flint --help` shows"))


def section(lines, heading):
    """(first line number, lines) of the section under `heading`, to the next heading of its level."""
    level = heading.split(" ", 1)[0]
    for n, line in enumerate(lines):
        if line.strip() == heading:
            end = next((k for k in range(n + 1, len(lines)) if re.match(rf"^{level} ", lines[k])), len(lines))
            return n + 1, lines[n + 1:end]
    return None


def check_crate_maps(ws, problems, allow):
    for name, heading, as_heading in CRATE_MAPS:
        path = ROOT / name
        page = book_page(path)
        found = section(path.read_text(encoding="utf-8").splitlines(), heading)
        if found is None:
            problems.append(problem("maps", (page, 1), f"no `{heading}` section", "it holds the crate map this check reads"))
            continue
        start, body = found
        text = "\n".join(body)
        for crate in ws["crates"]:
            COUNTS["maps"] += 1
            if as_heading:
                ok = re.search(rf"^### {re.escape(crate)}\s*$", text, re.M) is not None
            else:
                ok = re.search(rf"(?<![\w-]){re.escape(crate)}(?![\w-])", text) is not None
            if not ok and not allowed(allow, (page, start), crate):
                what = f"`### {crate}` section" if as_heading else f"`{crate}`"
                problems.append(problem("maps", (page, start), f"`{heading}` has no {what}", f"{crate} is a workspace crate; every map of the workspace names it"))


def sources():
    # A package's pages from its folder, never their copies in src/ (#1563)
    collected = {BOOK / book for book, _ in package_pages().values()}
    kept = sorted(src for src, (book, _) in package_pages().items() if book.endswith(".md"))
    files = sorted(p for p in BOOK.rglob("*.md") if p not in collected) + kept + [ROOT / "README.md"]
    listed = subprocess.run(["git", "ls-files", "*CLAUDE.md"], cwd=ROOT, capture_output=True, text=True)
    for name in listed.stdout.split():
        p = ROOT / name
        if p.exists() and not name.startswith(("engine/", "flint-examples/")):
            files.append(p)
    return files + sorted(TEMPLATES.rglob("*.md"))


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
    ap.add_argument("--examples", help="a clone of flint-examples to check citations against (default: flint-examples/ here, else GitHub)")
    ap.add_argument("--json", action="store_true", help="print the problems as JSON")
    ap.add_argument("--self-test", action="store_true", help="test the package owner check on made-up pages (no binary)")
    args = ap.parse_args()
    if args.self_test:
        return self_test()

    flint = Flint(find_flint(args.flint))
    if flint.help([]) is None:
        raise SystemExit(f"docs-check: `{flint.binary} --help` failed")
    styles = flint.styles()
    listed = flint.packages()
    packages = {p["name"] for p in listed}
    owners = Owners(listed, flint.script_modules())
    allow = load_allow()
    problems = []
    ws = workspace()
    files = sources()
    examples, source = example_files(args.examples)
    for f in files:
        scan(f, flint, styles, ws, problems, allow, examples, source, packages, owners)
    check_crate_maps(ws, problems, allow)
    # Where a package's pages go (#1563): what book-collect.py --check holds
    for f, message, hint in collector().problems(ROOT):
        problems.append(problem("book", (f, 1), message, hint))
    # A package's page is named by the file to edit, in its folder; the allow
    # file still keys it by its book path
    kept = {book: rel(src) for src, (book, _) in package_pages().items()}
    for p in problems:
        p["file"] = kept.get(p["file"], p["file"])

    if args.json:
        print(json.dumps({"files": len(files), "commands": len(flint.cache), "styles": sorted(styles), "packages": sorted(packages), "checked": COUNTS, "problems": problems}, indent=2))
    else:
        for p in problems:
            print(f"{p['file']}:{p['line']}: [{p['check']}] {p['message']}\n    {p['hint']}")
        print(f"docs-check: {len(files)} files; {COUNTS['flags']} flags on {len(flint.cache)} commands, "
              f"{COUNTS['styles']} style tables, {COUNTS['images']} images, {COUNTS['maps']} map entries, {COUNTS['examples']} example citations, "
              f"{COUNTS['packages']} package names, {COUNTS['owners']} items held to their packages, "
              f"{COUNTS['homes']} pages held to their folders: "
              + (f"{len(problems)} problem(s)" if problems else "clean"))
    return 1 if problems else 0


if __name__ == "__main__":
    sys.exit(main())
