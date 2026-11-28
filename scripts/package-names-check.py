#!/usr/bin/env python3
"""No package named in the kernel's source (epic #1548's gate, #1560).

The kernel is every crate `packages/engine/package.toml` lists under
`[provides] crates` (#1591; the launchers, `crates/flint-player/src` and
`crates/flint-cli/src`, alone before it): a package joins it through its
manifest (ADR 0137 section 5) or a point the kernel declares, never by
name. This fails on each way the source can name one:

  1. a Cargo feature gate, `feature = "<f>"` for any `<f>` a manifest's
     `[build] features` lists, in code or test code (a `cfg`, a `cfg_attr`,
     a `cfg!`): the way Phase 1's moves came undone (#1330);
  2. a path through a package's crate (`flint_living::...`, `use
     flint_music`), for any crate a manifest's `[provides] crates` lists,
     outside test code. Tests may name a package's crate: they run where a
     launcher lists it (the dev-dependencies, #1558, #1559);
  3. a string literal that is a package's name, `"living"`, outside test
     code (#1585 in the player, #1595 the CLI, #1591 every kernel crate):
     it builds without the package, but it is still the kernel knowing one
     by name (`registry.has("living")`, a policy, a flag's owner, the
     package a scan credits, a scene table's rule). What the kernel says or
     takes of a package comes from its manifest or its registration.

Comments are not read, nor are string literals for (2). A word inside a
longer string (`"[procgen] ..."`) is not (3): only a literal that is the
name. The kernel's own name, `engine`, is its own.

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


def kernel_crates():
    """Each crate the engine manifest provides, by name: its `src/`. A crate
    the manifest names that has no `crates/<name>/src` fails, so the check
    never shrinks in silence."""
    with open(REPO / "packages/engine/package.toml", "rb") as f:
        listed = tomllib.load(f)["provides"]["crates"]
    return {name: f"crates/{name}/src" for name in listed}


# (crate, "feature" | "crate" | "string", name): why it is still there.
# A "string" exception may name a file, (crate, "string", name, file):
# only that file's literal is excused.
EXCEPTIONS = {
    ("flint-core", "string", "particles", "crates/flint-core/src/project/assets.rs"): "the "
    "asset search's `particles/` folder and the `particles` kind (`AssetKind::Particles`, "
    "`ParticleTextures`): the folder a project keeps its effects and their textures in, "
    "as `models/` and `sprites/` are, which the kernel's one resolver searches (ADR 0114), "
    "not the package's name (#1622)",
    ("flint-cli", "string", "particles", "crates/flint-cli/src/check/references.rs"): "the "
    "asset folders `flint check` looks for a reference under, the resolver's `particles/` "
    "among them (ADR 0114), not the package's name (#1622)",
    ("flint-core", "string", "particles", "crates/flint-core/src/load_report.rs"): "the load "
    "report's `particles` row, the stage of a scene's bring-up the particles package's "
    "builder times (`NoteKind::ParticleLoad`), named for what is loaded there as `models` "
    "and `scripts` are (#1622)",
    ("flint-cli", "crate", "flint-animation"): "the model previewer (`preview.rs`), "
    "`flint render --anim-time` (`HeadlessAnimation`) and the render's one IK solve "
    "(`IkPass`) name the animation package's crate: skeletons, IK and the previewer "
    "are `rig`'s (ADR 0139 section 3), and #1621 puts them behind registration",
    ("flint-core", "string", "animation", "crates/flint-core/src/load_report.rs"): "the "
    "load report's `animation` row, the kernel's stage of a scene's bring-up that the "
    "animation package's builder times (`NoteKind::AnimationLoad`), named for what is "
    "loaded there as `models` and `scripts` are",
    ("flint-package", "string", "animation", "crates/flint-package/src/stage.rs"): "the "
    "label of `Stage::Animation` (after the animation, the pose and the particles), a "
    "kernel stage every package may hear, which a profiler scope names",
    ("flint-player", "string", "music", "crates/flint-player/src/logging.rs"): "a log "
    "target the engine's warnings are counted under (`flint_target`), as `load` and "
    "`perf` are: the music package logs as `music`, and so did the player before it",
    ("flint-script", "string", "music", "crates/flint-script/src/api/music.rs"): "the "
    "`music` script module, declared here until Phase 3 (the music manifest says so): "
    "#1619 moves it to the music package",
    ("flint-audio", "string", "music", "crates/flint-audio/src/bus.rs"): "the built-in "
    "`music` bus, which every game's audio has: #1619 makes flint-audio the audio "
    "package's, out of the kernel",
    # The frame's fixed-step part is named `physics` wherever the kernel
    # times or labels it, whichever package steps in it (#1617): the word,
    # not the package
    ("flint-player", "string", "physics", "crates/flint-player/src/player_app/perf.rs"): "the "
    "frame's `physics` phase (the fixed steps and the eye), as the perf line and the frame "
    "history have always named it",
    ("flint-package", "string", "physics", "crates/flint-package/src/stage.rs"): "`Stage::Physics`'s "
    "label, the stage after the fixed steps, which a profiler scope's detail names",
    ("flint-package", "string", "physics", "crates/flint-package/src/hud.rs"): "the frame graph's "
    "`physics` band, the frame's fixed-step phase",
    ("flint-cli", "string", "physics", "crates/flint-cli/src/commands/frame/open/timing.rs"): "a "
    "capture's `physics` phase, read into the frame graph's band",
    ("flint-cli", "string", "physics", "crates/flint-cli/src/commands/trace/profile.rs"): "a "
    "trace's `physics::*` scopes, banded with the frame's fixed-step phase",
    ("flint-core", "string", "physics", "crates/flint-core/src/load_report.rs"): "the load "
    "report's `physics` row (`NoteKind::PhysicsBuild`'s stage), which the package's build notes",
    ("flint-core", "string", "physics", "crates/flint-core/src/project.rs"): "the manifest's "
    "`[project.physics] layers`, a game's own collision layers, which `flint check` reads in "
    "every build",
    ("flint-runtime", "string", "physics", "crates/flint-runtime/src/state.rs"): "the `physics` "
    "policy name, kept as a constant of the name (ADR 0138 decision 7): a game state that names "
    "it registers in every build",
}


def manifests():
    features, crates, names = {}, {}, set()
    for file in sorted((REPO / "packages").glob("*/package.toml")):
        with open(file, "rb") as f:
            table = tomllib.load(f)
        name = table["package"]["name"]
        for feature in table.get("build", {}).get("features", []):
            features[feature] = name
        if table["package"].get("tier") == "kernel":
            continue
        names.add(name)
        for krate in table.get("provides", {}).get("crates", []):
            crates[krate] = name
    return features, crates, names


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
    features, crates, names = manifests()
    kernel = kernel_crates()
    found = {}  # (crate, kind, name[, file]) -> [file:line]
    failures = []
    for launcher, src in kernel.items():
        src = REPO / src
        if not src.is_dir():
            failures.append(
                f"packages/engine/package.toml lists {launcher}, which has no {kernel[launcher]}"
            )
            continue
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
            if whole_test:
                continue
            for m in re.finditer(r'(?<![\w#])"([a-z][a-z0-9-]*)"', code):
                if m.group(1) not in names:
                    continue
                if any(a <= m.start() < b for a, b in regions):
                    continue
                key = (launcher, "string", m.group(1), shown)
                if key not in EXCEPTIONS:
                    key = (launcher, "string", m.group(1))
                found.setdefault(key, []).append(f"{shown}:{line(m.start())}")

    for key, places in sorted(found.items()):
        if key in EXCEPTIONS:
            continue
        launcher, kind, name = key[:3]
        package = {"feature": features, "crate": crates}.get(kind, {}).get(name, name)
        what = {
            "feature": f'`feature = "{name}"`',
            "crate": f"the crate {name}",
            "string": f'the string "{name}"',
        }[kind]
        failures.append(
            f"{kernel[launcher]} names the `{package}` package: {what} at "
            + ", ".join(places[:5])
            + (f" and {len(places) - 5} more" if len(places) > 5 else "")
        )
    for key, why in EXCEPTIONS.items():
        if key not in found:
            launcher, kind, name = key[:3]
            failures.append(
                f"the exception for {kind} `{name}` in {launcher} matches nothing now "
                f"({why}): take it out of EXCEPTIONS in scripts/package-names-check.py"
            )
    if failures:
        for f in failures:
            print(f"package-names-check: {f}", file=sys.stderr)
        print(
            "package-names-check: a package joins the kernel through its manifest "
            "(ADR 0137 section 5): move the code into the package's crate, behind a "
            "point the kernel declares",
            file=sys.stderr,
        )
        return 1
    held = sum(1 for k in EXCEPTIONS)
    print(
        f"package-names-check: no package named in the {len(kernel)} kernel crates' src/ "
        f"({len(features)} features, {len(crates)} crates, {len(names)} names; "
        f"{held} exceptions, each with its reason)"
    )
    return 0


if __name__ == "__main__":
    sys.exit(main())
