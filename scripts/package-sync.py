#!/usr/bin/env python3
"""Write the launchers' generated Cargo blocks from packages/*/package.toml
(ADR 0137 section 5, #1560).

A package is a folder: creating `packages/<name>/` and running this script
adds it to the stock `flint-player` and `flint`, and deleting the folder and
running it again takes it out. Nothing else is edited by hand. The script
writes only between its markers in two files:

  crates/flint-player/Cargo.toml   crates/flint-cli/Cargo.toml

    # package-sync:features            # package-sync:dependencies
    ...                                ...
    # package-sync:end                 # package-sync:end

What it writes there:

  - `packages`: every package feature the crate has, the hand-written ones
    and the generated ones. Both crates' `default` turns it on, so a stock
    build has every package (ADR 0136 decision 3: no `[packages]` key means
    every package).
  - For each package whose feature the crate doesn't declare by hand, the
    feature and the optional dependencies that are its part of the binary:
      flint-player  `<name> = [<required features>, "dep:<crate>",
                    "<crate>?/player"]`, from `[rust] player`
      flint-cli     `<name> = [<required features>, "flint-player/<name>"
                    (when it has a player part), "dep:<crate>",
                    "<crate>?/cli"]`, from `[rust] cli`
    with `<crate> = { path = "../../packages/<name>/crates/<crate>",
    optional = true }` among the dependencies.

The six packages that moved in Phase 2a (ai, music, rhythm, procgen, living,
debug-hud) keep their hand-written features, which carry what no manifest
says yet (device features, the ai flags' crates, #1572); the script leaves
them alone and lists them in `packages`. A package it would generate that
needs more than the rule above is refused, naming why: one with more than
one `[build] features`, or a `[with.<other>]` part.

  python3 scripts/package-sync.py           write the blocks
  python3 scripts/package-sync.py --check   exit 1 when a block is stale
                                            (scripts/ci.sh runs this)

The next cargo command adds a new package's crates to Cargo.lock. Standard
library only (tomllib: Python 3.11).
"""

import sys
import tomllib
from pathlib import Path

REPO = Path(__file__).resolve().parent.parent
COMMAND = "python3 scripts/package-sync.py"
PLAYER = "crates/flint-player/Cargo.toml"
CLI = "crates/flint-cli/Cargo.toml"
END = "# package-sync:end"
# A package flint-cli has in every build, so no feature of its own: the CLI
# links flint-debug-ui for its windows' widgets and turns the player's
# `debug-hud` on in its dependency line (#1583).
CLI_HAS_IN_EVERY_BUILD = {"debug-hud"}


class Refused(Exception):
    pass


def manifests():
    """Every package manifest with `[build] features`, by folder name."""
    out = []
    for file in sorted((REPO / "packages").glob("*/package.toml")):
        with open(file, "rb") as f:
            table = tomllib.load(f)
        features = table.get("build", {}).get("features", [])
        if not features:
            continue  # the kernel: no switch, in every build
        out.append(
            {
                "name": table["package"]["name"],
                "folder": file.parent.name,
                "file": file.relative_to(REPO).as_posix(),
                "features": features,
                "requires": sorted(table.get("requires", {})),
                "rust": table.get("rust", {}),
                "with": table.get("with", {}),
            }
        )
    return out


def split(text, block):
    """The text before the block's opening marker line (inclusive), the
    block's lines, and the text from its end marker on."""
    begin = f"# package-sync:{block}"
    lines = text.splitlines(keepends=True)
    starts = [i for i, l in enumerate(lines) if l.strip() == begin]
    if len(starts) != 1:
        raise Refused(f"no single `{begin}` line (found {len(starts)})")
    s = starts[0]
    ends = [i for i in range(s + 1, len(lines)) if lines[i].strip() == END]
    if not ends:
        raise Refused(f"`{begin}` has no `{END}` after it")
    e = ends[0]
    return "".join(lines[: s + 1]), lines[s + 1 : e], "".join(lines[e:])


def hand_features(text):
    """The features a crate declares outside the generated block."""
    head, _, tail = split(text, "features")
    table = tomllib.loads(head + tail)
    return set(table.get("features", {})) - {"default"}


def toml_list(items):
    return "[" + ", ".join(f'"{i}"' for i in items) + "]"


def plan(packages):
    """The two crates' blocks: {file: {"features": [...], "dependencies": [...]}}."""
    texts = {}
    for f in (PLAYER, CLI):
        # As it is on disk: a CRLF checkout stays CRLF
        with open(REPO / f, encoding="utf-8", newline="") as file:
            texts[f] = file.read()
    hand = {f: hand_features(t) for f, t in texts.items()}

    # Which packages each crate has a part of, and which of those the
    # script generates
    def part(bin_file, p):
        if bin_file == PLAYER:
            return "player" in p["rust"]
        if p["name"] in CLI_HAS_IN_EVERY_BUILD:
            return False
        return "player" in p["rust"] or "cli" in p["rust"]

    generated = {f: [] for f in texts}
    has = {f: [] for f in texts}
    for p in packages:
        for f in texts:
            declared = [x for x in p["features"] if x in hand[f]]
            if declared and len(declared) == len(p["features"]):
                has[f].append(p)
                continue
            if declared:
                raise Refused(
                    f"{p['file']}: {f} declares some of its [build] features by hand "
                    f"({', '.join(declared)}) and not the rest"
                )
            if not part(f, p):
                continue
            if len(p["features"]) != 1 or p["features"][0] != p["name"]:
                raise Refused(
                    f"{p['file']}: a generated package has one [build] feature, its "
                    f"name (`features = [\"{p['name']}\"]`)"
                )
            if p["with"]:
                raise Refused(
                    f"{p['file']}: [with.{next(iter(p['with']))}] isn't generated yet: "
                    f"declare the package's features in {f} by hand"
                )
            generated[f].append(p)
            has[f].append(p)

    names = {f: {x for p in has[f] for x in p["features"]} for f in texts}
    out = {}
    for f in texts:
        feats, deps = [], []
        feats.append(f"packages = {toml_list(sorted(names[f]))}")
        for p in sorted(generated[f], key=lambda p: p["name"]):
            # What it requires comes with it, as `living = ["procgen", ..]`
            items = [x for q in has[f] if q["name"] in p["requires"] for x in q["features"]]
            crates = []
            if f == PLAYER:
                crates.append((p["rust"]["player"], "player"))
            else:
                if "player" in p["rust"]:
                    items.append(f"flint-player/{p['name']}")
                if "cli" in p["rust"]:
                    crates.append((p["rust"]["cli"], "cli"))
            for krate, side in crates:
                items += [f"dep:{krate}", f"{krate}?/{side}"]
                path = REPO / "packages" / p["folder"] / "crates" / krate / "Cargo.toml"
                if not path.is_file():
                    raise Refused(
                        f"{p['file']}: [rust] {side} names `{krate}`, and "
                        f"packages/{p['folder']}/crates/{krate}/Cargo.toml isn't there"
                    )
                line = (
                    f'{krate} = {{ path = "../../packages/{p["folder"]}/crates/{krate}", '
                    f"optional = true }}"
                )
                if line not in deps:
                    deps.append(line)
            feats.append(f"{p['name']} = {toml_list(items)}")
        out[f] = {"features": feats, "dependencies": deps}
    return texts, out


def rewrite(text, blocks):
    newline = "\r\n" if "\r\n" in text else "\n"
    for block, lines in blocks.items():
        head, _, tail = split(text, block)
        text = head + "".join(l + newline for l in lines) + tail
    return text


def main(argv):
    check = "--check" in argv
    unknown = [a for a in argv if a != "--check"]
    if unknown:
        print(f"package-sync: unknown argument {unknown[0]}", file=sys.stderr)
        return 2
    try:
        texts, blocks = plan(manifests())
    except Refused as e:
        print(f"package-sync: {e}", file=sys.stderr)
        return 1
    stale = []
    for f, text in texts.items():
        new = rewrite(text, blocks[f])
        if new == text:
            continue
        stale.append(f)
        if not check:
            with open(REPO / f, "w", encoding="utf-8", newline="") as out:
                out.write(new)
    if check:
        if stale:
            for f in stale:
                print(
                    f"package-sync: {f}'s generated block is stale against "
                    f"packages/*/package.toml; run `{COMMAND}`",
                    file=sys.stderr,
                )
            return 1
        print("package-sync: the generated blocks match the manifests")
        return 0
    for f in stale:
        print(f"package-sync: wrote {f}")
    if not stale:
        print("package-sync: nothing to write")
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
