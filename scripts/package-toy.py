#!/usr/bin/env python3
"""Phase 2a's gate (epic #1548, #1560): a package is added by creating its
folder and removed by deleting it. CI's `package done-when` job runs it.

  1. copies the toy package (tests/fixtures/packages/toy) to packages/toy/
     and runs scripts/package-sync.py, the one command ADR 0137 section 5
     lets a package add (decision 5's restated gate);
  2. holds what changed outside packages/toy/ to the generated blocks of
     crates/flint-player/Cargo.toml and crates/flint-cli/Cargo.toml (and,
     once cargo runs, Cargo.lock): a toy that needs any other edit to build
     fails here, at its build;
  3. builds the stock flint-player and flint (default features: every
     package) and asks the flint it built:
       - `package list --built --json` lists `toy`;
       - `check` on the toy's game (tests/fixtures/packages/toy-game) is
         clean, so its `toy_spring` is declared by the package's schemas;
       - `playtest` of the game's scene with its test script passes: the
         toy's subsystem pulled the spring in each fixed step (`toy_steps()`
         counts 120) and the `toy` module answered the script;
       - `toy springs <scene> --json` is the package's own command;
  4. deletes packages/toy/, runs the script again, builds both again, and
     holds the two Cargo.toml files and Cargo.lock to what they were before
     step 1, byte for byte.

Whatever happens, the folder is deleted and the three files are put back
at the end, so a failed run leaves the checkout as it found it.

  python3 scripts/package-toy.py

Builds a debug flint-player and flint with every package (minutes cold) in
the checkout's target dir, so a Linux runner needs scripts/linux-build-deps.sh
first. Standard library only.
"""

import json
import os
import shutil
import subprocess
import sys
from pathlib import Path

REPO = Path(__file__).resolve().parent.parent
FIXTURE = REPO / "tests/fixtures/packages/toy"
GAME = REPO / "tests/fixtures/packages/toy-game"
TOY = REPO / "packages/toy"
EXE = ".exe" if os.name == "nt" else ""
PLAYER_TOML = "crates/flint-player/Cargo.toml"
CLI_TOML = "crates/flint-cli/Cargo.toml"
KEPT = [PLAYER_TOML, CLI_TOML, "Cargo.lock"]
STEPS = 120

import importlib.util  # noqa: E402

# package-sync.py's block reader, so both scripts read the markers alike
_spec = importlib.util.spec_from_file_location("package_sync", REPO / "scripts/package-sync.py")
sync_module = importlib.util.module_from_spec(_spec)
_spec.loader.exec_module(sync_module)


class Failed(Exception):
    pass


def say(line):
    print(f"package-toy: {line}", flush=True)


def annotate(message):
    if os.environ.get("GITHUB_ACTIONS"):
        print(f"::error title=package toy::{message}", flush=True)


def run(words, **kw):
    say("$ " + " ".join(str(w) for w in words))
    return subprocess.run([str(w) for w in words], cwd=REPO, **kw)


def changed_paths():
    """What git sees changed or new, by path (untracked folders listed
    file by file)."""
    out = subprocess.run(
        ["git", "status", "--porcelain", "--untracked-files=all"],
        cwd=REPO,
        capture_output=True,
        text=True,
        check=True,
    ).stdout
    return {line[3:].strip().strip('"') for line in out.splitlines() if line.strip()}


def outside_blocks(text):
    """A Cargo.toml with its generated blocks emptied: what a hand edit
    would change."""
    for block in ("features", "dependencies"):
        head, _, tail = sync_module.split(text, block)
        text = head + tail
    return text


def sync():
    if run([sys.executable, "scripts/package-sync.py"]).returncode != 0:
        raise Failed("scripts/package-sync.py refused the packages (its line above says why)")


def build(why):
    target = Path(os.environ.get("CARGO_TARGET_DIR", REPO / "target"))
    if run(["cargo", "build", "-p", "flint-player", "-p", "flint-cli"]).returncode != 0:
        raise Failed(
            f"the stock flint-player and flint don't build {why}: a package that "
            "needs an edit outside its folder fails here"
        )
    return target / "debug" / f"flint{EXE}"


def ask(flint, words, what):
    result = run([flint, "--json", *words], capture_output=True, text=True)
    sys.stderr.write(result.stderr[-4000:])
    try:
        answer = json.loads(result.stdout)
    except json.JSONDecodeError:
        raise Failed(f"{what}: no JSON document on stdout (exit {result.returncode})")
    return result.returncode, answer


def gate(before):
    # 1. The folder, and the one command
    shutil.copytree(FIXTURE, TOY)
    say(f"copied {FIXTURE.relative_to(REPO).as_posix()} to packages/toy/")
    sync()

    # 2. Nothing else changed by hand
    stray = sorted(
        p
        for p in changed_paths() - before["changed"]
        if not p.startswith("packages/toy/") and p not in KEPT
    )
    if stray:
        raise Failed(f"adding packages/toy/ changed files outside it: {', '.join(stray)}")
    for f in (PLAYER_TOML, CLI_TOML):
        now = (REPO / f).read_text(encoding="utf-8")
        if outside_blocks(now) != outside_blocks(before["text"][f]):
            raise Failed(f"{f} changed outside its generated blocks")
        if now == before["text"][f]:
            raise Failed(f"{f}: package-sync.py wrote no line for the toy")
    say("outside packages/toy/ only the generated blocks changed")

    # 3. The stock binaries have it
    flint = build("with packages/toy/")
    code, listed = ask(flint, ["package", "list", "--built"], "package list --built")
    # With --built, `packages` is only those this binary has
    built = [p["name"] for p in listed.get("packages", [])]
    if code != 0 or "toy" not in built:
        raise Failed(f"`flint package list --built` doesn't list toy (built: {built})")
    say(f"the stock flint is built with: {', '.join(built)}")

    code, checked = ask(flint, ["check", GAME], "check")
    if code != 0 or not checked.get("valid"):
        problems = [d.get("message") for d in checked.get("diagnostics", [])]
        raise Failed(f"`flint check` on the toy's game isn't clean: {problems}")
    say("`flint check` on the toy's game is clean: toy_spring is declared")

    scene = GAME / "scenes/spring.scene.toml"
    code, report = ask(
        flint,
        [
            "playtest",
            scene,
            "--test-script",
            GAME / "playtests/swing.rhai",
            "--timeout",
            "10",
        ],
        "playtest",
    )
    log = report.get("log", {})
    if code != 0 or not report.get("passed"):
        raise Failed(f"the toy's playtest failed: {report.get('failures')}")
    if log.get("steps") != STEPS or log.get("springs") != 1:
        raise Failed(f"the toy's playtest heard {log}: wanted {STEPS} steps and 1 spring")
    say(
        f"playtest passed: {log['steps']} fixed steps pulled, bob swung to "
        f"x = {log.get('lowest'):.3f} m, the `toy` module answered"
    )

    code, springs = ask(flint, ["toy", "springs", scene], "toy springs")
    names = [s.get("entity") for s in springs.get("springs", [])]
    if code != 0 or names != ["bob"]:
        raise Failed(f"`flint toy springs` answered {springs}")
    say("`flint toy springs` is the package's own command: bob, k = 4")

    # 4. Deleting the folder takes it out again
    shutil.rmtree(TOY)
    say("deleted packages/toy/")
    sync()
    build("once packages/toy/ is deleted")
    for f in KEPT:
        now, then = (REPO / f).read_bytes(), before["bytes"][f]
        if f == "Cargo.lock":
            # cargo writes the lock with LF, whatever the checkout's endings
            now, then = now.replace(b"\r\n", b"\n"), then.replace(b"\r\n", b"\n")
        if now != then:
            raise Failed(f"{f} isn't what it was before the toy came in")
    # The three files were held above (git may call an LF-written lock
    # changed in a CRLF checkout); everything else is as git saw it
    stray = sorted(p for p in changed_paths() ^ before["changed"] if p not in KEPT)
    if stray:
        raise Failed(f"the checkout isn't as it was: {', '.join(stray)}")
    say("deleted, synced and rebuilt: the checkout is byte for byte as before")


def main():
    if TOY.exists():
        print("package-toy: packages/toy/ is there already; delete it first", file=sys.stderr)
        return 2
    before = {
        "bytes": {f: (REPO / f).read_bytes() for f in KEPT},
        "text": {f: (REPO / f).read_text(encoding="utf-8") for f in (PLAYER_TOML, CLI_TOML)},
        "changed": changed_paths(),
    }
    try:
        gate(before)
    except Failed as e:
        print(f"package-toy: FAIL {e}", file=sys.stderr)
        annotate(str(e))
        return 1
    finally:
        if TOY.exists():
            shutil.rmtree(TOY)
        for f in KEPT:
            if (REPO / f).read_bytes() != before["bytes"][f]:
                (REPO / f).write_bytes(before["bytes"][f])
    print("package-toy: ok")
    return 0


if __name__ == "__main__":
    sys.exit(main())
