#!/bin/bash
# Spike #963: Kuwahara's sector loop in f16 against the same shader in f32,
# both read from a FLINT_STYLE_SHADER_DIR copy, one binary, alternating.
cd "$(dirname "$0")"
EXE=../release/flint.exe
SCENE=../../showcase_game/levels/tavern_hub.scene.toml
export FLINT_SPIKE_F16=1
out=bench_f16.tsv
echo -e "backend\tvariant\tround\tkuwahara_min\tkuwahara_p50\tframe_p50" > $out
for round in 1 2 3; do
  for backend in vulkan dx12; do
    for v in f32 f16; do
      line=$(WGPU_BACKEND=$backend FLINT_STYLE_SHADER_DIR="$(cygpath -w $PWD/shaders_$v)" \
        "$EXE" render "$SCENE" -o "img/kuw-$backend-$v.png" --width 1920 --height 1080 \
        --style kuwahara --gpu-time 300 2>&1)
      k=$(echo "$line" | grep -E "^  Kuwahara " | awk '{print $2"\t"$3}')
      f=$(echo "$line" | grep -E "^  frame " | awk '{print $3}')
      echo -e "$backend\t$v\t$round\t$k\t$f" | tee -a $out
    done
  done
done
