# Spike #963 prototypes (throwaway)

This folder and the source edits in the same commit live only on the branch
`spike/963-prototypes`. The findings are `docs/design/wgpu-30-spike.md`.

- `crates/flint-render/src/driver_cache.rs`: `FLINT_SPIKE_PIPELINE_CACHE=<file>`
  makes a wgpu `PipelineCache` that every flint-render pipeline names
  (`proto_cache.py` rewrote the 50 `cache: None`), saved after `flint render`;
  `FLINT_SPIKE_F16=1` asks for `SHADER_F16`. `flint render` prints
  `spike963: render N ms, building pipelines M ms`. `gpu_select::instance`
  reads wgpu's own env switches (`WGPU_STRICT_WEBGPU_COMPLIANCE`).
- `bench_cache.sh`: the pipeline-cache table (`bench_cache.tsv`).
- `bench_f16.sh`, `bench_onepass.sh`: Kuwahara variants through
  `FLINT_STYLE_SHADER_DIR` (`kuwahara_f16.wgsl`, `kuwahara_onepass.wgsl`,
  made by `f16.py` / `onepass.py` from a copy of `src/stylize`).
- `webgpu_features.mjs`: what Chrome's and Firefox's adapters offer (`*.json`).
- `features.txt`: `cargo run -p flint-render --example gpu_features` here.
- `goldens-strict-summary.txt`: `flint goldens` under `WGPU_STRICT_WEBGPU_COMPLIANCE=1`.
