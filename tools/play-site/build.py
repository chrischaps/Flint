#!/usr/bin/env python3
"""Assemble play.chaps.dev (#103): the web player plus every game's bundle.

    python tools/play-site/build.py --player <trunk dist> --flint <flint exe> --out site

For each game in games.toml, fetches its repo at the pinned commit as a
tarball (no git needed; the repos are public) and exports it into the site
with `flint export web <game> --out site --player <dist> --name <id>
--meta key=value...` (#106), the command any Flint game ships to the web
with. Export copies the player into the site root, packs the bundle into
games/<id>/ (so `?game=<id>` finds games/<id>/<id>.flintpack, flint_web.rs),
copies a `video` or `poster` given as a path in the game's repo beside it,
and adds the game to games.json (what the page's chooser and fallback read),
in games.toml's order. This script then writes built-from.txt (the Flint
commit and each game's). The domain is a Pages setting, not a CNAME file:
Actions deploys ignore one.

A game may live in this repo instead (#430): `path` names its folder in
the Flint checkout the script runs from, with no `repo` or `ref`, and it is
exported from there, at the engine's own commit. Sun Prints, the site's
user-style game, is one.

`--games-src DIR` packs from DIR/<id> folders already there instead of
fetching, for trying a local checkout, and `--only id,id` builds just those
games. docs/design/web.md has the rest.
"""

import argparse
import io
import json
import os
import re
import shutil
import subprocess
import sys
import tarfile
import tomllib
import urllib.request
from pathlib import Path

HERE = Path(__file__).resolve().parent
# The Flint checkout this script is in: what a game's `path` is relative to.
FLINT_ROOT = HERE.parent.parent
ID = re.compile(r"^[a-z0-9_-]+$")
# What the page shows (games.json, through export's --meta); repo and ref go in built-from.txt.
PUBLIC = ("id", "title", "line", "controls", "touch", "orientation", "page", "video", "poster", "film")
# What `orientation` may say; unset, a phone turns the game freely.
ORIENTATIONS = ("landscape",)


def local_folder(game):
    """A `path` game's folder in this checkout, or None if it isn't one."""
    root = FLINT_ROOT.resolve()
    folder = (root / game["path"]).resolve()
    return folder if folder.is_relative_to(root) and folder.is_dir() else None


def load_games(path):
    with open(path, "rb") as f:
        games = tomllib.load(f).get("game", [])
    seen = set()
    for g in games:
        for key in ("id", "title"):
            if not g.get(key):
                sys.exit(f"{path}: a game has no {key}: {g}")
        if not ID.match(g["id"]):
            sys.exit(f"{path}: id {g['id']!r} must be lowercase letters, digits, - and _")
        if g["id"] in seen:
            sys.exit(f"{path}: two games are called {g['id']!r}")
        if g.get("orientation") is not None and g["orientation"] not in ORIENTATIONS:
            sys.exit(f"{path}: {g['id']}: orientation must be one of {ORIENTATIONS}, not {g['orientation']!r}")
        if g.get("path") is not None:
            # A folder in this checkout, built at the engine's own commit: no pin
            if g.get("repo") or g.get("ref"):
                sys.exit(f"{path}: {g['id']}: give `path` (a folder in this repo) or `repo` and `ref`, not both")
            if local_folder(g) is None:
                sys.exit(f"{path}: {g['id']}: path {g['path']!r} is not a folder in this repo")
        else:
            for key in ("repo", "ref"):
                if not g.get(key):
                    sys.exit(f"{path}: {g['id']}: no {key} (or `path`, for a folder in this repo)")
            if not re.fullmatch(r"[0-9a-f]{40}", g["ref"]):
                sys.exit(f"{path}: {g['id']}: ref must be a full commit SHA, not {g['ref']!r}")
        seen.add(g["id"])
    return games


def fetch(game, into):
    """The repo at its commit, from GitHub's tarball, unpacked into `into`."""
    url = f"https://codeload.github.com/{game['repo']}/tar.gz/{game['ref']}"
    print(f"fetching {game['repo']}@{game['ref'][:7]}", flush=True)
    with urllib.request.urlopen(url) as r:
        data = r.read()
    if into.exists():
        shutil.rmtree(into)
    into.mkdir(parents=True)
    with tarfile.open(fileobj=io.BytesIO(data), mode="r:gz") as tar:
        for member in tar.getmembers():
            # Every entry sits under one <repo>-<sha>/ folder: drop it.
            parts = member.name.split("/", 1)
            if len(parts) < 2 or not parts[1]:
                continue
            member.name = parts[1]
            tar.extract(member, into, filter="data")


def export(flint, game, src, player, out):
    """`flint export web` (#106). games.toml's words for the game go in as
    --meta, over the game's own bundle.toml [web]. A `video` or `poster`
    that is a path in the game's repo, not a URL, export copies next to the
    bundle (a game with no film on chaps.dev can ship one in its own repo,
    FlintKart #104)."""
    cmd = [flint, "export", "web", str(src), "--out", str(out), "--player", str(player), "--name", game["id"]]
    for key in PUBLIC:
        if key != "id" and game.get(key):
            cmd += ["--meta", f"{key}={game[key]}"]
    print("$ " + " ".join(cmd), flush=True)
    subprocess.run(cmd, check=True)
    games = out / "games" / game["id"]
    if not (games / f"{game['id']}.flintpack").is_file():
        sys.exit(f"flint export web wrote no {games}/{game['id']}.flintpack")
    return sorted(p.name for p in games.iterdir())


def flint_commit():
    sha = os.environ.get("GITHUB_SHA")
    if sha:
        return sha
    try:
        return subprocess.run(
            ["git", "rev-parse", "HEAD"], cwd=HERE, capture_output=True, text=True, check=True
        ).stdout.strip()
    except (OSError, subprocess.CalledProcessError):
        return "unknown"


def main():
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--player", required=True, type=Path, help="trunk's dist/ for crates/flint-player")
    ap.add_argument("--flint", required=True, help="the flint CLI, for `flint bundle`")
    ap.add_argument("--out", required=True, type=Path, help="the site folder to write (emptied first)")
    ap.add_argument("--games", type=Path, default=HERE / "games.toml")
    ap.add_argument("--games-src", type=Path, help="pack DIR/<id> instead of fetching each repo")
    ap.add_argument("--only", help="build these games alone (ids, comma-separated), for a local trial")
    args = ap.parse_args()
    # The --meta lines carry games.toml's words (◀ ▶, ·), which a Windows
    # console's code page can't print.
    sys.stdout.reconfigure(errors="backslashreplace")

    if not (args.player / "index.html").is_file():
        sys.exit(f"{args.player} has no index.html: run `trunk build --release` in crates/flint-player")
    games = load_games(args.games)
    if args.only:
        wanted = args.only.split(",")
        unknown = [i for i in wanted if i not in {g["id"] for g in games}]
        if unknown:
            sys.exit(f"--only: {args.games} has no {', '.join(unknown)}")
        games = [g for g in games if g["id"] in wanted]
    # A path to the CLI, made absolute (a bare name is looked up on PATH).
    flint = str(Path(args.flint).resolve()) if Path(args.flint).is_file() else args.flint

    if args.out.exists():
        shutil.rmtree(args.out)

    work = args.out.parent / (args.out.name + "-src")
    source = [f"flint {flint_commit()}"]
    for game in games:
        if game.get("path") is not None:
            # In this checkout already: packed where it is, at Flint's commit
            src = local_folder(game)
            source.append(f"{game['id']} flint:{game['path']}")
        elif args.games_src:
            src = args.games_src / game["id"]
            if not src.is_dir():
                sys.exit(f"--games-src: no {src}")
            source.append(f"{game['id']} {game['repo']}@{game['ref']}")
        else:
            src = work / game["id"]
            fetch(game, src)
            source.append(f"{game['id']} {game['repo']}@{game['ref']}")
        files = export(flint, game, src, args.player, args.out)
        print(f"{game['id']}: {', '.join(files)}", flush=True)
    if not args.games_src and work.exists():
        shutil.rmtree(work)

    listed = [g["id"] for g in json.loads((args.out / "games.json").read_text(encoding="utf-8"))["games"]]
    if listed != [g["id"] for g in games]:
        sys.exit(f"games.json lists {listed}, not games.toml's {[g['id'] for g in games]}")
    (args.out / "built-from.txt").write_text("\n".join(source) + "\n", encoding="utf-8")
    print(f"site: {args.out} ({len(games)} game{'s' if len(games) != 1 else ''})")


if __name__ == "__main__":
    main()
