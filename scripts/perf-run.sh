#!/usr/bin/env bash
# The book's perf method as one command (#1093): run the player N times on a
# scene, vsync off, and keep the least noisy run's --perf-report, with every
# run's numbers beside it so scripts/perf-compare.py can measure the
# run-to-run noise.
#
#   bash scripts/perf-run.sh <scene> [--runs 5] [--exit-after 12]
#       [--input-script FILE] [--player EXE] [--out perf.json] [-- <player flags>]
#
#   bash scripts/perf-run.sh levels/tavern.scene.toml --out before.json
#   (change something, build again)
#   bash scripts/perf-run.sh levels/tavern.scene.toml --out after.json
#   python scripts/perf-compare.py before.json after.json
#
# Each run is a fresh process (`FLINT_VSYNC=0`, `--exit-after`, `--no-activate`,
# `--quiet`, sound muted), so a run's warm caches don't flatter the next. The
# report kept is the run with the least frame p50: on a machine shared with
# other builds, noise only ever adds time. The player is the release build
# (`cargo build --release -p flint-player`) unless --player names another;
# a debug build's numbers compare only with a debug build's, and the reports
# say which they are.
set -euo pipefail

usage() {
    sed -n '2,21p' "$0" | sed 's/^# \{0,1\}//' >&2
    exit 2
}

here="$(cd "$(dirname "$0")" && pwd)"
scene=""
runs=5
exit_after=12
input_script=""
player=""
out="perf.json"
extra=()
while [ $# -gt 0 ]; do
    case "$1" in
        --runs) runs="$2"; shift 2 ;;
        --exit-after) exit_after="$2"; shift 2 ;;
        --input-script) input_script="$2"; shift 2 ;;
        --player) player="$2"; shift 2 ;;
        --out) out="$2"; shift 2 ;;
        -h|--help) usage ;;
        --) shift; extra=("$@"); break ;;
        -*) echo "perf-run: unknown flag $1" >&2; usage ;;
        *)
            if [ -n "$scene" ]; then echo "perf-run: one scene only ($scene, $1)" >&2; exit 2; fi
            scene="$1"; shift ;;
    esac
done
[ -n "$scene" ] || usage
case "$runs" in ''|*[!0-9]*|0) echo "perf-run: --runs takes a count of 1 or more" >&2; exit 2 ;; esac

if [ -z "$player" ]; then
    root="$(cd "$here/.." && pwd)"
    for candidate in "$root/target/release/flint-player.exe" "$root/target/release/flint-player"; do
        if [ -x "$candidate" ]; then player="$candidate"; break; fi
    done
    if [ -z "$player" ]; then
        echo "perf-run: no release player; build one (cargo build --release -p flint-player) or pass --player EXE" >&2
        exit 2
    fi
fi
# One that runs: on Windows `python3` can be the Store's stub, which only
# prints how to install Python
python=""
for candidate in python3 python py; do
    if command -v "$candidate" > /dev/null 2>&1 && "$candidate" -c 'import sys' > /dev/null 2>&1; then
        python="$candidate"
        break
    fi
done
[ -n "$python" ] || { echo "perf-run: needs Python 3 for the merge" >&2; exit 2; }

dir="$(mktemp -d "${TMPDIR:-/tmp}/flint-perf-run.XXXXXX")"
trap 'rm -rf "$dir"' EXIT
script_flags=()
if [ -n "$input_script" ]; then script_flags=(--input-script "$input_script"); fi

reports=()
for i in $(seq 1 "$runs"); do
    report="$dir/run-$i.json"
    echo "perf-run: run $i of $runs: $scene for ${exit_after} s" >&2
    if ! FLINT_VSYNC=0 "$player" "$scene" --exit-after "$exit_after" --perf-report "$report" \
        --no-activate --quiet --music-volume 0 --sfx-volume 0 \
        "${script_flags[@]}" "${extra[@]}" > "$dir/run-$i.log" 2>&1; then
        echo "perf-run: run $i failed; its log:" >&2
        tail -n 20 "$dir/run-$i.log" >&2
        exit 1
    fi
    [ -f "$report" ] || { echo "perf-run: run $i wrote no report" >&2; tail -n 20 "$dir/run-$i.log" >&2; exit 1; }
    reports+=("$report")
done
"$python" "$here/perf-compare.py" --merge-runs "$out" "${reports[@]}"
