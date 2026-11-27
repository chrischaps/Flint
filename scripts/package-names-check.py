#!/usr/bin/env python3
"""No package named in the launchers' source (epic #1548's gate, #1560).

`crates/flint-player/src` and `crates/flint-cli/src` are the kernel's
launchers: a package joins them through its manifest (ADR 0137 section 5),
never by name. This fails on either way the source can name one:

  1. a Cargo feature gate, `feature = "<f>"` for any `<f>` a manifest's
     `[build] features` lists, in code or test code (a `cfg`, a `cfg_attr`,
     a `cfg!`): the way Phase 1's moves came undone (#1330);
  2. a path through a package's crate (`flint_living::...`, `use
     flint_music`), for any crate a manifest's `[provides] crates` lists,
     outside test code. Tests may name a package's crate: they run where a
     launcher lists it (the dev-dependencies, #1558, #1559).

Comments are not read, nor are string literals for (2). A plain word
(`music`, a bus) isn't a package name here: a string that names a package
at run time, such as `registry.has("living")`, builds without it.

The packages are read from packages/*/package.toml, so a new package is
held with no edit here. What is still named on purpose is EXCEPTIONS, each
with the issue that takes it out; an exception that matches nothing fails
too, so the list only shrinks.

  python3 scripts/package-names-check.py        (scripts/ci.sh runs it)

Standard library only (tomllib: Python 3.11).
"""

import re
import sys
import tomllib
from pathlib import Path

REPO = Path(__file__).resolve().parent.parent
LAUNCHERS = {"flint-player": "crates/flint-player/src", "flint-cli": "crates/flint-cli/src"}

# (launcher, "feature" | "crate", name): why it is still there
EXCEPTIONS = {
    ("flint-cli", "feature", "ai"): "#1572: gen --ai-*, render --ai-refine and resolve "
    "--strategy ai_* are still declared by their commands",
    ("flint-cli", "crate", "flint-asset-gen"): "#1572: the ai flags' code",
    ("flint-cli", "crate", "flint-procgen-ai"): "#1572: the ai flags' code",
    ("flint-cli", "crate", "flint-procgen"): "#1572: gen --ai-*'s code reads procgen's specs",
    ("flint-cli", "crate", "flint-procgen-cli"): "#1572: gen --ai-*'s point in procgen's gen",

}


def manifests():
    features, crates = {}, {}
    for file in sorted((REPO / "packages").glob("*/package.toml")):
        with open(file, "rb") as f:
            table = tomllib.load(f)
        name = table["package"]["name"]
        for feature in table.get("build", {}).get("features", []):
            features[feature] = name
        if table["package"].get("tier") == "kernel":
            continue
        for krate in table.get("provides", {}).get("crates", []):
            crates[krate] = name
    return features, crates


# --- reading Rust well enough --------------------------------------------


def blank(text, keep_strings):
    """`text` with comments (and, unless kept, string and char literals)
    replaced by spaces, newlines kept, so offsets and lines still match."""
    out = list(text)
    i, n = 0, len(text)

    def wipe(a, b):
        for k in range(a, b):
            if out[k] != "\n":
                out[k] = " "

    while i < n:
        c = text[i]
        if text.startswith("//", i):
            j = text.find("\n", i)
            j = n if j < 0 else j
            wipe(i, j)
            i = j
        elif text.startswith("/*", i):
            depth, j = 1, i + 2
            while j < n and depth:
                if text.startswith("/*", j):
                    depth, j = depth + 1, j + 2
                elif text.startswith("*/", j):
                    depth, j = depth - 1, j + 2
                else:
                    j += 1
            wipe(i, j)
            i = j
        elif re.match(r'b?r(#*)"', text[i:i + 8]) and (i == 0 or not (text[i - 1].isalnum() or text[i - 1] == "_")):
            m = re.match(r'b?r(#*)"', text[i:])
            end = '"' + m.group(1)
            j = text.find(end, i + m.end())
            j = n if j < 0 else j + len(end)
            if not keep_strings:
                wipe(i + 1, j - 1)
            i = j
        elif c == '"':
            j = i + 1
            while j < n and text[j] != '"':
                j += 2 if text[j] == "\\" else 1
            if not keep_strings:
                wipe(i + 1, j)
            i = j + 1
        elif c == "'":
            # A char literal ('a', '\n', '\u{..}'), else a lifetime
            m = re.match(r"'(\\u\{[0-9a-fA-F]+\}|\\.|[^\\'])'", text[i:])
            if m:
                if not keep_strings:
                    wipe(i + 1, i + m.end() - 1)
                i += m.end()
            else:
                i += 1
        else:
            i += 1
    return "".join(out)


def item_end(code, at):
    """Where the item starting at `at` ends: its matching `}` or its `;`."""
    depth, i = 0, at
    while i < len(code):
        c = code[i]
        if c == "{":
            depth += 1
        elif c == "}":
            depth -= 1
            if depth == 0:
                return i + 1
        elif c == ";" and depth == 0:
            return i + 1
        i += 1
    return len(code)


TEST_ATTR = re.compile(r"#\[cfg\((?:all\()?test\b[^\]]*\]")


def test_regions(code):
    """(start, end) of each item under `#[cfg(test)]`."""
    regions = []
    for m in TEST_ATTR.finditer(code):
        # The item after this attribute and any others stacked on it
        i = m.end()
        while True:
            nxt = re.match(r"\s*#\[[^\]]*\]", code[i:])
            if not nxt:
                break
            i += nxt.end()
        regions.append((m.start(), item_end(code, i)))
    return regions


def child_modules(code, file):
    """Each `mod x;` of a file: (name, its file, declared under cfg(test))."""
    regions = test_regions(blank(code, keep_strings=False))
    kept = blank(code, keep_strings=True)
    out = []
    for m in re.finditer(r"((?:#\[[^\]]*\]\s*)*)(?:pub(?:\([^)]*\))?\s+)?mod\s+(\w+)\s*;", kept):
        attrs, name = m.group(1), m.group(2)
        here = file.parent
        if file.name not in ("lib.rs", "main.rs", "mod.rs"):
            here = here / file.stem
        path = re.search(r'#\[path\s*=\s*"([^"]+)"\]', attrs)
        if path:
            candidates = [file.parent / path.group(1)]
        else:
            candidates = [here / f"{name}.rs", here / name / "mod.rs"]
        found = next((c for c in candidates if c.is_file()), None)
        in_test = bool(TEST_ATTR.search(attrs)) or any(a <= m.start() < b for a, b in regions)
        if found:
            out.append((found.resolve(), in_test))
    return out


def test_files(src):
    """Every file under `src` that is test code: a module declared under
    `#[cfg(test)]`, a file starting `#![cfg(test)]`, and their children."""
    roots = [src / "lib.rs", src / "main.rs", *sorted((src / "bin").glob("*.rs"))]
    test, seen = set(), set()
    queue = [(r.resolve(), False) for r in roots if r.is_file()]
    while queue:
        file, in_test = queue.pop()
        if (file, in_test) in seen:
            continue
        seen.add((file, in_test))
        code = file.read_text(encoding="utf-8")
        if re.match(r"\s*(//[^\n]*\n\s*)*#!\[cfg\(test\)\]", code):
            in_test = True
        if in_test:
            test.add(file)
        for child, child_test in child_modules(code, file):
            queue.append((child, in_test or child_test))
    return test


# --- the check -------------------------------------------------------------


def main():
    features, crates = manifests()
    found = {}  # (launcher, kind, name) -> [file:line]
    for launcher, src in LAUNCHERS.items():
        src = REPO / src
        tests = test_files(src)
        for file in sorted(src.rglob("*.rs")):
            text = file.read_text(encoding="utf-8")
            code = blank(text, keep_strings=True)
            bare = blank(text, keep_strings=False)
            shown = file.relative_to(REPO).as_posix()
            line = lambda at: code.count("\n", 0, at) + 1  # noqa: E731
            for m in re.finditer(r'feature\s*=\s*"([^"]+)"', code):
                if m.group(1) in features:
                    key = (launcher, "feature", m.group(1))
                    found.setdefault(key, []).append(f"{shown}:{line(m.start())}")
            whole_test = file.resolve() in tests
            regions = [] if whole_test else test_regions(bare)
            for krate in crates:
                ident = krate.replace("-", "_")
                for m in re.finditer(rf"(?<![\w:]){ident}\b", bare):
                    if whole_test or any(a <= m.start() < b for a, b in regions):
                        continue
                    key = (launcher, "crate", krate)
                    found.setdefault(key, []).append(f"{shown}:{line(m.start())}")

    failures = []
    for key, places in sorted(found.items()):
        if key in EXCEPTIONS:
            continue
        launcher, kind, name = key
        package = features[name] if kind == "feature" else crates[name]
        what = f'`feature = "{name}"`' if kind == "feature" else f"the crate {name}"
        failures.append(
            f"{LAUNCHERS[launcher]} names the `{package}` package: {what} at "
            + ", ".join(places[:5])
            + (f" and {len(places) - 5} more" if len(places) > 5 else "")
        )
    for key, why in EXCEPTIONS.items():
        if key not in found:
            launcher, kind, name = key
            failures.append(
                f"the exception for {kind} `{name}` in {launcher} matches nothing now "
                f"({why}): take it out of EXCEPTIONS in scripts/package-names-check.py"
            )
    if failures:
        for f in failures:
            print(f"package-names-check: {f}", file=sys.stderr)
        print(
            "package-names-check: a package joins the launchers through its manifest "
            "(ADR 0137 section 5): move the code into the package's crate, behind a "
            "point the kernel declares",
            file=sys.stderr,
        )
        return 1
    held = sum(1 for k in EXCEPTIONS)
    print(
        f"package-names-check: no package named in {', '.join(LAUNCHERS.values())} "
        f"({len(features)} features, {len(crates)} crates; {held} exceptions, each an issue)"
    )
    return 0


if __name__ == "__main__":
    sys.exit(main())
