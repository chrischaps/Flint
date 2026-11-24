#!/usr/bin/env bash
# #1250 scratch: samply record on Linux, the marker file beside the hitch log.
set -x
mkdir -p out
EXE=./target/profiling/flint-player
SCENE=flint-examples/hitch_metronome.scene.toml
export LIBGL_ALWAYS_SOFTWARE=1

wait_for() { # file, pattern, seconds
  for _ in $(seq 1 "$3"); do grep -q "$2" "$1" 2>/dev/null && return 0; sleep 1; done; return 1
}

sudo sysctl kernel.perf_event_paranoid=1
timeout 300 xvfb-run -a samply record --save-only -o profile.json.gz $EXE $SCENE \
  --exit-after 12 --hitch-log "$PWD/out/hitches.jsonl" > out/samply-record.log 2>&1
echo "exit $?" >> out/samply-record.log
ls -la out profile.json.gz >> out/samply-record.log 2>&1
cp profile.json.gz out/samply-profile.json.gz

( samply load --no-open profile.json.gz > out/samply-url.txt 2> out/samply-load.log ) &
wait_for out/samply-url.txt "profiler.firefox.com" 60
python3 ci-scratch-1250/marks.py profile.json.gz out/samply-url.txt out/hitches.jsonl out/marks-linux.json \
  > out/marks.log 2>&1
echo "marks exit $?" >> out/marks.log
pkill samply
exit 0
