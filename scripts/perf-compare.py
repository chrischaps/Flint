#!/usr/bin/env python3
"""Compare two Flint perf reports (#1093).

    python scripts/perf-compare.py base.json new.json
    python scripts/perf-compare.py base.json new.json --metric frame.p50_ms --tolerance 3%
    python scripts/perf-compare.py base.json new.json --control gpu.passes.Shadows.p50_ms
    python scripts/perf-compare.py --merge-runs out.json run1.json run2.json ...

It reads `flint-player --perf-report` files and `flint render --gpu-time-json`
files, which name their GPU passes alike (`label`, `p50_ms`, `p90_ms`,
`max_ms`, `mean_ms`, `frames`), and prints a table of every metric both
have: base, new, the change, and whether that change is past the noise.

**The noise floor** is the larger of:

- `--noise` (default 5%): what one binary moves by between two sets of
  runs. Measured for #1093 (RTX 4070 laptop, Vulkan, release, the frozen
  `ascii_entities` scene at 1280x720, vsync off): two sets of five of the
  same binary read frame p50 1.254 and 1.318 ms, 5.1% apart, while each
  set's quieter runs agreed to 0.04 ms. A set drifts more than a run;
- `0.01 ms` of a time or one hitch of a count, below which nothing is said;
- the run-to-run range of each metric over the quieter half of the runs,
  when a report carries them (`scripts/perf-run.sh` merges five into one
  with `--merge-runs`). A run another build disturbed reads high, and is
  named in a warning rather than allowed to swamp the floor;
- the change of `--control METRIC`, a metric the change can't have touched
  (a GPU pass it doesn't draw): what the machine moved by between the runs.

A change below it is said `within noise`, never as a delta to trust. The
book's profiling guide measured identical binaries 8% apart on a shared
machine; one run each is weak evidence, and the script says so. Taking
base and new in turns (base, new, base) and a `--control` are what make a
smaller change believable. A worst frame (`max_ms`) is one frame: it is
listed with `--all`, never on its own as a finding.

**Refusals** (exit 2): reports whose adapter, backend, resolution or build
profile differ, or of two kinds (a player report and a `--gpu-time` file),
unless `--force`. A debug and a release frame time compare with nothing.

**The gate** (exit 1): each `--metric` (default `frame.p50_ms`, or
`gpu.frame.p50_ms` for `--gpu-time` files) that is worse by more than
`--tolerance` (default 10%) and by more than the noise floor. Lower is better
for every metric but `fps`.

Not a CI gate: CI has only WARP, and wall-clock numbers there are noise.
It is a tool for one machine. Standard library only.
"""

from __future__ import annotations

import argparse
import json
import sys

PLAYER = "flint-perf-report"
GPU_TIME = "flint-gpu-time"
# The report versions this script reads
VERSIONS = {PLAYER: 1, GPU_TIME: 1}
SPREAD_KEYS = ("p50_ms", "p90_ms", "p99_ms", "max_ms", "mean_ms", "min_ms")


def fail(message: str, code: int = 2) -> None:
    print(f"perf-compare: {message}", file=sys.stderr)
    sys.exit(code)


def load(path: str) -> dict:
    try:
        with open(path, encoding="utf-8") as f:
            doc = json.load(f)
    except OSError as e:
        fail(f"{path}: {e.strerror}")
    except json.JSONDecodeError as e:
        fail(f"{path}: not JSON ({e})")
    kind = kind_of(doc)
    if kind is None:
        fail(f"{path}: neither a --perf-report nor a --gpu-time-json file")
    version = doc.get("version", 1)
    if version > VERSIONS[kind]:
        fail(f"{path}: {kind} version {version}; this script reads up to {VERSIONS[kind]}")
    return doc


def kind_of(doc: dict) -> str | None:
    fmt = doc.get("format")
    if fmt in VERSIONS:
        return fmt
    # A --gpu-time-json file from before #1093 had no `format`
    if fmt is None and "passes" in doc and "adapter" in doc:
        return GPU_TIME
    return None


def spread(prefix: str, s: dict | None, out: dict) -> None:
    if not isinstance(s, dict):
        return
    # Not `lumps`: frames over 1.5x each run's own median, which moves with
    # the median, so two runs' counts don't compare
    for key in SPREAD_KEYS:
        if isinstance(s.get(key), (int, float)):
            out[f"{prefix}.{key}"] = float(s[key])


def gpu_passes(gpu: dict, out: dict) -> None:
    spread("gpu.frame", gpu.get("frame"), out)
    for p in gpu.get("passes") or []:
        spread(f"gpu.passes.{p.get('label')}", p, out)


def metrics(doc: dict) -> dict[str, float]:
    """Every number worth comparing, by a dotted name both kinds share."""
    out: dict[str, float] = {}
    if kind_of(doc) == GPU_TIME:
        gpu_passes(doc, out)
        return out
    run = doc.get("run") or {}
    for key in ("mean_ms", "fps"):
        if isinstance(run.get(key), (int, float)):
            out[f"run.{key}"] = float(run[key])
    spread("frame", doc.get("frame"), out)
    spread("dt", doc.get("dt"), out)
    for key, v in (doc.get("draws") or {}).items():
        out[f"draws.{key}"] = float(v)
    cpu = doc.get("cpu") or {}
    if isinstance(cpu.get("work_ms"), (int, float)):
        out["cpu.work_ms"] = float(cpu["work_ms"])
    for group in ("phases", "scripts"):
        for row in cpu.get(group) or []:
            for key in ("mean_ms", "max_ms"):
                out[f"cpu.{group}.{row['name']}.{key}"] = float(row[key])
    gpu = doc.get("gpu")
    if isinstance(gpu, dict):
        if gpu.get("timed_frames"):
            out["gpu.mean_ms"] = float(gpu["mean_ms"])
        gpu_passes(gpu, out)
    hitches = doc.get("hitches") or {}
    for key in ("after_start", "per_minute", "startup"):
        if isinstance(hitches.get(key), (int, float)):
            out[f"hitches.{key}"] = float(hitches[key])
    return out


def machine(doc: dict) -> dict:
    """What has to match for two reports to be compared."""
    if kind_of(doc) == GPU_TIME:
        a = doc.get("adapter") or {}
        return {
            "adapter": a.get("name"),
            "backend": a.get("backend"),
            "resolution": f"{doc.get('width')}x{doc.get('height')}",
            "build profile": doc.get("build_profile"),
        }
    m = doc.get("machine") or {}
    return {
        "adapter": m.get("adapter"),
        "backend": m.get("backend"),
        "resolution": f"{m.get('width')}x{m.get('height')}",
        "build profile": m.get("build_profile"),
    }


def describe(path: str, doc: dict) -> str:
    m = machine(doc)
    commit = (doc.get("machine") or {}).get("engine_commit") or doc.get("engine_commit")
    at = f" at {commit[:10]}" if commit else ""
    who = doc.get("written_by") or "flint render --gpu-time"
    extra = ""
    if kind_of(doc) == PLAYER:
        mode = (doc.get("machine") or {}).get("present_mode")
        if mode:
            extra = f", present {mode}"
    runs = doc.get("runs")
    if runs:
        extra += f", least of {runs['count']} runs"
    return (
        f"{path}: {who} {m['build profile'] or '?'}{at}, {m['adapter'] or '?'} "
        f"({m['backend'] or '?'}), {m['resolution']}{extra}"
    )


def lower_is_better(name: str) -> bool:
    return not name.endswith(".fps")


def percent(text: str) -> float:
    text = text.strip()
    value = float(text[:-1]) / 100.0 if text.endswith("%") else float(text)
    if value < 0:
        raise argparse.ArgumentTypeError("must be 0 or more")
    return value


def pick_metric(doc: dict) -> str | None:
    """The metric a merged report's runs were picked by (`least <metric>`)."""
    rule = (doc.get("runs") or {}).get("rule", "")
    return rule.removeprefix("least ") or None


def all_run_values(doc: dict, name: str) -> list[float | None]:
    runs = doc.get("runs") or {}
    return list((runs.get("metrics") or {}).get(name) or [])


def quiet_runs(doc: dict) -> list[int]:
    """The runs (0-based) that weren't disturbed: those whose pick metric
    read under DISTURBED times the least. A disturbed run is dropped from
    every metric, since the build that slowed it moved its waits and
    passes too."""
    by = all_run_values(doc, pick_metric(doc) or "")
    if not by:
        return []
    least = min(v for v in by if v is not None)
    return [i for i, v in enumerate(by) if v is not None and v < least * DISTURBED]


def run_values(doc: dict, name: str) -> list[float]:
    values = all_run_values(doc, name)
    quiet = quiet_runs(doc)
    return [values[i] for i in quiet if i < len(values) and values[i] is not None]


def run_range(doc: dict, name: str) -> float | None:
    """How far a metric moved between the quieter half of the runs a merged
    report was picked from (at least two), or None for a single run. Noise
    on a shared machine only adds time (takes away fps), so a run another
    build disturbed reads worse: the whole range would make every change
    look like noise (one disturbed run in five read 38 ms against 1.4)."""
    values = sorted(run_values(doc, name), reverse=not lower_is_better(name))
    if len(values) < 2:
        return None
    quiet = max(2, (len(values) + 1) // 2)
    return abs(values[quiet - 1] - values[0])


def best_of_runs(doc: dict, found: dict[str, float]) -> dict[str, float]:
    """Each metric as the least of the runs (the most, for fps): the book's
    least of N, metric by metric, since the run with the least frame p50
    needn't be the one with the least of every pass. A single run's own."""
    out = dict(found)
    for name in found:
        values = run_values(doc, name)
        if values:
            out[name] = min(values) if lower_is_better(name) else max(values)
    return out


# The least change worth a word: 10 us of a time (a timestamp's grain on
# most GPUs is far finer, a CPU phase's mark about 0.1 us, but runs move
# by more), and one of a count
FLOOR_MS = 0.01
FLOOR_COUNT = 1.0


def is_count(name: str) -> bool:
    return name.startswith("hitches.") and not name.endswith("per_minute")


# A run whose gated metric reads this many times the least was disturbed
DISTURBED = 1.5


def disturbed_runs(doc: dict) -> list[int]:
    """The runs (1-based) left out as disturbed."""
    count = len(all_run_values(doc, pick_metric(doc) or ""))
    quiet = set(quiet_runs(doc))
    return [i + 1 for i in range(count) if i not in quiet]


def compare(args: argparse.Namespace) -> int:
    base, new = load(args.base), load(args.new)
    if kind_of(base) != kind_of(new) and not args.force:
        fail(
            f"{args.base} is a {kind_of(base)} file and {args.new} a {kind_of(new)}: "
            "a player run and a headless --gpu-time measure different frames (--force to compare anyway)"
        )
    mb, mn = machine(base), machine(new)
    differ = [k for k in mb if mb[k] != mn[k]]
    if differ and not args.force:
        lines = [f"  {k}: {mb[k]} against {mn[k]}" for k in differ]
        fail(
            "these reports ran on different setups, so their numbers don't compare:\n"
            + "\n".join(lines)
            + "\n(--force to compare anyway)"
        )
    a, b = best_of_runs(base, metrics(base)), best_of_runs(new, metrics(new))
    names =[n for n in a if n in b and (a[n] != 0 or b[n] != 0)]

    gated = args.metric or [
        n for n in ("frame.p50_ms", "gpu.frame.p50_ms") if n in a and n in b
    ][:1]
    for g in gated:
        if g not in a or g not in b:
            fail(f"--metric {g}: not in both reports (they have e.g. {', '.join(names[:6])})")

    control_rel = None
    if args.control:
        c = args.control
        if c not in a or c not in b:
            fail(f"--control {c}: not in both reports")
        if a[c] > 0:
            control_rel = abs(b[c] - a[c]) / a[c]

    rows = []
    for n in names:
        delta = b[n] - a[n]
        rel = delta / a[n] if a[n] else None
        floors = {
            "--noise": args.noise * abs(a[n]),
            # Below a timer's grain, or one hitch either way
            "the least step": FLOOR_MS if n.endswith("_ms") else FLOOR_COUNT if is_count(n) else 0.0,
        }
        for which, doc in (("base runs", base), ("new runs", new)):
            r = run_range(doc, n)
            if r is not None:
                floors[which] = r
        if control_rel is not None:
            floors["control"] = control_rel * abs(a[n])
        floor_from = max(floors, key=floors.get)
        floor = floors[floor_from]
        worse = delta > 0 if lower_is_better(n) else delta < 0
        if abs(delta) <= floor:
            verdict = "within noise"
        elif n.endswith("_ms"):
            verdict = "slower" if worse else "faster"
        elif n.endswith(".fps"):
            verdict = "slower" if worse else "faster"
        else:
            verdict = "more" if delta > 0 else "fewer"
        rows.append(
            {
                "metric": n,
                "base": a[n],
                "new": b[n],
                "delta": delta,
                "relative": rel,
                "noise": floor,
                "noise_from": floor_from,
                "verdict": verdict,
                "worse": worse and verdict != "within noise",
            }
        )

    failures = []
    for g in gated:
        row = next(r for r in rows if r["metric"] == g) if g in names else None
        if row is None:
            continue
        past_tolerance = row["relative"] is not None and (
            (row["relative"] > args.tolerance)
            if lower_is_better(g)
            else (-row["relative"] > args.tolerance)
        )
        if row["worse"] and past_tolerance:
            failures.append(row)

    single = not base.get("runs") and not new.get("runs") and control_rel is None
    disturbed = {which: disturbed_runs(doc) for which, doc in (("base", base), ("new", new))}
    if args.json:
        print(
            json.dumps(
                {
                    "base": {"file": args.base, "machine": mb},
                    "new": {"file": args.new, "machine": mn},
                    "forced": bool(differ) or kind_of(base) != kind_of(new),
                    "noise": args.noise,
                    "control": args.control,
                    "control_change": control_rel,
                    "tolerance": args.tolerance,
                    "gated": gated,
                    "single_runs": single,
                    "disturbed_runs": disturbed,
                    "metrics": rows,
                    "regressions": [r["metric"] for r in failures],
                    "passed": not failures,
                },
                indent=2,
            )
        )
        return 1 if failures else 0

    print("base " + describe(args.base, base))
    print("new  " + describe(args.new, new))
    if differ:
        print("forced past: " + ", ".join(f"{k} {mb[k]} / {mn[k]}" for k in differ))
    floor_words = [f"{args.noise:.0%} (--noise)", f"{FLOOR_MS} ms or one hitch"]
    if base.get("runs") or new.get("runs"):
        floor_words.append("each metric's run-to-run range")
    if control_rel is not None:
        floor_words.append(f"{args.control} moved {control_rel:+.1%}")
    print("noise floor: the largest of " + ", ".join(floor_words))
    if base.get("runs") or new.get("runs"):
        print("  (a run-to-run range is over the quieter half of the runs)")
    if single:
        print(
            "  one run each: run-to-run noise is unmeasured; scripts/perf-run.sh takes several"
        )
    for which, path, doc in (("base", args.base, base), ("new", args.new, new)):
        why = (doc.get("gpu") or {}).get("timestamps_unavailable")
        if why:
            print(
                f"  warning: {which}'s GPU pass times are unavailable on its adapter ({why}): "
                "its gpu.* totals count only the scopes that read"
            )
        runs = disturbed[which]
        if runs:
            count = (doc.get("runs") or {}).get("count", "?")
            words = ", ".join(str(r) for r in runs)
            print(
                f"  warning: {which} run{'s' if len(runs) > 1 else ''} {words} of {count} read "
                f"{DISTURBED}x the least {pick_metric(doc)} or more: the machine was busy, so "
                f"{'they are' if len(runs) > 1 else 'it is'} left out. {path} deserves another "
                f"go on a quiet machine"
            )
    # A worst frame is one frame: shown with --all, never a finding alone
    shown = rows if args.all else [
        r
        for r in rows
        if r["metric"] in gated
        or (r["verdict"] != "within noise" and not r["metric"].endswith(".max_ms"))
    ]
    hidden = len(rows) - len(shown)
    if shown:
        w = max(len(r["metric"]) for r in shown)
        print()
        print(f"  {'metric':<{w}} {'base':>10} {'new':>10} {'change':>10} {'%':>8}  ")
        for r in shown:
            rel = f"{r['relative']:+.1%}" if r["relative"] is not None else ""
            gate = " *" if r["metric"] in gated else ""
            print(
                f"  {r['metric']:<{w}} {r['base']:>10.3f} {r['new']:>10.3f} "
                f"{r['delta']:>+10.3f} {rel:>8}  {r['verdict']}{gate}"
            )
    if hidden:
        print(f"  ... and {hidden} within noise or a single worst frame (--all shows them)")
    print()
    for g in gated:
        row = next((r for r in rows if r["metric"] == g), None)
        if row is None:
            continue
        if row in failures:
            print(
                f"REGRESSION {g}: {row['base']:.3f} -> {row['new']:.3f} ({row['relative']:+.1%}), "
                f"past the {args.tolerance:.0%} tolerance and the {row['noise']:.3f} noise floor"
            )
        elif row["verdict"] == "within noise":
            print(f"ok {g}: within noise ({row['noise']:.3f}, from {row['noise_from']})")
        else:
            rel = f"{row['relative']:+.1%}" if row["relative"] is not None else ""
            print(f"ok {g}: {row['verdict']} {rel}, within the {args.tolerance:.0%} tolerance")
    return 1 if failures else 0


def merge_runs(out: str, paths: list[str]) -> int:
    """`scripts/perf-run.sh`'s last step: of several runs of one binary, keep
    the least noisy (the least frame p50: noise on a shared machine only
    ever adds time) and carry every run's metrics, so a compare can measure
    each metric's run-to-run range."""
    if len(paths) < 1:
        fail("--merge-runs needs at least one run")
    docs = [load(p) for p in paths]
    kinds = {kind_of(d) for d in docs}
    if len(kinds) > 1:
        fail("--merge-runs: the runs are of different kinds")
    setups = {json.dumps(machine(d), sort_keys=True) for d in docs}
    if len(setups) > 1:
        fail("--merge-runs: the runs ran on different setups")
    all_metrics = [metrics(d) for d in docs]
    rule = "frame.p50_ms" if "frame.p50_ms" in all_metrics[0] else "gpu.frame.p50_ms"
    if any(rule not in m for m in all_metrics):
        fail(f"--merge-runs: a run has no {rule}")
    picked = min(range(len(docs)), key=lambda i: all_metrics[i][rule])
    merged = dict(docs[picked])
    names = sorted(set().union(*all_metrics))
    merged["runs"] = {
        "count": len(docs),
        "picked": picked,
        "rule": f"least {rule}",
        "files": paths,
        "metrics": {n: [m.get(n) for m in all_metrics if n in m] for n in names},
    }
    with open(out, "w", encoding="utf-8", newline="\n") as f:
        json.dump(merged, f, indent=2)
        f.write("\n")
    values = [m[rule] for m in all_metrics]
    print(
        f"perf-compare: {out}: run {picked + 1} of {len(docs)} "
        f"(least {rule}, {min(values):.3f}; runs {min(values):.3f}..{max(values):.3f})"
    )
    return 0


def main() -> int:
    p = argparse.ArgumentParser(
        description="Compare two Flint perf reports (--perf-report, --gpu-time-json).",
        formatter_class=argparse.RawDescriptionHelpFormatter,
        epilog="Exit 0: no gated metric regressed. 1: one did. 2: refused or bad input.",
    )
    p.add_argument("base", nargs="?")
    p.add_argument("new", nargs="?")
    p.add_argument(
        "--metric",
        action="append",
        help="a metric to gate on, repeatable (default frame.p50_ms, or gpu.frame.p50_ms)",
    )
    p.add_argument("--tolerance", type=percent, default=0.10, help="how much worse a gated metric may get (default 10%%)")
    p.add_argument("--noise", type=percent, default=0.05, help="the least noise floor (default 5%%)")
    p.add_argument("--control", help="a metric the change can't touch: its change is noise")
    p.add_argument("--force", action="store_true", help="compare reports from different setups or kinds")
    p.add_argument("--all", action="store_true", help="show the metrics within noise too")
    p.add_argument("--json", action="store_true", help="one JSON document on stdout")
    p.add_argument(
        "--merge-runs",
        nargs="+",
        metavar=("OUT", "RUN"),
        help="write OUT: the least noisy of the RUN reports, with every run's metrics",
    )
    args = p.parse_args()
    if args.merge_runs:
        if args.base or args.new:
            fail("--merge-runs takes OUT and the runs, not base and new")
        return merge_runs(args.merge_runs[0], args.merge_runs[1:])
    if not args.base or not args.new:
        p.print_usage(sys.stderr)
        fail("give a base report and a new one")
    return compare(args)


if __name__ == "__main__":
    sys.exit(main())
