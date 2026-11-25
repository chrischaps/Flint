#!/usr/bin/env python3
"""Epic #1328's three "Done when" checks on a project `flint init` made and
`flint build` built (#1525). CI's `package done-when` job runs it.

The package matrix (scripts/package-check.sh) reads the trees of the
configurations it makes up. This is a game's way in instead: a flint with no
package makes a project, the project lists none of the feature packages (init
writes `[packages]` empty), `flint build` builds its player and CLI, and the
binaries it built are asked:

  1. the build: its plan leaves every feature package out, it builds, the
     cargo tree of its own command holds no crate of a package it left out,
     and the built flint says it has exactly the packages the plan put in;
  2. `flint schema --list --json`, of the built flint in the project, lists
     no component of a left-out package (living_prop, shallow_wooze,
     music_session);
  3. `flint check` on a scene with a `living_prop` exits 1, and the
     diagnostic names the package and the line to add.

Then, with `living = "*"` listed, the built flint's check says this binary
lacks it (`unbuilt-package`, naming `flint build`) and the built player
refuses the scene the same way: what was left out is really not there.

  python3 scripts/package-done-when.py            build the bootstrap flint first
  python3 scripts/package-done-when.py --flint F  start from the flint F

It builds a debug flint-player and flint (minutes cold) into the checkout's
target dir, so a Linux runner needs scripts/linux-build-deps.sh first: a
game's build keeps sound, gamepads and the file dialogs. The project is
target/tmp/package-done-when/game, kept after the run to look at.
Standard library only (tomllib: Python 3.11).
"""

import json
import os
import shutil
import subprocess
import sys
import tomllib
from pathlib import Path

REPO = Path(__file__).resolve().parent.parent
EXE = ".exe" if os.name == "nt" else ""
# The scene a game that left `living` out can't have, and what it says
GROVE = """[scene]
name = "grove"

[entities.oak.transform]
position = [0.0, 0.0, 0.0]

[entities.oak.living_prop]
lineage_seed = 3
"""
# The components the epic's box names. They are also read off the
# manifests; these are held to be among what that reading finds.
NAMED = ["living_prop", "shallow_wooze", "music_session"]
# No exception: since #1516 flint-procgen links flint-wooze only with its
# `living` feature, so a build that leaves living out links none of
# living's crates. (package-check.sh's one exception, flint-cli linking
# flint-procgen always (#1543), can't arise here: procgen is in.)

failures = []


def say(line):
    print(f"done-when: {line}", flush=True)


def fail(line):
    failures.append(line)
    print(f"FAIL {line}", file=sys.stderr, flush=True)
    if os.environ.get("GITHUB_ACTIONS"):
        print(f"::error title=package done-when::{line}", flush=True)


def run(words, cwd, check=True):
    """Run `words` in `cwd`, its stdout and stderr captured."""
    shown = " ".join(str(w) for w in words)
    out = subprocess.run(
        [str(w) for w in words],
        cwd=cwd,
        capture_output=True,
        text=True,
        encoding="utf-8",
        errors="replace",
    )
    if check and out.returncode != 0:
        sys.stderr.write(out.stdout + out.stderr)
        sys.exit(f"done-when: `{shown}` in {cwd} exited {out.returncode}")
    return out


def document(out, words):
    try:
        return json.loads(out.stdout)
    except json.JSONDecodeError as e:
        sys.stderr.write(out.stdout + out.stderr)
        sys.exit(f"done-when: `{' '.join(words)}` printed no JSON document: {e}")


def manifests():
    """Every package.toml under packages/, by name."""
    found = {}
    for path in sorted((REPO / "packages").glob("*/package.toml")):
        with open(path, "rb") as f:
            m = tomllib.load(f)
        found[m["package"]["name"]] = m
    return found


def target_dir():
    out = run(
        ["cargo", "metadata", "--format-version", "1", "--no-deps"], REPO
    )
    return Path(json.loads(out.stdout)["target_directory"])


def bootstrap(args, scratch):
    """A flint with no package, copied aside: `flint build` writes over
    the target dir's flint."""
    if "--flint" in args:
        source = Path(args[args.index("--flint") + 1]).resolve()
    else:
        say("cargo build -p flint-cli --no-default-features (the bootstrap flint)")
        run(
            ["cargo", "build", "-q", "-p", "flint-cli", "--no-default-features",
             "--bin", "flint"],
            REPO,
        )
        source = target_dir() / "debug" / f"flint{EXE}"
    copy = scratch / "bootstrap" / f"flint{EXE}"
    copy.parent.mkdir(parents=True)
    shutil.copy2(source, copy)
    return copy


def make_game(flint, scratch):
    run([flint, "init", "game", "--engine-path", REPO], scratch)
    game = scratch / "game"
    with open(game / "flint.toml", "rb") as f:
        listed = tomllib.load(f).get("packages")
    if listed is None:
        sys.exit("done-when: flint init wrote no [packages] table")
    say(f"flint init: [packages] lists {sorted(listed) or 'nothing'}")
    (game / "scenes" / "grove.scene.toml").write_text(GROVE, encoding="utf-8")
    return game, listed


def set_packages(game, lines):
    path = game / "flint.toml"
    text = path.read_text(encoding="utf-8")
    at = text.index("[packages]")
    path.write_text(text[:at] + "[packages]\n" + lines, encoding="utf-8")


def check_build(flint, game, packages, listed):
    words = [flint, "build", "--json"]
    out = run(words, game, check=False)
    doc = document(out, words)
    if out.returncode != 0 or not doc.get("built"):
        sys.stderr.write(out.stderr[-4000:])
        sys.exit(f"done-when: flint build failed: {doc.get('error')}")
    included = sorted(p["name"] for p in doc["packages"])
    left_out = sorted(doc["left_out"])
    features = sorted(n for n, m in packages.items()
                      if m["package"]["tier"] == "feature" and n not in listed)
    say(f"flint build: {doc['seconds']:.0f} s; in {included}, left out {left_out}")
    for p in features:
        if p not in left_out:
            fail(f"flint build put the unlisted feature package `{p}` in")
    for p in included:
        if p in left_out:
            fail(f"flint build has `{p}` both in and left out")

    # 1a. What the build's own cargo command links
    tree = set()
    for argv in doc["argv"]:
        if argv[1] != "build":
            sys.exit(f"done-when: the build's command isn't `cargo build`: {argv}")
        # The same words as `cargo tree`, which takes no --bin or --release
        words = ["cargo", "tree"]
        rest = iter(argv[2:])
        for w in rest:
            if w == "--bin":
                next(rest, None)
            elif w != "--release":
                words.append(w)
        words += ["-q", "-e", "normal", "--prefix", "none"]
        lines = run(words, doc["dir"] if Path(doc["dir"]).is_absolute()
                    else game / doc["dir"]).stdout.splitlines()
        tree |= {line.split(" ")[0] for line in lines if line.strip()}
    before = len(failures)
    for p in left_out:
        for c in packages[p].get("provides", {}).get("crates", []):
            if c in tree:
                fail(f"the built binaries link {c}, of the left-out package `{p}`")
    if len(failures) == before:
        say(f"tree: {len(tree)} crates, none of a left-out package's")

    # 1b. The built flint says what it has
    built = {b: Path(p) if Path(p).is_absolute() else game / p
             for b, p in doc["binaries"].items()}
    words = [built["flint"], "package", "list", "--built", "--json"]
    has = document(run(words, game), words)
    # --built lists only the packages this binary was built with
    have = sorted(p["name"] for p in has["packages"] if p["tier"] != "kernel")
    wanted = sorted(p for p in included if packages[p]["package"]["tier"] != "kernel")
    if have != wanted:
        fail(f"the built flint has {have}, the build put in {wanted}")
    else:
        say(f"the built flint has {have}")
    return built, left_out


def check_schemas(flint, game, packages, left_out):
    words = [flint, "schema", "--list", "--json"]
    doc = document(run(words, game), words)
    listed = {c if isinstance(c, str) else c["name"] for c in doc["components"]}
    if "transform" not in listed:
        fail(f"flint schema lists no transform: {sorted(listed)}")
    gone = [c for p in left_out
            for c in packages[p].get("provides", {}).get("components", [])]
    for c in NAMED:
        if c not in gone:
            fail(f"`{c}` is no left-out package's component here")
    there = sorted(c for c in gone if c in listed)
    if there:
        fail(f"flint schema lists left-out packages' components: {there}")
    else:
        say(f"flint schema: {len(listed)} components, none of {sorted(gone)}")


def check_scene(flint, game):
    words = [flint, "check", "scenes/grove.scene.toml", "--json"]
    out = run(words, game, check=False)
    doc = document(out, words)
    if out.returncode != 1:
        fail(f"flint check on a living_prop exited {out.returncode}, not 1")
    found = [d for d in doc["diagnostics"]
             if d["code"] == "unlisted-package" and d.get("component") == "living_prop"]
    if not found:
        fail(f"flint check named no unlisted-package for the living_prop: {doc['diagnostics']}")
        return
    hint = found[0].get("hint") or ""
    if 'living = "*"' not in hint:
        fail(f"the unlisted-package hint doesn't name the line to add: {hint!r}")
    else:
        say(f"flint check: exit 1, {found[0]['message']!r}; hint {hint!r}")


def check_binaries_lack_it(built, game):
    set_packages(game, 'living = "*"\n')
    words = [built["flint"], "check", "--json"]
    out = run(words, game, check=False)
    doc = document(out, words)
    found = [d for d in doc["diagnostics"] if d["code"] == "unbuilt-package"]
    said = " ".join(f"{d['message']} {d.get('hint') or ''}" for d in found)
    if out.returncode != 1 or "living" not in said or "flint build" not in said:
        fail(f"with living listed, the built flint's check said {said!r} "
             f"(exit {out.returncode})")
    else:
        say(f"living listed: the built flint says {found[0]['message']!r}")
    words = [built["flint-player"], "scenes/grove.scene.toml", "--exit-after", "1"]
    out = run(words, game, check=False)
    said = out.stdout + out.stderr
    if out.returncode == 0 or "living" not in said or "flint build" not in said:
        fail(f"with living listed, the built player exited {out.returncode}: {said[-600:]!r}")
    else:
        line = next((l for l in said.splitlines() if "flint build" in l), said.strip())
        say(f"living listed: the built player exits {out.returncode}: {line.strip()!r}")
    set_packages(game, "")


def main(args):
    if "-h" in args or "--help" in args:
        print(__doc__)
        return 0
    scratch = target_dir() / "tmp" / "package-done-when"
    try:
        if scratch.exists():
            shutil.rmtree(scratch)
    except OSError as e:
        sys.exit(f"done-when: can't clear the last run's {scratch} ({e}); "
                 "is a shell or binary still in it?")
    scratch.mkdir(parents=True)
    packages = manifests()
    flint = bootstrap(args, scratch)
    game, listed = make_game(flint, scratch)
    built, left_out = check_build(flint, game, packages, listed)
    check_schemas(built["flint"], game, packages, left_out)
    check_scene(built["flint"], game)
    check_binaries_lack_it(built, game)
    if failures:
        print(f"\npackage-done-when: {len(failures)} check(s) failed:", file=sys.stderr)
        for f in failures:
            print(f"  {f}", file=sys.stderr)
        return 1
    print("package-done-when: ok")
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
