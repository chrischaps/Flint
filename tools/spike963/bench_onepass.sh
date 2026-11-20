#!/bin/bash
# Spike #963 aside: Kuwahara walking its window once against eight times.
cd "$(dirname "$0")"
for round in 1 2 3; do
  for backend in vulkan dx12; do
    for v in f32 onepass; do
      line=$(WGPU_BACKEND=$backend FLINT_STYLE_SHADER_DIR="$(cygpath -w $PWD/shaders_$v)" \
        ../release/flint.exe render ../../showcase_game/levels/tavern_hub.scene.toml \
        -o "img/kuw-$backend-$v.png" --width 1920 --height 1080 --style kuwahara --gpu-time 300 2>&1)
      echo "$backend $v $round $(echo "$line" | grep -E '^  Kuwahara ')"
    done
  done
done
