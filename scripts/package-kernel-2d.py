#!/usr/bin/env python3
"""Epic #1611's gate the way a game meets it (#1624): the 2d template builds
and plays on the kernel alone. CI's `kernel 2d` job runs it.

scripts/package-deps-check.py holds the crate graph to the rule. This is the
promise the rule is for: a game can leave every package out. A flint with no
package makes a project with `flint init --template 2d`, which writes its
own five `= false` lines (ADR 0138 decision 7); this sets every other
standard package `false` too, so `[packages]` drops the whole standard tier
and lists no feature package. Then:

  1. `flint build --json` builds the project's flint-player and flint; its
     plan puts no package in, the cargo tree of its own command holds no
     package's crate at all, and the built flint says it has no package;
  2. the built flint's `check` passes on the project;
  3. the built flint's `playtest` plays the template's scene headless with
     an input script: D moves the player right, W up, the HUD draws its help
     line, and no script errs or load warns (`--strict`, which runs `flint
     check` on the scene first; tests/fixtures/packages/kernel-2d.playtest.txt);
  4. the built binaries' sizes and crate counts are printed, as context
     (no budget reads them, ADR 0138 decision 8).

  python3 scripts/package-kernel-2d.py            build the bootstrap flint first
  python3 scripts/package-kernel-2d.py --flint F  start from the flint F

It builds a debug flint-player and flint into the checkout's target dir, so
a Linux runner needs scripts/linux-build-deps.sh first: a game's build keeps
sound, gamepads and the file dialogs. The project is
target/tmp/package-kernel-2d/game, kept after the run to look at.
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
PLAYTEST = REPO / "tests" / "fixtures" / "packages" / "kernel-2d.playtest.txt"
# What the 2d template writes itself (ADR 0138 decision 7)
TEMPLATE_FALSE = ["physics", "first-person", "rig", "particles", "terrain"]

failures = []


def say(line):
    print(f"kernel-2d: {line}", flush=True)


def fail(line):
    failures.append(line)
    print(f"FAIL {line}", file=sys.stderr, flush=True)
    if os.environ.get("GITHUB_ACTIONS"):
        print(f"::error title=kernel 2d::{line}", flush=True)


def run(words, cwd, check=True):
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
        sys.exit(f"kernel-2d: `{shown}` in {cwd} exited {out.returncode}")
    return out


def document(out, words):
    try:
        return json.loads(out.stdout)
    except json.JSONDecodeError as e:
        sys.stderr.write(out.stdout[-4000:] + out.stderr[-4000:])
        sys.exit(f"kernel-2d: `{' '.join(str(w) for w in words)}` printed no JSON document: {e}")


def manifests():
    found = {}
    for path in sorted((REPO / "packages").glob("*/package.toml")):
        with open(path, "rb") as f:
            m = tomllib.load(f)
        found[m["package"]["name"]] = m
    return found


def target_dir():
    out = run(["cargo", "metadata", "--format-version", "1", "--no-deps"], REPO)
    return Path(json.loads(out.stdout)["target_directory"])


def bootstrap(args, scratch):
    """A flint with no package, copied aside: `flint build` writes over the
    target dir's flint."""
    if "--flint" in args:
        source = Path(args[args.index("--flint") + 1]).resolve()
    else:
        say("cargo build -p flint-cli --no-default-features (the bootstrap flint)")
        run(["cargo", "build", "-q", "-p", "flint-cli", "--no-default-features", "--bin", "flint"], REPO)
        source = target_dir() / "debug" / f"flint{EXE}"
    copy = scratch / "bootstrap" / f"flint{EXE}"
    copy.parent.mkdir(parents=True)
    shutil.copy2(source, copy)
    return copy


def make_game(flint, scratch, packages):
    run([flint, "init", "game", "--template", "2d", "--engine-path", REPO], scratch)
    game = scratch / "game"
    path = game / "flint.toml"
    with open(path, "rb") as f:
        listed = tomllib.load(f).get("packages")
    if listed is None:
        sys.exit("kernel-2d: flint init wrote no [packages] table")
    wrote = sorted(n for n, v in listed.items() if v is False)
    if wrote != sorted(TEMPLATE_FALSE):
        fail(f"the 2d template set {wrote} false; ADR 0138 decision 7 says {sorted(TEMPLATE_FALSE)}")
    else:
        say(f"flint init --template 2d: [packages] sets {wrote} false")
    if any(v is not False for v in listed.values()):
        fail(f"the 2d template lists a package in: {listed}")
    standard = sorted(n for n, m in packages.items() if m["package"]["tier"] == "standard")
    more = [n for n in standard if n not in listed]
    text = path.read_text(encoding="utf-8")
    if not text.endswith("\n"):
        text += "\n"
    text += "# The gate (#1624): the rest of the standard tier, so the game is the kernel alone\n"
    text += "".join(f"{n} = false\n" for n in more)
    path.write_text(text, encoding="utf-8")
    say(f"and the gate sets {more} false: the kernel alone")
    return game


def build(flint, game, packages):
    words = [flint, "build", "--json"]
    out = run(words, game, check=False)
    doc = document(out, words)
    if out.returncode != 0 or not doc.get("built"):
        sys.stderr.write(out.stderr[-4000:])
        sys.exit(f"kernel-2d: flint build failed: {doc.get('error')}")
    included = sorted(p["name"] for p in doc["packages"] if packages.get(p["name"], {}).get("package", {}).get("tier") != "kernel")
    say(f"flint build: {doc['seconds']:.0f} s; packages in {included or 'none but the kernel'}")
    if included:
        fail(f"flint build put packages in a kernel-only game: {included}")
    others = sorted(n for n, m in packages.items() if m["package"]["tier"] != "kernel")
    missing = [n for n in others if n not in doc["left_out"]]
    if missing:
        fail(f"flint build doesn't say it left out {missing}")

    # What the build's own cargo command links: no package's crate at all
    tree = set()
    for argv in doc["argv"]:
        words = ["cargo", "tree"]
        rest = iter(argv[2:])
        for w in rest:
            if w == "--bin":
                next(rest, None)
            elif w != "--release":
                words.append(w)
        words += ["-q", "-e", "normal", "--prefix", "none"]
        where = doc["dir"] if Path(doc["dir"]).is_absolute() else game / doc["dir"]
        tree |= {line.split(" ")[0] for line in run(words, where).stdout.splitlines() if line.strip()}
    linked = sorted(
        f"{c} ({n})" for n in others for c in packages[n].get("provides", {}).get("crates", []) if c in tree
    )
    if linked:
        fail(f"the kernel-only binaries link package crates: {linked}")
    else:
        say(f"tree: {len(tree)} crates, none of any package's")

    built = {b: Path(p) if Path(p).is_absolute() else game / p for b, p in doc["binaries"].items()}
    words = [built["flint"], "package", "list", "--built", "--json"]
    has = document(run(words, game), words)
    have = sorted(p["name"] for p in has["packages"] if p["tier"] != "kernel")
    if have:
        fail(f"the built flint says it has {have}")
    else:
        say("the built flint has no package but the kernel")
    return built, len(tree)


def check(flint, game):
    words = [flint, "check", "--json"]
    out = run(words, game, check=False)
    doc = document(out, words)
    if out.returncode != 0 or not doc.get("valid"):
        shown = [f"{d['code']}: {d['message']}" for d in doc.get("diagnostics", [])]
        fail(f"flint check on the kernel-only 2d game exited {out.returncode}: {shown}")
    else:
        say(f"flint check: {doc['summary']}")


def playtest(flint, game):
    script = game / "playtests" / "kernel.playtest.txt"
    script.parent.mkdir(exist_ok=True)
    shutil.copy2(PLAYTEST, script)
    words = [flint, "playtest", "scenes/main.scene.toml", "--input-script", script.relative_to(game),
             "--exit-after", "1.5", "--strict", "--json"]
    out = run(words, game, check=False)
    doc = document(out, words)
    checks = doc.get("checks", [])
    held = sum(1 for c in checks if c.get("passed"))
    if out.returncode != 0 or not doc.get("passed"):
        shown = [f.get("message") or f if isinstance(f, dict) else f for f in doc.get("failures", [])]
        fail(f"flint playtest of the kernel-only 2d game failed (exit {out.returncode}): {shown}")
    elif not checks or held != len(checks):
        fail(f"flint playtest passed but held {held} of {len(checks)} checks")
    else:
        say(f"flint playtest --strict: passed, {held} of {len(checks)} checks held, "
            f"{doc.get('frames')} frames, ended by {doc.get('ended')} at {doc.get('game_time')} s")


def sizes(built, crates):
    """Context only (ADR 0138 decision 8)."""
    for name, path in sorted(built.items()):
        mb = path.stat().st_size / 1e6
        say(f"size: {name} (debug) {mb:.1f} MB")
    say(f"size: {crates} crates in the build's tree")


def main(args):
    if "-h" in args or "--help" in args:
        print(__doc__)
        return 0
    scratch = target_dir() / "tmp" / "package-kernel-2d"
    try:
        if scratch.exists():
            shutil.rmtree(scratch)
    except OSError as e:
        sys.exit(f"kernel-2d: can't clear the last run's {scratch} ({e}); is a shell or binary still in it?")
    scratch.mkdir(parents=True)
    packages = manifests()
    flint = bootstrap(args, scratch)
    game = make_game(flint, scratch, packages)
    built, crates = build(flint, game, packages)
    check(built["flint"], game)
    playtest(built["flint"], game)
    sizes(built, crates)
    if failures:
        print(f"\npackage-kernel-2d: {len(failures)} check(s) failed:", file=sys.stderr)
        for f in failures:
            print(f"  {f}", file=sys.stderr)
        return 1
    print("package-kernel-2d: ok: the 2d template builds and plays on the kernel alone")
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
