#!/bin/bash
# Spike #963: first-frame pipeline build time, driver cache vs wgpu PipelineCache.
# Usage: bash bench_cache.sh <rounds>
cd "$(dirname "$0")"
EXE=../release/flint.exe
ROUNDS=${1:-3}
SCENES="ascii_lantern=../../examples/ascii_lantern.scene.toml tavern=../../showcase_game/levels/tavern_hub.scene.toml neon=../../examples/neon_bloom.scene.toml"
out=bench_cache.tsv
echo -e "scene\tcondition\tround\trender_ms\tbuild_ms" > $out
run() { # scene path condition
  local line
  line=$("$EXE" render "$2" -o "img/$1-$3.png" --width 1280 --height 720 2>&1 | grep "spike963:")
  local r b
  r=$(echo "$line" | sed -E 's/.*render ([0-9]+) ms.*/\1/')
  b=$(echo "$line" | sed -E 's/.*pipelines ([0-9]+) ms.*/\1/')
  echo -e "$1\t$3\t$4\t$r\t$b" | tee -a $out
}
mkdir -p img
"$EXE" --version > /dev/null 2>&1
for round in $(seq 1 $ROUNDS); do
  for s in $SCENES; do
    name=${s%%=*}; path=${s#*=}
    W=$(cygpath -w "$PWD")
    # A: driver cache cold
    rm -rf drv; mkdir drv
    export __GL_SHADER_DISK_CACHE_PATH="$W\\drv"; unset __GL_SHADER_DISK_CACHE FLINT_SPIKE_PIPELINE_CACHE
    WGPU_BACKEND=vulkan run $name "$path" vk-driver-cold $round
    # B: driver cache warm (same dir)
    WGPU_BACKEND=vulkan run $name "$path" vk-driver-warm $round
    # C/D: a fresh driver cache each run, wgpu PipelineCache cold then warm
    rm -f wgpucache.bin*
    export FLINT_SPIKE_PIPELINE_CACHE="$W\wgpucache.bin"
    rm -rf drv; mkdir drv
    WGPU_BACKEND=vulkan run $name "$path" vk-wgpu-cold $round
    rm -rf drv; mkdir drv
    WGPU_BACKEND=vulkan run $name "$path" vk-wgpu-warm $round
    unset FLINT_SPIKE_PIPELINE_CACHE __GL_SHADER_DISK_CACHE_PATH
    # DX12: DXC every launch; the driver's own DXCache as the machine has it
    WGPU_BACKEND=dx12 run $name "$path" dx12 $round
  done
done
