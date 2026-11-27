#!/usr/bin/env python3
"""Write the launchers' generated Cargo blocks from packages/*/package.toml
(ADR 0137 section 5, #1560, #1568).

A package is a folder: creating `packages/<name>/` and running this script
adds it to the stock `flint-player` and `flint`, and deleting the folder and
running it again takes it out. Nothing else is edited by hand. The script
writes only between its markers in two files:

  crates/flint-player/Cargo.toml   crates/flint-cli/Cargo.toml

    # package-sync:features            # package-sync:dependencies
    ...                                ...
    # package-sync:end                 # package-sync:end

What it writes there, all from the manifests:

  - `packages`: every package feature the crate has. Both crates' `default`
    turns it on, so a stock build has every package (ADR 0136 decision 3:
    no `[packages]` key means every package).
  - `packages-<device>` for each device feature written by hand outside the
    block (`audio`, `gamepad`, `dialogs`), which turns it on: the features
    of the packages' crates that each manifest's `[rust.devices]` names for
    it, weakly (`flint-music?/device`), so a device reaches a package's crate
    only where the package is in the build.
  - For each package, its feature and the optional dependencies that are
    its part of the binary:
      flint-player  `<name> = [<required features>, "dep:<crate>",
                    "<crate>?/player"]`, from `[rust] player`
      flint-cli     `<name> = [<required features>, "flint-player/<name>"
                    (when it has a player part), "dep:<crate>",
                    "<crate>?/cli"]`, from `[rust] cli`
    then, for each other package's `[with.<name>] features`, that crate's
    feature, weakly (`flint-living?/debug-hud` on `debug-hud`: on when the
    game lists both). Each dependency is `<crate> = { workspace = true,
    optional = true }` when the root's `[workspace.dependencies]` has the
    crate (its line says the crate's default features), else `{ path =
    "../../packages/<name>/crates/<crate>", optional = true }`.

A line flint-player's part already turns on is not written again in
flint-cli, whose package and device features forward to flint-player's.
A package with more than one `[build] features` is refused, as is a feature
of a package written by hand outside the block, a `[rust.devices]` name
neither crate has a feature for, a hand-written device feature that doesn't
turn on its `packages-<device>`, and a `[with]` or device line whose crate
no launcher links; each refusal names the edit.

  python3 scripts/package-sync.py           write the blocks
  python3 scripts/package-sync.py --check   exit 1 when a block is stale
                                            (scripts/ci.sh runs this)
  python3 scripts/package-sync.py --self-test
                                            the generator over a scratch copy
                                            of the tree, with a made-up package
                                            (scripts/ci.sh runs this too)

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
LAUNCHERS = (PLAYER, CLI)
SIDE = {PLAYER: "player", CLI: "cli"}
END = "# package-sync:end"
DEVICE_PREFIX = "packages-"
WIDTH = 92  # a feature line wraps past it


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
        rust = table.get("rust", {})
        out.append(
            {
                "name": table["package"]["name"],
                "folder": file.parent.name,
                "file": file.relative_to(REPO).as_posix(),
                "features": features,
                "requires": sorted(table.get("requires", {})),
                "rust": {k: v for k, v in rust.items() if k in SIDE.values()},
                "devices": rust.get("devices", {}),
                "with": {k: v.get("features", []) for k, v in table.get("with", {}).items()},
            }
        )
    return out


def workspace_crates():
    """The crates the root's `[workspace.dependencies]` declares."""
    with open(REPO / "Cargo.toml", "rb") as f:
        root = tomllib.load(f)
    return set(root.get("workspace", {}).get("dependencies", {}))


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
    """The features a crate declares outside the generated block, each with
    what it turns on."""
    head, _, tail = split(text, "features")
    table = tomllib.loads(head + tail)
    return {k: v for k, v in table.get("features", {}).items() if k != "default"}


def toml_list(items, key=""):
    """A TOML list of strings; past `WIDTH` with its key, one indented line
    of items at a time."""
    quoted = [f'"{i}"' for i in items]
    one = "[" + ", ".join(quoted) + "]"
    if len(key) + 3 + len(one) <= WIDTH:
        return one
    lines, line = [], ""
    for q in quoted:
        if line and len(line) + len(q) + 2 > WIDTH - 4:
            lines.append(line)
            line = ""
        line += ("" if not line else " ") + q + ","
    lines.append(line)
    return "[\n" + "".join(f"    {l}\n" for l in lines) + "]"


def crate_feature(entry, where):
    krate, _, feature = entry.partition("/")
    if not krate or not feature:
        raise Refused(f"{where}: `{entry}` is no crate's feature: write `crate/feature`")
    return krate, feature


def plan(packages):
    """The two crates' blocks: {file: {"features": [...], "dependencies": [...]}}."""
    texts = {}
    for f in LAUNCHERS:
        # As it is on disk: a CRLF checkout stays CRLF
        with open(REPO / f, encoding="utf-8", newline="") as file:
            texts[f] = file.read()
    hand = {f: hand_features(t) for f, t in texts.items()}
    in_workspace = workspace_crates()

    # Each launcher's packages, and the crates it links for each (generated
    # dependencies). A package's feature written by hand is refused: every
    # package's is the script's since #1568.
    has = {f: [] for f in LAUNCHERS}
    links = {f: {} for f in LAUNCHERS}  # launcher -> package name -> [crate]
    for p in packages:
        for f in LAUNCHERS:
            declared = [x for x in p["features"] if x in hand[f]]
            if declared:
                raise Refused(
                    f"{p['file']}: {f} declares its [build] features by hand "
                    f"({', '.join(declared)}): delete those lines and run `{COMMAND}`"
                )
            part = "player" in p["rust"] or (f == CLI and "cli" in p["rust"])
            if not part:
                continue
            if len(p["features"]) != 1 or p["features"][0] != p["name"]:
                raise Refused(
                    f"{p['file']}: a package has one [build] feature, its "
                    f"name (`features = [\"{p['name']}\"]`)"
                )
            has[f].append(p)
            krate = p["rust"].get(SIDE[f])
            links[f][p["name"]] = [krate] if krate else []
            if krate:
                path = REPO / "packages" / p["folder"] / "crates" / krate / "Cargo.toml"
                if not path.is_file():
                    raise Refused(
                        f"{p['file']}: [rust] {SIDE[f]} names `{krate}`, and "
                        f"packages/{p['folder']}/crates/{krate}/Cargo.toml isn't there"
                    )
    by_name = {p["name"]: p for p in packages}

    def player_has(package, krate):
        return krate in links[PLAYER].get(package, [])

    # The weak lines each feature of a launcher gets: device lines into
    # `packages-<device>`, `[with]` lines into the other package's feature
    extra = {f: {} for f in LAUNCHERS}  # launcher -> feature -> [line]

    def put(f, feature, line):
        lines = extra[f].setdefault(feature, [])
        if line not in lines:
            lines.append(line)

    devices = {d for p in packages for d in p["devices"]}
    for f in LAUNCHERS:
        devices |= {
            g[len(DEVICE_PREFIX):]
            for on in hand[f].values()
            for g in on
            if g.startswith(DEVICE_PREFIX)
        }
    for d in sorted(devices):
        owners = [f for f in LAUNCHERS if d in hand[f]]
        if not owners:
            named = [p["file"] for p in packages if d in p["devices"]]
            raise Refused(
                f"{', '.join(named)}: [rust.devices] {d}: neither {PLAYER} nor {CLI} "
                f"has a `{d}` feature"
            )
        for f in owners:
            helper = DEVICE_PREFIX + d
            if helper not in hand[f][d]:
                raise Refused(
                    f"{f}: its `{d}` feature doesn't turn on `{helper}`, the packages' "
                    f"part of it: add \"{helper}\" to it"
                )
            extra[f].setdefault(helper, [])
    for p in packages:
        for d, entries in sorted(p["devices"].items()):
            for entry in entries:
                krate, feature = crate_feature(entry, f"{p['file']}: [rust.devices] {d}")
                written = False
                for f in LAUNCHERS:
                    if d not in hand[f] or krate not in links[f].get(p["name"], []):
                        continue
                    written = True
                    if f == CLI and d in hand[PLAYER] and player_has(p["name"], krate):
                        continue  # flint-cli's `<d>` turns flint-player's on
                    put(f, DEVICE_PREFIX + d, f"{krate}?/{feature}")
                if not written:
                    raise Refused(
                        f"{p['file']}: [rust.devices] {d} turns on `{entry}`, and no "
                        f"launcher with a `{d}` feature links `{krate}`: name a [rust] crate"
                    )
        for other, entries in sorted(p["with"].items()):
            if other not in by_name:
                raise Refused(f"{p['file']}: [with.{other}] names no package")
            for entry in entries:
                krate, feature = crate_feature(entry, f"{p['file']}: [with.{other}]")
                written = False
                for f in LAUNCHERS:
                    if other not in links[f] or krate not in links[f].get(p["name"], []):
                        continue
                    written = True
                    if f == CLI and other in links[PLAYER] and player_has(p["name"], krate):
                        continue  # flint-cli's `<other>` turns flint-player's on
                    put(f, other, f"{krate}?/{feature}")
                if not written:
                    raise Refused(
                        f"{p['file']}: [with.{other}] turns on `{entry}`, and no launcher "
                        f"links `{krate}` for `{p['name']}` beside `{other}`: name a "
                        f"[rust] crate"
                    )

    out = {}
    for f in LAUNCHERS:
        feats, deps = [], []
        feats.append(f"packages = {toml_list(sorted(p['name'] for p in has[f]), 'packages')}")
        for helper in sorted(k for k in extra[f] if k.startswith(DEVICE_PREFIX)):
            feats.append(f"{helper} = {toml_list(sorted(extra[f][helper]), helper)}")
        names = {p["name"] for p in has[f]}
        for p in sorted(has[f], key=lambda p: p["name"]):
            # What it requires comes with it, as `living = ["procgen", ..]`
            items = [q for q in p["requires"] if q in names]
            if f == CLI and "player" in p["rust"]:
                items.append(f"flint-player/{p['name']}")
            for krate in links[f][p["name"]]:
                items += [f"dep:{krate}", f"{krate}?/{SIDE[f]}"]
                if krate in in_workspace:
                    line = f"{krate} = {{ workspace = true, optional = true }}"
                else:
                    line = (
                        f'{krate} = {{ path = "../../packages/{p["folder"]}/crates/{krate}", '
                        f"optional = true }}"
                    )
                if line not in deps:
                    deps.append(line)
            items += sorted(extra[f].get(p["name"], []))
            feats.append(f"{p['name']} = {toml_list(items, p['name'])}")
        out[f] = {"features": feats, "dependencies": sorted(deps)}
    return texts, out


def rewrite(text, blocks):
    newline = "\r\n" if "\r\n" in text else "\n"
    for block, lines in blocks.items():
        head, _, tail = split(text, block)
        # A wrapped list is one entry of several lines
        text = head + "".join(x + newline for l in lines for x in l.split("\n")) + tail
    return text


TOY = """[package]
name = "toy"
version = "0.1.0"
tier = "feature"
summary = "A made-up package"
engine = "^0.1"

[requires]
music = "*"

[build]
features = ["toy"]

[rust]
player = "flint-toy"
cli = "flint-toy-cli"

[rust.devices]
audio = ["flint-toy/device"]
dialogs = ["flint-toy-cli/dialogs"]

[with.debug-hud]
features = ["flint-toy/debug-hud"]

[provides]
crates = ["flint-toy", "flint-toy-cli"]
"""


def self_test():
    """The generator over a scratch copy of the tree: the repo's blocks as
    they are, a made-up package with devices and a `[with]` part, and each
    refusal. Needs no cargo (scripts/ci.sh runs it)."""
    import shutil
    import tempfile

    global REPO
    real = REPO
    failures = []

    def expect(what, ok):
        print(f"  {'ok  ' if ok else 'FAIL'} {what}")
        if not ok:
            failures.append(what)

    def scratch(root):
        for f in ("Cargo.toml", PLAYER, CLI):
            (root / f).parent.mkdir(parents=True, exist_ok=True)
            shutil.copyfile(real / f, root / f)
        for m in (real / "packages").glob("*/package.toml"):
            (root / m.relative_to(real)).parent.mkdir(parents=True, exist_ok=True)
            shutil.copyfile(m, root / m.relative_to(real))
        for c in (real / "packages").glob("*/crates/*/Cargo.toml"):
            (root / c.relative_to(real)).parent.mkdir(parents=True, exist_ok=True)
            (root / c.relative_to(real)).write_text("", encoding="utf-8")

    def add_toy(root, text=TOY, crates=("flint-toy", "flint-toy-cli")):
        (root / "packages/toy").mkdir(parents=True, exist_ok=True)
        (root / "packages/toy/package.toml").write_text(text, encoding="utf-8")
        for c in crates:
            (root / f"packages/toy/crates/{c}").mkdir(parents=True, exist_ok=True)
            (root / f"packages/toy/crates/{c}/Cargo.toml").write_text("", encoding="utf-8")

    def lines(blocks, f):
        return {l.split(" = ", 1)[0]: l.split(" = ", 1)[1] for l in blocks[f]["features"]}

    def refused(root, edit, *words):
        edit(root)
        try:
            plan(manifests())
        except Refused as e:
            return all(w in str(e) for w in words), str(e)
        return False, "not refused"

    with tempfile.TemporaryDirectory() as tmp:
        try:
            REPO = Path(tmp) / "a"
            scratch(REPO)
            texts, blocks = plan(manifests())
            expect(
                "the tree's blocks are what it holds",
                all(rewrite(texts[f], blocks[f]) == texts[f] for f in LAUNCHERS),
            )

            add_toy(REPO)
            _, blocks = plan(manifests())
            player, cli = lines(blocks, PLAYER), lines(blocks, CLI)
            expect(
                "player: the toy's feature brings what it requires and its player part",
                player.get("toy") == '["music", "dep:flint-toy", "flint-toy?/player"]',
            )
            expect(
                "player: its device line is in packages-audio, weakly",
                '"flint-toy?/device"' in player.get("packages-audio", ""),
            )
            expect(
                "player: its [with.debug-hud] line is on debug-hud's feature",
                '"flint-toy?/debug-hud"' in player.get("debug-hud", ""),
            )
            expect(
                "cli: the toy's feature forwards to the player's and turns on its cli part",
                cli.get("toy")
                == '["music", "flint-player/toy", "dep:flint-toy-cli", "flint-toy-cli?/cli"]',
            )
            expect(
                "cli: a line flint-player's part has is not written again",
                "flint-toy?/" not in cli.get("packages-audio", "") + cli.get("debug-hud", ""),
            )
            expect(
                "cli: a device of a crate only it links is written",
                '"flint-toy-cli?/dialogs"' in cli.get("packages-dialogs", ""),
            )
            dep = 'flint-toy = { path = "../../packages/toy/crates/flint-toy", optional = true }'
            expect("a crate the workspace doesn't declare is a path", dep in blocks[PLAYER]["dependencies"])
            expect(
                "a crate the workspace declares is `workspace = true`",
                "flint-music = { workspace = true, optional = true }" in blocks[PLAYER]["dependencies"],
            )

            cases = [
                (
                    "a device no launcher has",
                    lambda r: add_toy(r, TOY.replace("dialogs = [", "rumble = [")),
                    ("[rust.devices] rumble", "has a `rumble` feature"),
                ),
                (
                    "a [with] crate no launcher links",
                    lambda r: add_toy(
                        r,
                        TOY.replace('features = ["flint-toy/debug-hud"]', 'features = ["flint-toyz/debug-hud"]'),
                    ),
                    ("[with.debug-hud]", "flint-toyz"),
                ),
                (
                    "a package's feature written by hand",
                    lambda r: (r / PLAYER).write_text(
                        (r / PLAYER).read_text(encoding="utf-8").replace(
                            "frame-history = []", 'frame-history = []\nmusic = ["dep:flint-music"]'
                        ),
                        encoding="utf-8",
                    ),
                    ("declares its [build] features by hand", "music"),
                ),
                (
                    "a device feature that doesn't turn on its packages' part",
                    lambda r: (r / PLAYER).write_text(
                        (r / PLAYER).read_text(encoding="utf-8").replace(
                            '"flint-audio/device", "packages-audio"', '"flint-audio/device"'
                        ),
                        encoding="utf-8",
                    ),
                    ("`audio` feature doesn't turn on `packages-audio`",),
                ),
            ]
            for i, (what, edit, words) in enumerate(cases):
                REPO = Path(tmp) / f"r{i}"
                scratch(REPO)
                ok, said = refused(REPO, edit, *words)
                expect(f"refused: {what}" + ("" if ok else f" ({said})"), ok)
        finally:
            REPO = real
    if failures:
        print(f"package-sync: self-test: {len(failures)} failed", file=sys.stderr)
        return 1
    print("package-sync: self-test passed")
    return 0


def main(argv):
    if argv == ["--self-test"]:
        return self_test()
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
