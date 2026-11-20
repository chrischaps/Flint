# What wgpu 30 and egui 0.36 make possible (spike #963)

*2026-11-20. Measured on one machine: RTX 4070 (driver 610.88), Windows 11,
release build of main 7101efab plus the prototypes, Chrome 157 and Firefox 159.
The prototypes, bench scripts and raw numbers are on the branch
`spike/963-prototypes` (`tools/spike963/`), never merged. The presentation is
linked from PR and issue #963.*

## The question

#950 moved Flint from wgpu 23 to 30 and egui 0.30 to 0.36 like for like. This
spike asks what the new versions offer that Flint doesn't use yet, which of
those a player or a game author would feel, and which are worth tickets.

Sources: the wgpu changelog at v30.0.2 (v24 to v30), wgpu-types, wgpu-hal and
wgpu's WebGPU backend at 30.0.2 in the cargo registry, and egui's changelogs and
0.36.2 sources. Each row below was checked against one of these.

## What each adapter offers

`cargo run -p flint-render --example gpu_features` (added by this spike) prints
these on any machine. Flint asks for `TIMESTAMP_QUERY` and nothing else
(`context.rs`, `headless.rs`).

| Feature (wgpu 30 name) | Vulkan | DX12 | WARP | GL | Chrome 157 | Firefox 159 | Reaches the web player through wgpu 30? |
|---|---|---|---|---|---|---|---|
| `TIMESTAMP_QUERY` | yes | yes | yes | yes | yes | yes | yes |
| `TIMESTAMP_QUERY_INSIDE_PASSES` | yes | yes | yes | yes | - | - | native only |
| `PIPELINE_CACHE` | yes | - | - | - | - | - | native only (hal implements Vulkan only) |
| `SUBGROUP` | yes (32) | yes (32) | yes (4) | - | `subgroups` | - | **no**: not in wgpu's WebGPU feature map |
| `SHADER_F16` (`enable f16;`, WGSL since v25) | yes | yes | yes | - | yes | yes | yes |
| `DUAL_SOURCE_BLENDING` (spec syntax v25) | yes | yes | yes | yes | yes | yes | yes |
| `IMMEDIATES` (was push constants, v28) | 256 B | 128 B | 128 B | 256 B | 64 B | - | **no**: `set_immediates` panics on the WebGPU backend |
| `CLIP_DISTANCES` | yes | - | - | yes | yes | - | yes |
| `MULTIVIEW` / `SELECTIVE_MULTIVIEW` (v28) | 32 views | 4 views | 4 | - | - | - | native only |
| Binding arrays (`TEXTURE_BINDING_ARRAY`, ...) | 1M | 1M | 1M | 0 | - | - | native only (limit 0 on web) |
| `EXPERIMENTAL_RAY_QUERY` | yes | yes | yes | - | - | - | native, behind `ExperimentalFeatures` (v27) |
| `EXPERIMENTAL_MESH_SHADER` (WGSL on DX12/Metal v30) | yes | yes | yes | - | - | - | native, experimental |
| `LoadOp::DontCare` (v28) | yes | yes | yes | yes | clears | clears | becomes a clear |
| `TextureUsages::TRANSIENT_ATTACHMENT` (v28) | saves nothing here | - | - | - | unknown | unknown | - |
| HDR surface: `SurfaceConfiguration::color_space` (v30) | driver | scRGB, PQ | - | - | `toneMapping: extended` took | `rgba16float` canvas refused | ExtendedSrgb, DisplayP3 |
| `InstanceFlags::STRICT_WEBGPU_COMPLIANCE` (v30) | a native device limited to WebGPU's set | | | | | | n/a |

DX12 lists the 4070 three times (once a display output); the example prints it
once. Chrome's WGSL language features add `subgroup_id`, `immediate_address_space`,
`unrestricted_pointer_parameters` and `uniform_buffer_standard_layout`, none of
which naga 30 parses yet. naga 30 refuses `enable subgroups;`: subgroup builtins
are written with no `enable`, and the device feature gates them.

Already in 23, so nothing new: `SUBGROUP`, `SHADER_F16` as a feature (its WGSL
arrived in 25), `PIPELINE_CACHE`, `MULTIVIEW`, the timestamp features. Already
adopted by #944/#945: `create_shader_module_trusted` with loop bounding off.

## What was measured

### 1. First frame: whose pipeline cache? (`PIPELINE_CACHE`)

The prototype makes one `wgpu::PipelineCache` from a file and names it in all
50 pipeline descriptors of flint-render. `flint render` prints its time and the
time spent building pipelines (`pipeline_cache::thread_build_time`). NVIDIA's
own disk cache was isolated with `__GL_SHADER_DISK_CACHE_PATH`, which moves its
Vulkan cache too (`__GL_SHADER_DISK_CACHE=0` does not turn it off on Windows:
a fresh folder per run is the only cold cache). Median of 3, 1280x720:

| Scene | Vulkan, driver cache cold | Vulkan, driver cache warm | Vulkan, driver cold, wgpu cache warm | DX12 (any launch) |
|---|---|---|---|---|
| `ascii_lantern` (ASCII + edges) | 2484 ms building (3401 ms render) | 49 (967) | 147 (1123) | 1668 (2609) |
| `tavern_hub` (the showcase game) | 1325 (1960) | 37 (565) | 92 (650) | 682 (1260) |
| `neon_bloom` (user style, bloom) | 1310 (1996) | 40 (647) | 86 (715) | 659 (1268) |

- On this machine wgpu's cache saves nothing a player feels: NVIDIA's driver
  already caches Vulkan pipelines across launches, and does it better (49 ms
  against 147). It would matter where the driver keeps no cache. Android's
  Vulkan drivers are the likely case, and they are unmeasured here (they need
  the phone).
- **DX12 pays DXC on every launch**: 1.7 s for the ASCII scene and 0.7 s for a
  plain one, warm or cold. No cache covers naga's HLSL going through DXC; the
  driver's DXCache only skips DXIL to ISA. Vulkan is what this machine picks
  by default, but the book sends hybrid-laptop players to DX12.

### 2. f16 in Kuwahara (`SHADER_F16`)

Kuwahara's sector loop (rotation, distance, atan2, weights and the three
accumulators) in half precision, swapped in by `FLINT_STYLE_SHADER_DIR`
against an unchanged copy, one binary, alternating, 3 rounds, `tavern_hub
--style kuwahara` at 1080p, `--gpu-time 300` p50:

| | Vulkan | DX12 |
|---|---|---|
| f32 (shipped) | 4.56 ms | 4.68 ms |
| f16 | 5.19 ms (+14%) | 4.85 ms (+4%) |

It also moved 28% of the pixels by up to 77 levels. On this GPU f16 runs at the
f32 rate and the conversions cost extra. Where f16 runs at twice the rate
(Mali, Adreno, Apple) it might win, but that is unmeasured, so f16 isn't worth
a ticket on its own.

### 3. Found on the way: Kuwahara walks its window eight times

The loop above runs the whole `(2r+1)^2` window once per sector (8 times),
rotating, measuring and sampling every texel each time, and keeps only the
samples that fall in that sector. Walking the window once and adding each
sample into its sector's accumulator (arrays of 8) gives, same bench:

| | Vulkan | DX12 |
|---|---|---|
| shipped | 4.56 ms | 4.68 ms |
| one walk | **1.98 ms** | **1.53 ms** |

0 of 2,073,600 pixels differ on either backend: the same products are summed
per sector in the same order. This is not a wgpu 30 feature, but it is the
largest saving the spike found that a player would feel, in the most expensive
built-in style.

### 4. Flint already fits WebGPU natively (`STRICT_WEBGPU_COMPLIANCE`)

With `InstanceFlags::with_env()` (Flint builds its flags without wgpu's env
switches) and `WGPU_STRICT_WEBGPU_COMPLIANCE=1`, the device's features are
masked to WebGPU's and native-only limits zeroed. `flint goldens` on Vulkan:
all 125 cases pass, every one byte-identical with its golden, in 26.3 s. Six
example scenes (ASCII, neon, pencil draw-on, watercolor, comic panels, user
styles) render with no error. Today nothing native-only can reach the web
player. This mode is the cheap guard that keeps it so once a native-only feature is
adopted. It refuses `LoadOp::DontCare`, so adopting that needs a gate.

### 5. What the browsers have that wgpu 30 can't pass on

`tools/spike963/webgpu_features.mjs` asked each browser's adapter in a
throwaway profile:

- **Chrome 157** offers `subgroups` (32), `shader-f16`, `dual-source-blending`,
  `clip-distances`, `primitive-index`, `timestamp-query`, a 64-byte
  `maxImmediateSize`, and an `rgba16float` canvas with `toneMapping: extended`.
- **Firefox 159** offers `shader-f16`, `dual-source-blending`, `timestamp-query`,
  8 bind groups and no subgroups. It refuses an `rgba16float` canvas
  (bug 1834395).
- wgpu 30's WebGPU backend maps 16 features (`FEATURES_MAPPING`), with no
  subgroups and no primitive index, and its `set_immediates` panics. So on the
  web Flint gets f16, dual-source blending, clip distances, timestamps and an
  HDR canvas in Chrome, and nothing else new.

## Not measured

- Android: the pipeline cache, `LoadOp::DontCare` and transient attachments
  across the post chain (this GPU reports `transient_saves_memory: false`), and
  f16 at a tiler's double rate. These need the phone (`pixel-lock` protocol).
- HDR output: this display is SDR (`dynamic-range: standard`), so no picture
  could be judged.
- egui 0.31 to 0.36: no prototype. `egui_kittest` isn't in the registry, so
  its 0.36 API is unread.
- Timestamps inside passes, subgroups, immediates, multiview, ray queries, mesh
  shaders and binding arrays: only read and probed, not built (reasons below).

## egui 0.31 to 0.36, read

| Feature | Version | Flint's use |
|---|---|---|
| epaint rasterizes with skrifa + vello_cpu (hinting, variable-font axes); harfrust shaping | 0.34, 0.35 | The ASCII glyph atlas's TTF sets still use `ab_glyph`, its only user. On skrifa + vello_cpu (already built, at opt-level 3) they'd share egui's rasterizer and gain hinting and weights. epaint's raster API is private, so it's skrifa directly, not through epaint. Coverage changes, so the TTF ramps recalibrate and the ASCII goldens are re-blessed. |
| `egui::Scene` (pan and zoom over real widgets) | 0.31 | Zoom in `CurveEditor` / `GradientEditor`, which have none (#1360) |
| `dnd_set_drag_payload` / `dnd_drop_zone` | older | Reparent by drag in the scene tree, which has no drag (#1360) |
| `TextEdit::event_filter` | 0.36.2 | Which keys a text field takes. The viewer's host already skips keys egui consumes, so there's no problem shown for it to fix |
| `Response::total_drag_delta` | 0.33.2 | The gizmo's and curve editor's shift-drag lock without a kept origin |
| `Panel` (one API, drag to open and close) | 0.34 to 0.36 | The F3/F4 columns are hand-built |
| `DragValue::prefix/suffix` | before 0.31 | Not new: 0.34 only moved them to Atoms |
| `egui::Plugin` | 0.33 | Flint calls no `on_begin_pass` for it to replace |
| `egui_inspection` / `egui_mcp` (AccessKit tree over a port) | 0.35 | An agent driving F3/F4; eframe-only as shipped, so it needs wiring for Flint's host |
| Colour emoji | none | Does not exist in 0.36.2 |

## Read and not proposed

- **Subgroups** for ASCII, Kuwahara and bloom: every built-in style is a
  fragment pass, where subgroup layout over pixels is the driver's (a risk to
  byte-stable goldens), and wgpu can't reach the browser's `subgroups`.
- **Immediates** for per-draw data: native only, and the web backend panics.
  Flint's uniforms already come from caches (0 bind groups a frame at scale 1,
  #565).
- **Multiview** for #880's comic panels: views must be layers of one texture of
  one size drawn with one pipeline, and panels differ in size, look and post
  chain. It fits stereo, which Flint doesn't draw.
- **Ray queries and mesh shaders**: experimental, native only, no shadow, AO or
  picking path that needs them yet.
- **Binding arrays**: no web support, and bind-group churn is already gone.
- **Timestamps inside passes**: Flint's timer writes its timestamps from empty
  compute passes, which works on every backend and in both browsers. Inside a
  pass would be native only.
- **Dual-source blending**: one colour target only, and the post chain has no
  blend it would replace.

## Proposed tickets

Ranked by what a player or a game author would feel. Filed after the spike
(2026-11-20), at Chris's word.

1. **#1354 Kuwahara walks its window once.** Players of the painterly look:
   4.6 to 2.0 ms (Vulkan) and 4.7 to 1.5 ms (DX12) at 1080p, byte-identical.
   The shader is in `tools/spike963/kuwahara_onepass.wgsl`. Impact high,
   complexity low.
2. **#1355 DX12 compiles every shader through DXC at every launch.** Players
   on DX12 wait 0.7 to 1.7 s at every start; a DXIL cache needs
   `PASSTHROUGH_SHADERS` (DX12 supports it) and Flint calling DXC itself.
   Building pipelines on worker threads is safe again since wgpu 30's per-wait
   events (#948). A style switched on mid-game was already #1121, and
   preloading looks #1159, so this ticket is the launch. Impact medium (high
   for DX12 players), complexity high.
3. **#1356 HDR output.** Players with HDR displays see bloom, emissive and the
   sky past white. wgpu 30's `color_space` (scRGB/PQ on DX12, driver-dependent
   on Vulkan) and Chrome's `toneMapping: extended`; Firefox can't yet. Opt-in,
   tonemapped against `display_hdr_info().tone_map_headroom()`. Impact medium,
   complexity medium. It needs an HDR display to judge.
4. **#1357 Strict WebGPU mode in CI and `InstanceFlags::with_env()`.** For game
   authors: a native-only feature can't sneak into the web player. CI's GPU
   job sets `WGPU_STRICT_WEBGPU_COMPLIANCE=1` for the goldens, which pass today.
   With the env read on, `WGPU_VALIDATION` and friends work too. Impact medium,
   complexity low.
5. **#1358 Android: pipeline cache, `LoadOp::DontCare` and transient
   attachments, measured on the phone first.** Players on phones: the cold
   start (Vulkan on Android keeps no implicit cache on many drivers) and the
   post chain's bandwidth on a tiler. The prototype cache is about 60 lines
   plus the 50 descriptor sites. Impact medium, unknown until measured,
   complexity medium.
6. **#1359 TTF glyph sets on skrifa + vello_cpu.** Drops `ab_glyph` and with it
   the tree's only path to `ttf-parser`, unmaintained since RUSTSEC-2026-0192,
   which names skrifa as the alternative. Game authors making ASCII fonts get
   hinting and variable weights, with one rasterizer in the tree. Impact
   medium, complexity medium (recalibrate and re-bless).
7. **#1360 Editor: `Scene` zoom in the curve and gradient editors, and drag to
   reparent in the scene tree.** For game authors in `flint edit`. Impact low,
   complexity low.
8. **Watch upstream: wgpu's WebGPU backend mapping `subgroups` and immediates.**
   Chrome has both. No ticket until wgpu passes them through.
