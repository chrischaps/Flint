#!/usr/bin/env bash
# #1249 scratch: the book's Linux sampling commands, as written, on a runner.
# $UNWIND is eh_frame (the plain profiling build) or fp (RUSTFLAGS frame pointers).
set -x
mkdir -p out
S=ci-scratch-1249
EXE=./target/profiling/flint-player
SCENE=flint-examples/hitch_metronome.scene.toml
export LIBGL_ALWAYS_SOFTWARE=1

wait_for() { # file, pattern, seconds
  for _ in $(seq 1 "$3"); do grep -q "$2" "$1" 2>/dev/null && return 0; sleep 1; done; return 1
}

echo "perf_event_paranoid before: $(cat /proc/sys/kernel/perf_event_paranoid)" | tee out/paranoid.txt
vulkaninfo --summary > out/vulkaninfo.txt 2>&1

# 1. samply with the runner's default paranoid level: does it say so?
timeout 60 xvfb-run -a samply record $EXE $SCENE --exit-after 12 --hitch-log out/hitches-paranoid.jsonl \
  > out/samply-paranoid.log 2>&1
echo "exit $?" >> out/samply-paranoid.log

sudo sysctl kernel.perf_event_paranoid=1

# 2. The book's command, as written (inside xvfb-run: the runner has no display).
rm -f profile.json.gz
( timeout 180 xvfb-run -a samply record $EXE $SCENE --exit-after 12 --hitch-log hitches.jsonl \
  > out/samply-record.log 2>&1; echo "exit $?" >> out/samply-record.log ) &
wait_for out/samply-record.log "Local server listening\|exit " 170
sleep 2
pkill -INT samply; sleep 2; pkill samply
cp hitches.jsonl out/ 2>/dev/null
ls -la profile.json.gz >> out/samply-record.log 2>&1
cp profile.json.gz out/samply-profile.json.gz

# 3. Open it the way the browser would: samply load prints the URL with --no-open.
( samply load --no-open profile.json.gz > out/samply-url.txt 2> out/samply-load.log ) &
wait_for out/samply-url.txt "profiler.firefox.com" 60
URL=$(grep -o 'https://profiler.firefox.com[^ ]*' out/samply-url.txt | head -1)
timeout 600 node $S/profshot.mjs "$URL" "out/samply-$UNWIND" "$(command -v google-chrome)" > out/profshot.log 2>&1
echo "profshot exit $?" >> out/profshot.log
pkill samply

# 4. perf --call-graph fp (the book: needs the frame-pointer build).
timeout 120 xvfb-run -a perf record -F 999 --call-graph fp -o perf.fp.data $EXE $SCENE --exit-after 12 \
  > out/perf-fp-record.log 2>&1
perf script -i perf.fp.data --no-inline > perf.fp.txt 2> out/perf-fp-script.log
python3 $S/perfstats.py perf.fp.txt flint-player > out/perf-fp-$UNWIND.json

# 5. perf --call-graph dwarf, and Hotspot reading it (the plain build only).
if [ "$UNWIND" = eh_frame ]; then
  timeout 180 xvfb-run -a perf record -F 499 --call-graph dwarf -o perf.dwarf.data $EXE $SCENE --exit-after 12 \
    > out/perf-dwarf-record.log 2>&1
  timeout 900 perf script -i perf.dwarf.data > perf.dwarf.txt 2> out/perf-dwarf-script.log
  python3 $S/perfstats.py perf.dwarf.txt flint-player > out/perf-dwarf.json
  hotspot --version > out/hotspot.log 2>&1
  timeout 900 xvfb-run -a hotspot --exportTo out/hotspot.perfparser perf.dwarf.data >> out/hotspot.log 2>&1
  echo "hotspot exit $?" >> out/hotspot.log
  ls -la out/hotspot.perfparser >> out/hotspot.log 2>&1
  rm -f out/hotspot.perfparser.keep
fi
ls -la out
exit 0
