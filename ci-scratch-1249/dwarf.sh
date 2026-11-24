#!/usr/bin/env bash
# #1249 scratch: why perf's --call-graph dwarf stacks stop after a frame or two here.
set -x
mkdir -p out
S=ci-scratch-1249
EXE=./target/profiling/flint-player
SCENE=flint-examples/hitch_metronome.scene.toml
sudo sysctl kernel.perf_event_paranoid=1
perf version --build-options > out/perf-build-options.txt 2>&1
for size in default 65528; do
  cg=dwarf; [ "$size" = default ] || cg="dwarf,$size"
  timeout 180 xvfb-run -a perf record -F 199 --call-graph $cg -o perf.$size.data $EXE $SCENE --exit-after 12 \
    > out/perf-$size-record.log 2>&1
  ls -la perf.$size.data >> out/perf-$size-record.log
  timeout 1200 perf script -i perf.$size.data --no-inline > perf.$size.txt 2> out/perf-$size-script.log
  echo "perf script exit $?" >> out/perf-$size-script.log
  python3 $S/perfstats.py perf.$size.txt flint-player > out/perf-dwarf-$size.json
  grep -m3 -i "unwind\|dwarf" out/perf-$size-script.log
  timeout 600 samply import --save-only -o out/samply-import-$size.json.gz perf.$size.data > out/samply-import-$size.log 2>&1
done
# Is it perf's unwinder or the data? perf report's own view of the same file
perf report -i perf.default.data --stdio --no-children -g caller --percent-limit 5 2>&1 | head -150 > out/perf-report-default.txt
ls -la out
exit 0
