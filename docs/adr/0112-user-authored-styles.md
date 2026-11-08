# ADR 0112: User-authored styles — one WGSL function beside a `.shader.toml`

*Accepted 2026-11-07. Spike #274 for epic #260; the production loader is #275.
Builds on [ADR 0111](0111-stylized-rendering.md). Book:
[Stylize Passes → User-authored styles](../book/src/concepts/stylize-passes.md#user-authored-styles).
Prototype: `crates/flint-render/src/stylize/custom.rs`, demos in `demo/styles/`.*

## Context

ADR 0111 made a style a Rust module: a `StyleDescriptor` plus a
`StylePass`, compiled into the engine and listed in `register_styles!`.
Six styles have been written that way (Kuwahara, watercolor, ASCII,
pencil, invert, mask_view). A game that wants a look of its own has to
fork the engine and wait for a release build. Tuning made this worse: the
watercolor session found each shader-only tweak cost a 3-7 minute release
rebuild, because the WGSL is `include_str!`'d. It added a temporary
env-var override to read the shader from disk, then removed it.

The request (epic #260) named "full custom shader support" as a
direction. This spike settles what a user-authored style looks like now
that six built-ins show what styles actually need:

1. **Format.** What file does a game write, where does it live, what
   does it declare?
2. **Inputs.** Which buffers can a user shader read, and how does it
   bind them?
3. **Params.** How do user parameters reach the GPU, the F4 menu, scene
   TOML and `--style`?
4. **Failure.** What happens when the WGSL doesn't compile, compiles but
   the GPU refuses it, loops forever, or is edited into a broken state
   while the game runs? What changes on the web and in a bundle?

What the built-ins read, surveyed for this ADR:

| style | stage | colour | depth | mask | own targets | passes | frame history |
|---|---|---|---|---|---|---|---|
| invert | display | ✓ | | | | 1 | |
| mask_view | display | ✓ | | ✓ | | 1 | |
| kuwahara | hdr | ✓ | | | 2 (tensor, blur) | 3 | |
| watercolor | display | ✓ | ✓ | ✓ | 1 (wash) | 2 | |
| pencil | display | ✓ | ✓ | ✓ | 1 (prep) | 2 | |
| ascii | display | ✓ | ✓ | ✓ | 3 (cells, edges, history) | 4 | ✓ |

Every style reads colour. Half read depth and the mask. All of the
expensive ones have more than one pass. Only ASCII keeps history.

## Decision

### 1. Format: `styles/<name>.shader.toml` naming a WGSL file beside it

```toml
# styles/risograph.shader.toml
version = 1
name = "risograph"          # = the file stem; lowercase, digits, _; not a built-in's
label = "Risograph"         # F4 and --gpu-time (default: the name made readable)
stage = "display"           # or "hdr" (default display)
shader = "risograph.wgsl"   # a file beside this one; no paths out
inputs = ["depth", "mask"]  # optional; colour is always there

[params.dot]                # one f32 each; ordered by name in the uniform
default = 4.0
min = 2.0
max = 16.0
label = "Dot pitch (pt)"    # optional, as StyleHint
help = "Distance between halftone dots"
integer = false
```

- **Not `*.style.toml`.** The issue suggested that name, but it is taken
  twice: AI style guides (`styles/medieval_tavern.style.toml`,
  `flint-asset-gen/src/style.rs`) and the UI class files of
  `flint-script/src/ui/loader.rs`, which hot-reloads them. A third
  meaning would make `styles/*.style.toml` ambiguous to every loader
  and every agent reading the tree. `.shader.toml` says what it is.
- **In `styles/`,** beside the `*.look.toml` presets (#279) that will
  name these styles. Found from `<scene_dir>/styles/` and then
  `<scene_dir>/../styles/` (the model search order), through
  `flint_core::vfs`, so a bundle and the web read them the same way.
- **`version = 1`,** checked. Unknown keys are errors
  (`deny_unknown_fields`), so a typo in a key is reported instead of
  being silently ignored.
- **A user style is a table like any other.** `[post_process.styles.risograph]`
  turns it on, its keys are its params, `--style risograph:dot=6` and
  `--sweep risograph.dot=3,4,6` work, F4 draws its sliders, and
  `<base>_r/_g/_b` params fold into a colour picker. All of this comes
  from the descriptor, with no code per style.
- **No string options in v1.** Options exist for fonts and ramps, which
  a WGSL function can't use without textures (see Follow-ups).

### 2. The shader: one function, the framework writes the rest

The user writes:

```wgsl
fn style(p: StylePixel) -> vec4<f32> {
    return vec4<f32>(vec3<f32>(flint_luma(p.color.rgb)), 1.0);
}
```

The framework composes the module wgpu sees: a generated **prelude**,
then the user's source, then generated **entry points** (`vs_main`, a
full-screen triangle, and `fs_main`, which builds a `StylePixel` and calls
`style`).

| binding | what | when |
|---|---|---|
| `@group(0) @binding(0) flint: FlintFrame` | `inv_view_proj`, `resolution`, `texel`, `depth_size`, `time`, `frame`, `near`, `far`, `srgb`, `pixels_per_point`, `camera_pos`, `mask_id` | always |
| `@group(0) @binding(1) params: Params` | one `f32` field per declared param, by name, then a `vec4` pad | always |
| `@group(1) flint_input`, `flint_sampler` | the stage's input and the chain's linear sampler | always |
| `@group(2) @binding(0) flint_depth_texture` | the scene's single-sample depth | `inputs` has `depth` |
| `@group(2) @binding(1) flint_mask_texture` | the entity style mask (id + blend) | `inputs` has `mask` |
| `@group(2) @binding(2) flint_edges_texture` | the shared geometry edges (#301, added after this ADR): silhouette, crease, direction; read with `flint_edges(uv)` / `flint_edge(uv)` | `inputs` has `edges` |

Helpers: `flint_sample(uv)`, `flint_luma`, `flint_hash`,
`flint_to_display` / `flint_to_linear`; with depth `flint_depth(uv)`,
`flint_view_depth(uv)` (metres) and `flint_world_pos(uv)`; with the mask
`flint_mask_id(uv)` and `flint_style_blend(uv)` (0 off this style's
entities, their blend from #278 on them).

Why this shape:

- **Bindings are the framework's, not the user's.** The pass makes the
  bind groups, so it has to know the layout. Reflecting a user's
  arbitrary `@group/@binding` declarations would mean matching them to
  resources by name. The prelude fixes the layout. User code that
  declares a binding or an entry point is refused.
- **Colour space is handled once.** At the display stage, on an sRGB
  surface, a style samples linear and its writes are encoded. Every
  built-in display style decodes and encodes by hand (the gotcha from
  #262). The generated `fs_main` gives the user display values in and
  takes display values out, whatever the target format. At the hdr stage
  they get linear HDR.
- **The cost is one fragment pass.** No own targets, no multi-pass, no
  history (see Follow-ups). Four of the six built-ins need more, so a
  user style of v1 is an invert/mask_view/contour-class effect, not an
  ASCII. That's deliberate: one pass is the shape whose failure modes
  can be made safe here.
- **Max 4 bind groups (wgpu 23)** leaves group 3 free for v2 (textures
  of the style's own).

### 3. Params

Each `[params.<name>]` becomes a `StyleParam` (and a `StyleHint` when it
has a label, help or `integer`). The descriptor's strings and slices are
leaked once at load, so a user style is a `&'static StyleDescriptor` like
a built-in. The registry becomes an append-only snapshot, the built-ins
followed by user styles in load order. `StyleSet` keys built passes by
registry index, which therefore never changes. Values are clamped to
`min..max` before upload. The `Params` uniform is
`ceil(n/4)*16 + 16` bytes, at most 64 params (272 bytes).

Hosts call `stylize::custom::load_dir(scene_dir)` **before** the scene's
`[post_process]` is read. Otherwise its table names an unknown style and
its params are dropped with a warning. `flint render` does this in the
prototype; #275 wires the player and viewer.

### 4. Failure behaviour

| what | when found | what happens |
|---|---|---|
| bad `.shader.toml` (unknown key, version, range, name ≠ stem, built-in name, `..` path) | load | not registered; one message names the file and key |
| WGSL doesn't parse or validate | load and every rebuild, by **naga** (native) | registered anyway, draws a **passthrough** (its input unchanged); the error is `file:line:col: message` plus the offending line, in the user's file, with a hint if they called a helper of an undeclared input |
| no `fn style`, a binding, an entry point of the user's own | naga IR check | the same as above |
| the backend's own compiler refuses what naga passed | `push_error_scope` around the pipeline (blocking natively) | the same as above |
| a later edit breaks it (hot reload) | rebuild | the **last good build keeps drawing**; the error replaces the old one; fixing it logs "compiles again" |
| a loop that never ends | **not detectable** | warned at check: "N loops: a loop that never ends hangs the GPU until the driver resets it (TDR)"; >32 texture reads written out are warned too |
| the shader crashes the driver | `catch_unwind` around `create` (ADR 0111) | the style stays off; the renderer reports a lost device |

`stylize::custom::errors()` lists each style's current error for the host
to show. The prototype logs; #275 draws it on screen.

**Passthrough, not "off".** A style that is switched off changes which
styles chain and what the scene looks like for reasons the user can't see.
A passthrough keeps the chain's shape and the scene's other styles, and
the frame is visibly "my style isn't there", with the reason in the log.
The passthrough is the same prelude with `return p.color`, so it shares
the layout and always compiles.

**Infinite loops.** WGSL has no bounded-loop rule, and a fragment loop
that never ends is a GPU hang. Windows resets the driver after 2 s (TDR).
Browsers lose the device. Static analysis can't decide termination. We
considered three mitigations:

- *Reject loops.* This also rejects every blur and every kernel, the most
  common thing a style does.
- *Inject a guard counter into every naga `Loop`* and write the module
  back out with naga's WGSL backend. This is sound and invisible to the
  user, and it costs one compare a loop iteration. It needs naga's
  `wgsl-out` and an IR rewrite. Filed as a follow-up, recommended before
  user styles ship in a bundle someone else wrote.
- *Warn* (chosen for v1). User styles today are written by the game's own
  author, who sees the warning on the first render.

### 5. Hot reload

The pass polls the WGSL file's modified time every 15 frames (about four
times a second at 60 fps, one `stat` each), natively only. On a change
it re-reads through the vfs, re-checks and rebuilds the pipeline. Bind
groups are unchanged, because the layout is fixed by the description.
The `.shader.toml` is not reloaded in the prototype: params are part of
the leaked descriptor. #275 replaces the descriptor and re-parses the
scene's `[post_process]` (see "Built in #275" below).

### 6. Web and bundles

- **naga stays out of the web build.** It is a native-only dependency of
  flint-render (wgpu already builds it natively, so this adds nothing
  there). On wasm the browser's WGSL compiler is the check: errors arrive
  through wgpu's error scope, which can't be awaited inside a frame, so
  the browser logs them. The style still falls back, because the pipeline
  is invalid, but `errors()` doesn't learn why. #275 can resolve the
  scope's future on the next frame to report it. *Built in #415, below.*
- **No hot reload on the web.** There is no file to poll; a bundle is
  fixed.
- **Bundles already carry them.** `flint bundle` packs `.toml` and
  `.wgsl` files that aren't excluded, and every read goes through the
  vfs.

## Alternatives considered

- **Full user modules (the user writes the bindings and entry points;
  Flint reflects them with naga).** Maximal freedom, but the pass would
  have to match bindings to resources by name and type, and every new
  input would become a new naming convention. Errors in the user's own
  boilerplate are the commonest failure in shader playgrounds
  (ShaderPlayground, 2026-10-01). The one-function contract removes the
  boilerplate, and v2 can add `entry = "module"` if a game needs it.
- **A TOML-described pass graph (several passes, own targets).** This is
  what four built-ins need, and it would let a game write a watercolor.
  But it is the riskiest surface: targets cost memory per resolution and
  a graph needs ordering and cycle checks. It should be designed from a
  game asking for one. Filed as a follow-up.
- **Rust styles behind a `PlayerExtension` hook.** Games already embed
  `PlayerApp` (ADRs 0071-0075), so a game could register a Rust style.
  That covers the power user, but not `flint render`, the viewer, the web
  page's shared player or agents iterating on a look. It is worth doing
  separately (a public `register_style`), and it is not a substitute.
- **Bind user params as an array (`params.v[3]`).** This is simpler to
  generate, but unreadable in the shader, and every rename would shift
  indices. Named fields cost one generated struct.

## Consequences

- A game gets a new look by writing two small files. The first renders
  of three demo styles (risograph, contour, hologram) needed no engine
  change beyond the loader.
- `registry()` is now a snapshot behind an `RwLock` (one read lock a
  call). It is called per frame in a few places. Not measured on its own:
  an uncontended read lock is a few nanoseconds, and a frame could take
  one snapshot if it ever shows up in `--perf`.
- `--style` and `--sweep` no longer refuse an unknown style name at
  argument parsing, since a user style is registered only once the
  scene's folder is read. `flint render` checks each `--style` right after
  `load_dir` (`StyleArgs::check`). A bad name is still an error, just a
  moment later.
- Each loaded style leaks its descriptor (a few hundred bytes), once per
  file per run. #275's TOML reload leaks one more per edit, and an
  editor session can afford that.
- The WGSL contract (`StylePixel`, the helper names, `flint.*` fields) is
  now a public API. Additions are compatible. Renames need `version = 2`.

## Measured

`flint render demo/user_styles.scene.toml`, RTX 4070, 1280x720, release,
`--gpu-time 240` (the renders are in the PR and the presentation):

| style | inputs | GPU time |
|---|---|---|
| risograph | colour (two reads a pixel) | 0.025 ms |
| contour | depth (several depth loads, `fwidth`) | 0.018 ms |
| hologram | mask (five mask loads) | 0.012 ms |

- **The fallback is exact.** With `contour.wgsl` broken on purpose (a mask
  helper called without `"mask"` in `inputs`), the render is
  byte-identical to the scene without the style: the passthrough's sRGB
  decode/encode round trip loses nothing at 8 bits. The error read
  `demo\styles\contour.wgsl:21:16: no definition in scope for identifier:
  'flint_style_blend'`, with the line quoted and the hint "add "mask" to
  inputs".
- **Hot reload** is covered by a GPU test
  (`hot_reload_keeps_the_last_good_build`): good build, broken edit
  (reported, last good pipeline kept), fix (error cleared), each seen
  within `POLL_FRAMES`.
- **Iterating on a look without a rebuild:** tuning `risograph.wgsl` took
  three edits. Each one was a re-render of a few seconds, against the 3-7
  minute rebuild a built-in style costs (#272).
- naga's check time wasn't measured separately: it runs once a load or
  edit, not once a frame.

## Built in #275

#275 turned the prototype into the production loader. It kept the format
and contract above and changed five things:

- **Hosts.** `flint-player` (scene load and every transition) and
  `flint edit` call `load_dir` before the scene's `[post_process]` is
  read. Both draw `custom::errors()` as a card in the bottom-left corner
  (`flint_debug_ui::paint_style_errors`). F4 tags a user style's header
  `user` (hover: both files) and `error` (hover: the message).
- **`.shader.toml` reload.** `custom::reload_changed(scene_dir)` polls each
  description's modified time natively. Hosts call it about every 15
  frames. A changed file is read again and its new descriptor goes in at
  the old one's registry index (`replace_runtime`). `StyleSet` remembers
  the descriptor each slot was built from and builds again when it
  differs, and `active()` never pairs a new descriptor with a stale pass.
  The host then re-reads the scene's tables for that style
  (`refresh_settings`). The player re-parses its `PostProcessDef`, and the
  viewer reloads the scene. A new file in a style folder is picked up the
  same way. A file that fails is retried only once its modified time moves
  again.
- **Bind names carry a generation.** Found running the player, not by the
  unit tests: the chain's bind cache keys groups by name, so a replaced
  style bound the *old* pass's uniform buffers. They were frozen at the
  old values, and a new param count would have bound a buffer of the
  wrong size. Each install now names its groups `<style> ... #<n>`.
- **A user style that panics building is not a lost device.** The
  renderer switches it off and records the panic as its error. Only a
  built-in's panic still marks the device lost (ADR 0111).
- **`flint bundle`** loads every user style each scene sees from the
  packed files, through the bundle's vfs, as the web player will. It
  refuses the bundle if a description doesn't read, a shader isn't packed
  or a shader doesn't compile. `.shader.toml` files are also scanned for
  file references, like scenes and glyph sets. Check warnings start
  `<file>: warning:` and don't refuse a bundle.

A game folder outside the engine (`levels/garden.scene.toml`,
`styles/cyanotype.*`, the engine's schemas only) rendered in `flint
render`, played in `flint-player`, hot-reloaded both files mid-run, and
bundled and played from the bundle with `flint-player --bundle`, the same
`MemorySource` path the browser takes. The browser itself wasn't run.

### Built in #415: the browser's errors

- **naga still stays out.** wgpu builds naga's WGSL front end for wasm
  (`wgpu-core` with `wgsl` is in the tree), but on wasm only the `webgl`
  backend calls it, and the web build has WebGPU alone, so the linker
  drops it. Running `check` in the browser would put the front end and
  the validator into the download.
- **A build waits for the GPU's answer.** `rebuild` creates the pipeline
  as *pending* and keeps two futures: `ShaderModule::get_compilation_info()`
  (the compiler's messages, with lines in the composed module) and the
  validation error scope's `pop_error_scope()` (the pipeline against its
  layout too). Neither is awaited: the pass polls both with a no-op waker
  each frame (`Settling`) and only then trusts the pipeline. Natively both
  are ready on the first poll, inside `rebuild`, so native and web share
  one path and the native tests drive the web's (a future that answers on
  a later poll). An invalid pipeline in a WebGPU pass invalidates the
  frame's command buffer, so until the answer the last good build (or the
  passthrough) draws.
- **Lines in the user's file.** `compiler_error` takes the first
  `Error` message and maps its line through `Composed::user_line`, by the
  same `locate` naga's errors use: `file:line:col: message`, the line
  quoted, `(and N more)` after it. The message lands in the style's error
  slot, so `errors()`, the player's card and the console show it.
- **What reaches the browser.** `flint bundle` refuses what naga refuses,
  so a bundle carries only styles the browser is stricter about. The one
  found here is derivative uniformity: naga doesn't check it in fragment
  shaders, while Tint refuses `fwidth`/`dpdx`/`dpdy` under a branch on a
  pixel's value (`flint_sample` is `textureSampleLevel`, which has no
  such rule). In Chrome a style calling `fwidth` there drew its input,
  and about 0.8 s after the first frame the card and the console read
  `styles/frost.wgsl:12:42: 'fwidth' must only be called from uniform
  control flow`, with the line quoted.

## Follow-ups

Filed from this spike (numbers in the PR): the loop guard (naga IR
rewrite), own textures and a second pass (`[[passes]]`), string options
and user images (an `[images]` table: LUTs, paper, glyph sheets),
per-entity user styles (a generated component, as `ascii_style`), web
error reporting, a template generator (`flint style new <name>`), and
user styles in the goldens (`render-suite` cases from `demo/styles/`).
