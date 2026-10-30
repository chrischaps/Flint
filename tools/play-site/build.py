#!/usr/bin/env python3
"""Assemble play.chaps.dev (#103): the web player plus every game's bundle.

    python tools/play-site/build.py --player <trunk dist> --flint <flint exe> --out site

1. Copies the player (`trunk build --release` of crates/flint-player) into
   the site root.
2. For each game in games.toml, fetches its repo at the pinned commit as a
   tarball (no git needed; the repos are public) and packs it with
   `flint bundle <game> --name <id> --out-dir site/games/<id>`, so
   `?game=<id>` finds games/<id>/<id>.flintpack (flint_web.rs).
   A `video` or `poster` given as a path in the game's repo is copied
   there too (copy_media).
3. Writes games.json (what the page's chooser and fallback read) and
   built-from.txt (the Flint commit and each game's). The domain is a Pages
   setting, not a CNAME file: Actions deploys ignore one.

`--games-src DIR` packs from DIR/<id> folders already there instead of
fetching, for trying a local checkout. docs/design/web.md has the rest.
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
ID = re.compile(r"^[a-z0-9_-]+$")
# What the page shows; repo and ref go in built-from.txt.
PUBLIC = ("id", "title", "line", "controls", "page", "video", "poster", "film")


def load_games(path):
    with open(path, "rb") as f:
        games = tomllib.load(f).get("game", [])
    seen = set()
    for g in games:
        for key in ("id", "title", "repo", "ref"):
            if not g.get(key):
                sys.exit(f"{path}: a game has no {key}: {g}")
        if not ID.match(g["id"]):
            sys.exit(f"{path}: id {g['id']!r} must be lowercase letters, digits, - and _")
        if g["id"] in seen:
            sys.exit(f"{path}: two games are called {g['id']!r}")
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


def pack(flint, game, src, out):
    cmd = [flint, "bundle", str(src), "--name", game["id"], "--out-dir", str(out)]
    print("$ " + " ".join(cmd), flush=True)
    subprocess.run(cmd, check=True)
    main = out / f"{game['id']}.flintpack"
    if not main.is_file():
        sys.exit(f"flint bundle wrote no {main}")
    return sorted(p.name for p in out.iterdir())


def copy_media(game, src, out):
    """A `video` or `poster` that is a path in the game's repo, not a URL,
    is copied next to its bundle, and games.json names it there. A game
    with no film on chaps.dev can ship one in its own repo (FlintKart, #104)."""
    for key in ("video", "poster"):
        value = game.get(key)
        if not value or re.match(r"^https?://", value):
            continue
        path = (src / value).resolve()
        if not path.is_file() or src.resolve() not in path.parents:
            sys.exit(f"{game['id']}: {key} {value!r} is not a file in the game's repo")
        shutil.copy2(path, out / path.name)
        game[key] = f"games/{game['id']}/{path.name}"


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
    args = ap.parse_args()

    if not (args.player / "index.html").is_file():
        sys.exit(f"{args.player} has no index.html: run `trunk build --release` in crates/flint-player")
    games = load_games(args.games)
    # A path to the CLI, made absolute (a bare name is looked up on PATH).
    flint = str(Path(args.flint).resolve()) if Path(args.flint).is_file() else args.flint

    if args.out.exists():
        shutil.rmtree(args.out)
    shutil.copytree(args.player, args.out)

    work = args.out.parent / (args.out.name + "-src")
    source = [f"flint {flint_commit()}"]
    for game in games:
        if args.games_src:
            src = args.games_src / game["id"]
            if not src.is_dir():
                sys.exit(f"--games-src: no {src}")
        else:
            src = work / game["id"]
            fetch(game, src)
        files = pack(flint, game, src, args.out / "games" / game["id"])
        copy_media(game, src, args.out / "games" / game["id"])
        print(f"{game['id']}: {', '.join(files)}", flush=True)
        source.append(f"{game['id']} {game['repo']}@{game['ref']}")
    if not args.games_src and work.exists():
        shutil.rmtree(work)

    listing = {"games": [{k: g[k] for k in PUBLIC if g.get(k)} for g in games]}
    (args.out / "games.json").write_text(json.dumps(listing, indent=2) + "\n", encoding="utf-8")
    (args.out / "built-from.txt").write_text("\n".join(source) + "\n", encoding="utf-8")
    print(f"site: {args.out} ({len(games)} game{'s' if len(games) != 1 else ''})")


if __name__ == "__main__":
    main()
