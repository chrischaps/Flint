### `flint goldens` keeps its cases' pipelines; `kit::shader` returns a `kit::Shader` (#911)
**What changed.** In one `flint goldens` process each case takes the
pipelines an earlier case built on the device (scene, post-process and
built-in style pipelines, keyed by format, sample count and WGSL text), so a
suite runs several times faster; the summary prints how many were built and
reused, and `--fresh-pipelines` builds them anew. `flint render` now leaves
out a user style an earlier render in the process loaded from a folder the
scene can't see (#426). For a game's Rust style: `stylize::kit::shader`
returns a `kit::Shader`, which derefs to the `wgpu::ShaderModule`, and the
`kit::fullscreen_pipeline*` functions return whatever their result is stored
as: a `wgpu::RenderPipeline` as before, or an `Arc<wgpu::RenderPipeline>`
that the cache may share.
**What to do.** Nothing, unless a style stores `kit::shader(..)` in a
`wgpu::ShaderModule` field: change the field to `kit::Shader`, or call
`.into_module()`.
