#!/usr/bin/env python3
"""One dependency rule over the whole crate graph (epic #1611's gate, #1624).

Phase 2's gate (scripts/package-names-check.py) reads the kernel's source
for package names. This reads the graph the source builds into: `cargo
metadata --no-deps` gives every workspace member's declared dependencies,
and every `packages/*/package.toml` says which package owns each crate
(`[provides] crates`; the `engine` manifest's crates are the kernel's). A
member no manifest names (`tools/arch-analyzer`, a generated
`launch/<id>/`) is held to nothing. ADR 0138 decision 6 is the rule:

  Held: every dependency of kind normal or build, optional or not, for
  every target. Dev-dependencies are not: a test links what it tests.

  1. A kernel crate depends on no package's crate. The one allowance is a
     line inside the `# package-sync:dependencies` block of a stock
     launcher's Cargo.toml (flint-player, flint-cli: the list package-sync
     writes from the manifests, ADR 0137 section 5), and it is `optional`.
  2. A crate of package P depends only on the kernel, on P's own crates,
     on crates of a package P's `[requires]` names (as written, not closed
     over: rhythm uses audio's crate, so rhythm lists audio though music
     already requires it), and, as an `optional` dependency that only
     features P's `[with.Q] features` names turn on, on crates of Q. A
     crate's `[with.Q]` part is Q's edge wherever it is turned on: a crate
     that asks another for such a feature (in its dependency's `features`,
     a `dep/feature` of its own, or the dependency's `default`) brings Q in,
     so its package requires Q or asks from its own `[with.Q]` feature. A
     crate in `packages/<p>/` that p's manifest doesn't list is p's, and
     named.
  3. EXCEPTIONS below, each naming the issue that ends it. One fails when
     its edge is gone, and when its issue is closed (asked of GitHub with
     `gh`, else the REST API with $GITHUB_TOKEN; with neither it says it
     couldn't ask and holds the edge alone). So the list only shrinks.

A failure names the crate, the dependency, both packages and the fix.

  python3 scripts/package-deps-check.py           (scripts/ci.sh runs it)
  python3 scripts/package-deps-check.py --graph   print every held edge
                                                  between owners, and pass
  python3 scripts/package-deps-check.py --metadata FILE
                                                  read `cargo metadata
                                                  --format-version 1
                                                  --no-deps` from FILE

Standard library only (tomllib: Python 3.11).
"""

import json
import os
import re
import subprocess
import sys
import tomllib
import urllib.request
from pathlib import Path

REPO = Path(__file__).resolve().parent.parent
LAUNCHERS = ("flint-player", "flint-cli")
REPO_SLUG = "chrischaps/Flint"

# (crate, dependency): (issue that ends it, why it is still there).
EXCEPTIONS = {
    ("flint-procgen", "flint-wooze"): (
        1659,
        "procgen's `living` feature (on by default, and asked for by flint-living) "
        "checks a scene's `[wooze]` and `[living.species]` against the wooze field: "
        "living's part, which #1659 moves into living (ADR 0138 decision 6's one exception)",
    ),
}


# --- the owners ------------------------------------------------------------


def manifests():
    """Every package manifest, by package name: its tier, crates, requires
    and `[with.Q] features`."""
    out = {}
    for file in sorted((REPO / "packages").glob("*/package.toml")):
        with open(file, "rb") as f:
            table = tomllib.load(f)
        name = table["package"]["name"]
        out[name] = {
            "file": file.relative_to(REPO).as_posix(),
            "tier": table["package"].get("tier", ""),
            "crates": table.get("provides", {}).get("crates", []),
            "requires": sorted(table.get("requires", {})),
            "with": {q: part.get("features", []) for q, part in table.get("with", {}).items()},
        }
    return out


def owners(packages):
    """crate -> package; the kernel's crates are `engine`'s. A crate two
    manifests claim fails."""
    owner, failures = {}, []
    for name, m in packages.items():
        for krate in m["crates"]:
            if krate in owner:
                failures.append(
                    f"{krate} is in two manifests' [provides] crates: "
                    f"{packages[owner[krate]]['file']} and {m['file']}"
                )
            owner[krate] = name
    return owner, failures


def kernel_name(packages):
    return next(n for n, m in packages.items() if m["tier"] == "kernel")


# --- the graph -------------------------------------------------------------


def metadata(path=None):
    if path:
        return json.loads(Path(path).read_text(encoding="utf-8"))
    out = subprocess.run(
        ["cargo", "metadata", "--format-version", "1", "--no-deps"],
        cwd=REPO,
        capture_output=True,
        text=True,
        encoding="utf-8",
    )
    if out.returncode != 0:
        sys.stderr.write(out.stderr)
        raise SystemExit("package-deps-check: `cargo metadata --no-deps` failed")
    return json.loads(out.stdout)


def held(dep):
    """A normal or build dependency (cargo metadata's kind null or "build")."""
    return dep.get("kind") in (None, "normal", "build")


def sync_block(manifest_path):
    """The dependency names inside a launcher's generated block."""
    text = Path(manifest_path).read_text(encoding="utf-8")
    m = re.search(r"# package-sync:dependencies\r?\n(.*?)# package-sync:end", text, re.S)
    if not m:
        return set()
    return {
        line.split("=", 1)[0].strip()
        for line in m.group(1).splitlines()
        if re.match(r"\s*[A-Za-z0-9_-]+\s*=", line)
    }


def turned_on_by(package, dep_key):
    """Every feature of `package` (cargo metadata's) that turns the optional
    dependency `dep_key` on: one that names it (`dep:x`, `x/f`; `x?/f` only
    reaches into it, so not that), the implicit feature `x` a dependency no
    `dep:` names gets, and every feature that reaches one of those through
    the crate's own feature lists, `default` among them."""
    features = package.get("features", {})
    direct = set()
    explicit = any(v == f"dep:{dep_key}" for vs in features.values() for v in vs)
    for feature, values in features.items():
        for v in values:
            if v == f"dep:{dep_key}" or v.startswith(f"{dep_key}/"):
                direct.add(feature)
            elif v == dep_key and not explicit:
                direct.add(feature)
    if not explicit and dep_key not in features:
        direct.add(dep_key)  # the implicit feature
    reach = set(direct)
    changed = True
    while changed:
        changed = False
        for feature, values in features.items():
            if feature not in reach and any(v in reach for v in values):
                reach.add(feature)
                changed = True
    return reach


# --- the check -------------------------------------------------------------


def issue_open(number):
    """True / False, or None when GitHub couldn't be asked."""
    try:
        out = subprocess.run(
            ["gh", "issue", "view", str(number), "--repo", REPO_SLUG, "--json", "state", "-q", ".state"],
            capture_output=True,
            text=True,
            timeout=30,
        )
        if out.returncode == 0 and out.stdout.strip():
            return out.stdout.strip() == "OPEN"
    except (OSError, subprocess.TimeoutExpired):
        pass
    token = os.environ.get("GITHUB_TOKEN") or os.environ.get("GH_TOKEN")
    if not token:
        return None
    req = urllib.request.Request(
        f"https://api.github.com/repos/{REPO_SLUG}/issues/{number}",
        headers={"Authorization": f"Bearer {token}", "Accept": "application/vnd.github+json"},
    )
    try:
        with urllib.request.urlopen(req, timeout=30) as r:
            return json.load(r).get("state") == "open"
    except (OSError, ValueError):
        return None


def check(meta, packages, issue_state=issue_open):
    """(failures, notes, edges): edges is [(crate, owner, dep, dep owner, how)].
    `issue_state(n)` says whether an exception's issue is open (None: unknown)."""
    owner, failures = owners(packages)
    kernel = kernel_name(packages)
    members = set(meta["workspace_members"])
    by_name = {p["name"]: p for p in meta["packages"] if p["id"] in members}
    notes, edges, seen_exceptions = [], [], set()

    for name in owner:
        if name not in by_name:
            failures.append(
                f"{packages[owner[name]]['file']} lists the crate {name}, which is no workspace member"
            )
    # A crate in a package's folder that its manifest doesn't list is still
    # that package's: held, and named
    for name, pkg in sorted(by_name.items()):
        try:
            parts = Path(pkg["manifest_path"]).resolve().relative_to(REPO).parts
        except ValueError:
            continue
        if name not in owner and len(parts) > 2 and parts[0] == "packages" and parts[1] in packages:
            owner[name] = parts[1]
            failures.append(
                f"{name} is in packages/{parts[1]}/ but {packages[parts[1]]['file']}'s "
                f"[provides] crates doesn't list it: add it there"
            )

    # Each crate's [with] parts: feature -> the packages it brings in, the
    # features that turn on an optional dependency on another package's crate
    parts_of = {}
    for name, pkg in by_name.items():
        found = {}
        for dep in pkg["dependencies"]:
            other = owner.get(dep["name"])
            if not held(dep) or not dep.get("optional") or other in (None, kernel, owner.get(name)):
                continue
            for feature in turned_on_by(pkg, dep.get("rename") or dep["name"]):
                found.setdefault(feature, set()).add(other)
        parts_of[name] = found

    for krate, pkg in sorted(by_name.items()):
        mine = owner.get(krate)
        if mine is None:
            continue  # a tool or a generated launcher: held to nothing
        block = sync_block(pkg["manifest_path"]) if krate in LAUNCHERS else set()
        for dep in pkg["dependencies"]:
            if not held(dep):
                continue
            # A [with] part of the crate depended on, turned on from here,
            # brings its package in too. The launchers' feature lines are
            # package-sync's generated list (rule 1's allowance)
            parts = parts_of.get(dep["name"], {})
            if parts and mine != kernel:
                key = dep.get("rename") or dep["name"]
                asked = [(f, None) for f in dep.get("features", [])]
                if dep.get("uses_default_features", True):
                    asked.append(("default", None))
                for feature, values in pkg.get("features", {}).items():
                    for v in values:
                        m = re.match(rf"^{re.escape(key)}\??/(.+)$", v)
                        if m:
                            asked.append((m.group(1), feature))
                for f, by in asked:
                    for brought in sorted(parts.get(f, ())):
                        if brought == mine or brought in packages[mine]["requires"]:
                            continue
                        named = {g.split("/", 1)[1] for g in packages[mine]["with"].get(brought, [])
                                 if g.startswith(f"{krate}/")}
                        if by is not None and by in named:
                            continue
                        failures.append(
                            f"{krate} (the `{mine}` package) turns on {dep['name']}'s `{f}`"
                            + (f" (from its feature `{by}`)" if by else "")
                            + f", which brings in the `{brought}` package, but "
                            f"{packages[mine]['file']}'s [requires] doesn't list it: add "
                            f"`{brought} = \"*\"`, or ask for `{f}` only from a feature its "
                            f"`[with.{brought}]` lists"
                        )
            theirs = owner.get(dep["name"])
            if theirs is None or theirs == mine:
                continue
            key = dep.get("rename") or dep["name"]
            what = "build-dependency" if dep.get("kind") == "build" else "dependency"
            where = f" (target {dep['target']})" if dep.get("target") else ""
            ok, how = False, ""
            if theirs == kernel:
                ok, how = True, "the kernel"
            elif mine == kernel:
                # Rule 1
                # The block's lines are plain `[dependencies]`: the same name
                # under a target table is a line of its own, outside it
                if krate in LAUNCHERS and key in block and dep.get("optional") and not dep.get("target"):
                    ok, how = True, "the launcher's generated list"
                elif krate in LAUNCHERS and key in block and not dep.get("target"):
                    failures.append(
                        f"{krate} (the kernel) depends on {dep['name']} (the `{theirs}` package) "
                        f"inside its package-sync block, but not as `optional`: "
                        f"run `python3 scripts/package-sync.py`"
                    )
                    continue
            else:
                # Rule 2
                m = packages[mine]
                if theirs in m["requires"]:
                    ok, how = True, f"{mine} requires {theirs}"
                elif theirs in m["with"] and dep.get("optional"):
                    named = {f.split("/", 1)[1] for f in m["with"][theirs] if f.startswith(f"{krate}/")}
                    # Every feature that turns it on, `default` too, is one
                    # the [with] part names
                    stray = sorted(turned_on_by(pkg, key) - named)
                    if named and not stray:
                        ok, how = True, f"{mine}'s [with.{theirs}]"
                    elif (krate, dep["name"]) not in EXCEPTIONS:
                        failures.append(
                            f"{krate} (the `{mine}` package) has {dep['name']} (the `{theirs}` "
                            f"package) as an optional {what}{where} for its [with.{theirs}] part, "
                            f"but features [with.{theirs}] doesn't name turn it on: "
                            + ", ".join(f"`{f}`" for f in stray)
                            + f". Turn it on only from a feature {m['file']}'s "
                            f"`[with.{theirs}] features` lists (`\"{krate}/<feature>\"`)"
                        )
                        continue
            if ok:
                edges.append((krate, mine, dep["name"], theirs, how))
                continue
            if (krate, dep["name"]) in EXCEPTIONS:
                seen_exceptions.add((krate, dep["name"]))
                edges.append((krate, mine, dep["name"], theirs, f"excepted until #{EXCEPTIONS[(krate, dep['name'])][0]}"))
                continue
            if mine == kernel:
                fix = (
                    "a kernel crate may name no package's crate: take what it uses through a "
                    "point the kernel declares (ADR 0137), or move the code into the package"
                )
                if krate in LAUNCHERS:
                    fix += "; a launcher's package lines are package-sync's (`python3 scripts/package-sync.py`)"
                failures.append(
                    f"{krate} (the kernel) has a {what}{where} on {dep['name']} (the `{theirs}` "
                    f"package): {fix}"
                )
            else:
                failures.append(
                    f"{krate} (the `{mine}` package) has a {what}{where} on {dep['name']} (the "
                    f"`{theirs}` package), which {packages[mine]['file']}'s [requires] doesn't "
                    f"list: add `{theirs} = \"*\"` to its [requires], or make the dependency "
                    f"optional under `[with.{theirs}]`"
                )

    for (krate, dep), (issue, why) in sorted(EXCEPTIONS.items()):
        if (krate, dep) not in seen_exceptions:
            failures.append(
                f"the exception for {krate} -> {dep} (#{issue}) excuses nothing now (the edge "
                f"is gone, or the rule allows it): take it out of EXCEPTIONS in "
                f"scripts/package-deps-check.py"
            )
            continue
        state = issue_state(issue)
        if state is False:
            failures.append(
                f"the exception for {krate} -> {dep} names #{issue}, which is closed, but the "
                f"edge is still there ({why}): end the edge, or name the issue that will"
            )
        elif state is None:
            notes.append(f"couldn't ask GitHub whether #{issue} is open (no gh, no $GITHUB_TOKEN)")
    return failures, notes, edges


# --- the self-test ---------------------------------------------------------


def self_test(meta, packages):
    """Each way an edge can break the rule, planted in a copy of the real
    graph (no cargo build, no GitHub): each must fail naming its edge, and
    the allowed ones must pass. Done-when #1624: flint-physics in
    flint-render, and music on rhythm's crate."""
    import copy

    def crate(m, name):
        return next(p for p in m["packages"] if p["name"] == name and p["id"] in m["workspace_members"])

    def plant(m, krate, dep, kind=None, optional=False):
        crate(m, krate)["dependencies"].append(
            {"name": dep, "rename": None, "kind": kind, "optional": optional, "target": None,
             "features": [], "uses_default_features": True, "req": "*"}
        )

    def drop(m, krate, dep):
        c = crate(m, krate)
        c["dependencies"] = [d for d in c["dependencies"] if d["name"] != dep]

    open_ = lambda n: True  # noqa: E731
    cases = []  # (label, mutate meta, mutate packages, issue_state, expected text or None)
    cases.append(("main as it is", None, None, open_, None))
    cases.append(("flint-render gains flint-physics (#1624's done-when)",
                  lambda m: plant(m, "flint-render", "flint-physics"), None, open_,
                  "flint-render (the kernel) has a dependency on flint-physics"))
    cases.append(("flint-music gains flint-rhythm (#1624's done-when)",
                  lambda m: plant(m, "flint-music", "flint-rhythm"), None, open_,
                  "add `rhythm = \"*\"` to its [requires]"))
    cases.append(("... and passes once music requires rhythm (as written)",
                  lambda m: plant(m, "flint-music", "flint-rhythm"),
                  lambda p: p["music"]["requires"].append("rhythm"), open_, None))
    cases.append(("a kernel crate's optional dependency is still held",
                  lambda m: plant(m, "flint-script", "flint-physics", optional=True), None, open_,
                  "flint-script (the kernel) has a dependency on flint-physics"))
    cases.append(("a build-dependency is held",
                  lambda m: plant(m, "flint-render", "flint-terrain", kind="build"), None, open_,
                  "build-dependency on flint-terrain"))
    cases.append(("a dev-dependency is not",
                  lambda m: plant(m, "flint-render", "flint-physics", kind="dev"), None, open_, None))
    cases.append(("a launcher's package line outside the generated block",
                  lambda m: plant(m, "flint-player", "flint-wooze", optional=True), None, open_,
                  "flint-player (the kernel) has a dependency on flint-wooze"))
    def not_optional(m):
        for d in crate(m, "flint-player")["dependencies"]:
            if d["name"] == "flint-physics":
                d["optional"] = False
    cases.append(("a generated line that isn't optional", not_optional, None, open_,
                  "inside its package-sync block, but not as `optional`"))
    cases.append(("[requires] is not closed over: rhythm-game's audio",
                  None, lambda p: p["rhythm"]["requires"].remove("audio"), open_,
                  "flint-rhythm-game (the `rhythm` package) has a dependency on flint-audio"))
    cases.append(("a [with] part's dependency turned on by default",
                  lambda m: crate(m, "flint-procgen")["features"]["default"].append("terrain"), None,
                  open_, "doesn't name turn it on: `default`"))
    cases.append(("a [with] part's dependency that isn't optional",
                  lambda m: [d.update(optional=False) for d in crate(m, "flint-procgen")["dependencies"]
                             if d["name"] == "flint-terrain"], None, open_,
                  "flint-procgen (the `procgen` package) has a dependency on flint-terrain"))
    def ask(m, krate, dep, feature):
        for d in crate(m, krate)["dependencies"]:
            if d["name"] == dep:
                d["features"] = d["features"] + [feature]
    cases.append(("a [with] part turned on from another package's crate",
                  lambda m: ask(m, "flint-living", "flint-procgen", "terrain"), None, open_,
                  "flint-living (the `living` package) turns on flint-procgen's `terrain`, which "
                  "brings in the `terrain` package"))
    cases.append(("... and from that crate's own feature list",
                  lambda m: crate(m, "flint-living")["features"]["player"].append("flint-procgen/terrain"),
                  None, open_, "(from its feature `player`)"))
    def by_default(m):
        for d in crate(m, "flint-asset-gen")["dependencies"]:
            if d["name"] == "flint-procgen":
                d["uses_default_features"] = True
    cases.append(("... and by a dependency that keeps its default features", by_default, None, open_,
                  "flint-asset-gen (the `ai` package) turns on flint-procgen's `default`"))
    def on_a_target(m):
        for d in crate(m, "flint-player")["dependencies"]:
            if d["name"] == "flint-physics":
                d["target"] = "cfg(windows)"
    cases.append(("a launcher's package line under a target table, outside the block", on_a_target,
                  None, open_, "flint-player (the kernel) has a dependency (target cfg(windows)) on flint-physics"))
    cases.append(("a crate in a package's folder its manifest doesn't list",
                  None, lambda p: p["vehicle"]["crates"].remove("flint-vehicle"), open_,
                  "flint-vehicle is in packages/vehicle/ but"))
    cases.append(("an exception whose edge is gone",
                  lambda m: drop(m, "flint-procgen", "flint-wooze"), None, open_,
                  "excuses nothing now"))
    cases.append(("an exception whose issue is closed", None, None, lambda n: False,
                  "which is closed, but the edge is still there"))
    cases.append(("a crate two manifests claim",
                  None, lambda p: p["physics"]["crates"].append("flint-render"), open_,
                  "flint-render is in two manifests"))

    bad = 0
    for label, mutate_meta, mutate_packages, state, expected in cases:
        m, p = copy.deepcopy(meta), copy.deepcopy(packages)
        if mutate_meta:
            mutate_meta(m)
        if mutate_packages:
            mutate_packages(p)
        failures, _, _ = check(m, p, issue_state=state)
        text = "\n".join(failures)
        if expected is None:
            ok = not failures
        else:
            ok = expected in text
        bad += not ok
        print(f"  {'ok  ' if ok else 'FAIL'} {label}" + ("" if ok else f"\n       got: {text or 'a pass'}"))
    if bad:
        print(f"package-deps-check --self-test: {bad} case(s) wrong", file=sys.stderr)
        return 1
    print(f"package-deps-check --self-test: {len(cases)} cases, each as the rule says")
    return 0


def main(argv):
    meta_file = None
    if "--metadata" in argv:
        meta_file = argv[argv.index("--metadata") + 1]
    packages = manifests()
    meta = metadata(meta_file)
    if "--self-test" in argv:
        return self_test(meta, packages)
    failures, notes, edges = check(meta, packages)
    if "--graph" in argv:
        kernel = kernel_name(packages)
        for krate, mine, dep, theirs, how in sorted(edges, key=lambda e: (e[1] != kernel, e[1], e[0], e[2])):
            if theirs == kernel:
                continue
            print(f"{mine:>13} {krate:<20} -> {dep:<20} {theirs:<13} {how}")
    for note in notes:
        print(f"package-deps-check: note: {note}")
    if failures:
        for f in failures:
            print(f"package-deps-check: {f}", file=sys.stderr)
        print(
            "package-deps-check: the rule is ADR 0138 decision 6: a kernel crate depends on "
            "no package's crate, and a package's crate only on the kernel and what its "
            "[requires] lists",
            file=sys.stderr,
        )
        return 1
    owner, _ = owners(packages)
    kernel = kernel_name(packages)
    held_crates = sum(1 for p in meta["packages"] if p["name"] in owner)
    across = sum(1 for e in edges if e[3] != kernel)
    print(
        f"package-deps-check: {held_crates} crates in {len(packages)} manifests hold the rule "
        f"({across} edges between packages, each allowed; {len(EXCEPTIONS)} exception(s), "
        f"each with its issue)"
    )
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
