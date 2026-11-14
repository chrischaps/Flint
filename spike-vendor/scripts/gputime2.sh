#!/usr/bin/env bash
# GPU frame p50: main (wgpu 23) vs spike (wgpu 30) under FLINT_SHADER_CHECKS all / noloop / none.
S="C:/Users/chris/AppData/Local/Temp/claude/a6d66f2b54a46888/9f49eb0a-75de-4227-8d39-fd599c0c6bba/scratchpad"
MAIN=/c/Users/chris/dev/target-934main/debug/flint.exe
SPIKE=/c/Users/chris/dev/Flint-934/target/debug/flint.exe
cd /c/Users/chris/dev/Flint-934 || exit 1
for backend in ${BACKENDS:-vulkan dx12}; do
  for style in none ascii kuwahara watercolor pencil; do
    for round in 1 2; do
      for run in main spike:all spike:noloop spike:none; do
        bin=${run%%:*}; checks=${run#*:}
        if [ "$bin" = main ]; then F=$MAIN; checks=-; else F=$SPIKE; fi
        args=()
        [ "$style" != none ] && args=(--style "$style")
        out=$(FLINT_SHADER_CHECKS=$checks WGPU_BACKEND=$backend "$F" render tests/fixtures/scenes/pbr_test.scene.toml "${args[@]}" \
          --gpu-time 150 --output "$S/renders/gt2_${bin}_${checks}_${style}_${backend}.png" --width 1280 --height 720 2>&1)
        frame=$(echo "$out" | awk '$1=="frame"{print $3}')
        echo "$backend $style $bin $checks $round $frame"
      done
    done
  done
done
