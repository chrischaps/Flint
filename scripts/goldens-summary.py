"""Markdown table of a render-suite report.json, for a CI job summary (#314).

    python scripts/goldens-summary.py target/tmp/stylize-goldens/report.json

(the folder CI's FLINT_GOLDENS_OUT names; a local `cargo test` run keeps
its target/tmp/stylize-goldens-<pid> only when it fails, #421)

Each case's status and how far it sits from its golden, against the
tolerance it is held to, so a run on a software adapter shows the margin
and not only the verdict. A case that passes but isn't byte-identical
with its golden is marked drift (#387), and the last line names them.
A missing report says so, rather than nothing.
"""

import json
import sys


def main(path: str) -> None:
    print("### Stylize goldens\n")
    try:
        with open(path, encoding="utf-8") as f:
            cases = json.load(f)
    except OSError as e:
        print(f"No report at `{path}` ({e.strerror}): the suite never ran.")
        return
    adapter = next((c["adapter"] for c in cases if c.get("adapter")), "none found")
    print(f"Adapter: **{adapter}**\n")
    print("| case | status | past level | mean | max | limit (level / share / mean) | golden |")
    print("|---|---|---:|---:|---:|---|---|")
    counts = {}
    for c in cases:
        status = c["status"]
        counts[status] = counts.get(status, 0) + 1
        t = c["tolerance"]
        limit = f"{t['level']} / {t['share'] * 100:.2f}% / {t['mean']:.2f}"
        cmp = c.get("comparison")
        if cmp:
            row = f"{cmp['far_share'] * 100:.3f}% | {cmp['mean']:.3f} | {cmp['max']}"
        else:
            row = " | | "
        mark = {"pass": "pass", "fail": "**FAIL**", "skip": "**SKIP**"}.get(status, f"**{status}**")
        if c.get("drift") and status == "pass":
            mark = "pass, *drift*"
        golden = c["golden"].replace("\\", "/").rsplit("/", 1)[-1]
        print(f"| {c['name']} | {mark} | {row} | {limit} | {golden} |")
    print()
    print(", ".join(f"{n} {s}" for s, n in sorted(counts.items())))
    drifted = [c["name"] for c in cases if c.get("drift") and c["status"] == "pass"]
    if drifted:
        print(f"\nDrift inside the tolerance ({len(drifted)}): {', '.join(drifted)}")
    elif any(c.get("comparison") for c in cases):
        print("\nDrift: none, every passing case is byte-identical with its golden.")


if __name__ == "__main__":
    main(sys.argv[1])
