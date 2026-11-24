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
TH=$(python3 $S/symstats.py profile.json.gz out/samply-url.txt out/samply-$UNWIND-symstats.json 2> out/symstats.log | tail -1)
timeout 900 node $S/profshot.mjs "$URL" "out/samply-$UNWIND" "$(command -v google-chrome)" "$TH" > out/profshot.log 2>&1
echo "profshot exit $?" >> out/profshot.log
pkill samply

# 4. perf --call-graph fp (the book: needs the frame-pointer build).
timeout 120 xvfb-run -a perf record -F 999 --call-graph fp -o perf.fp.data $EXE $SCENE --exit-after 12 \
  > out/perf-fp-record.log 2>&1
perf script -i perf.fp.data --no-inline > perf.fp.txt 2> out/perf-fp-script.log
python3 $S/perfstats.py perf.fp.txt flint-player > out/perf-fp-$UNWIND.json

# 5. perf --call-graph dwarf, and Hotspot reading it (the plain build only).
if [ "$UNWIND" = eh_frame ]; then
  timeout 180 xvfb-run -a perf record -F 199 --call-graph dwarf -o perf.dwarf.data $EXE $SCENE --exit-after 12 \
    > out/perf-dwarf-record.log 2>&1
  ls -la perf.dwarf.data >> out/perf-dwarf-record.log
  start=$(date +%s)
  timeout 1200 perf script -i perf.dwarf.data --no-inline > perf.dwarf.txt 2> out/perf-dwarf-script.log
  echo "perf script exit $? in $(( $(date +%s) - start )) s" >> out/perf-dwarf-script.log
  head -c 200000 perf.dwarf.txt > out/perf-dwarf-head.txt
  python3 $S/perfstats.py perf.dwarf.txt flint-player > out/perf-dwarf.json
  # samply can read the same perf.data and unwind it itself
  timeout 600 samply import --save-only -o out/samply-import-dwarf.json.gz perf.dwarf.data > out/samply-import.log 2>&1
  echo "samply import exit $?" >> out/samply-import.log
  # Hotspot: open the recording on Xvfb, give it time to parse, photograph the screen
  hotspot --version > out/hotspot.log 2>&1
  timeout 400 xvfb-run -a -s "-screen 0 1600x1000x24" bash -c '
    echo "DISPLAY=$DISPLAY"
    hotspot perf.dwarf.data & h=$!
    for t in 60 120 180 240; do sleep 60; import -window root out/hotspot-$t.png; done
    kill $h' >> out/hotspot.log 2>&1
  echo "hotspot exit $?" >> out/hotspot.log
fi
ls -la out
exit 0
