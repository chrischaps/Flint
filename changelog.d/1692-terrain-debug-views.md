### `--debug-mode` and the F4 Shading modes draw the terrain (#1692)
**Package.** engine
**What changed.** A terrain now takes every debug view the meshes do. `wireframe` draws its triangles, with each chunk's border in amber, and the hills hide what stands behind them. Before, the terrain disappeared and left only the sky. `wireframe-overlay` draws the same lines over the lit terrain. `normals`, `depth`, `uv`, `unlit` and `metalrough` show the terrain's own values, where before it was drawn lit as usual. The normal lit frame is byte-identical to before.
**What to do.** Nothing.
