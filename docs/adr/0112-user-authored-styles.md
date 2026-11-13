# ADR 0112: User-authored styles — one WGSL function beside a `.shader.toml`

*Accepted 2026-11-07. Spike #274 for epic #260; the production loader is #275.
Builds on [ADR 0111](0111-stylized-rendering.md). Book:
[Stylize Passes → User-authored styles](../book/src/concepts/stylize-passes.md#user-authored-styles).
Prototype: `crates/flint-render/src/stylize/custom.rs`, demos in `demo/styles/`.*

## Context

[ADR 0111](0111-stylized-rendering.md) made a style a Rust module: a `StyleDescriptor` plus a
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
  a WGSL function can't use without textures (see Follow-ups). *#413
  added both, still `version = 1` since the additions are compatible:
  see "Built in #413" below.*

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
| a loop that never ends | **not detectable**, so every loop is **guarded** (#411, natively) | each user loop stops after `LOOP_LIMIT` (4096) turns a run; its pixel draws whatever it had reached, and the frame finishes. A `loop {}` style renders in `flint render` in 0.9 ms of GPU at 960x540. On the web (no naga) the loop runs as written, unguarded |
| many texture reads | check | >32 reads written out are warned |
| the shader crashes the driver | `catch_unwind` around `create` ([ADR 0111](0111-stylized-rendering.md)) | the style stays off; the renderer reports a lost device |

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
  user, and it costs one add and one compare a loop iteration. It needs
  naga's `wgsl-out` and an IR rewrite. **Chosen in #411** (natively).
- *Warn* (v1, #274). User styles were written by the game's own author,
  who saw the warning on the first render. It is still what a check says
  when the guard is off (`custom::set_loop_guard(false)`, for tests).

**The guard (#411).** After naga parses and validates the composed module,
`custom::check` walks every `Statement::Loop` in the user's functions (the
prelude's and the entry points are left alone). Each loop gets a `u32`
local of its own, stored 0 just before the loop (so a nested loop's count
starts again each time its outer loop turns), incremented at the end of
`continuing`, and `break if` that reaches `LOOP_LIMIT` (4096), or-ed with
the loop's own `break if` when it has one. `continue` goes through
`continuing`, so every turn counts. The module is validated again and
written out with `naga::back::wgsl`, and wgpu builds that text
(`Report::guarded`); a style without loops builds its composed text as
before. What it costs and shows:

- A loop that ends on its own within the limit runs exactly as written: a
  9x9 blur draws byte for byte the same guarded as not
  (`a_bounded_loop_draws_the_same_guarded`, in `tests/stylize.rs`), which
  also shows naga's written-out module draws what the composed text did.
- A `loop {}` style draws a frame (`a_loop_that_never_ends_draws_a_frame`,
  37 ms built and drawn, and `flint render` of such a scene: 0.9 ms of GPU
  at 960x540, the whole command 1.2 s). Before, it hung the GPU until TDR.
- The limit is per run of a loop, so nested loops multiply (a 64x64
  kernel is fine). A loop that needs more than 4096 turns a pixel is far
  past what a full-screen pass can afford anyway.
- **A loop the IR proves short is left alone (#424).** Before the
  rewrite, `custom/bounded.rs` looks for a counter in each loop: started
  at a constant (a store just before the loop, or the variable's own init
  when nothing else writes it), compared with a constant (a literal or a
  module `const`) by a `for`'s leading `if .. else { break }` or by a
  `break if`, stepped by a constant in `continuing`, and written nowhere
  else (no store in the body, no pointer handed to a call). It runs the
  counter forward on the CPU in its own type, wrapping as the GPU does; a
  loop that ends within `LOOP_LIMIT` turns could never trip the guard, so
  it isn't guarded (`Report::bounded`), and a style whose loops are all
  like that builds its composed text with no rewrite. Octaves and kernel
  taps, the commonest loops, are such loops. A bound read from a
  parameter stays guarded: params are clamped to their range where they
  are written, but a guard that trusts every writer is weaker than one
  that trusts none. Measured at 1080p on the 4070 (`flint render
  --gpu-time 240`, main and the branch alternated five times, minimums),
  with pixels byte-identical: an 11x11 constant kernel 0.946-0.951 ms
  guarded vs 0.881-0.884 ms as written (-7%), three octaves of noise 0.071
  vs 0.069-0.070 ms, and `radial_blur` (guarded in both) 0.194-0.195 ms
  in both. With the guard off, only the loops not proven are warned about.
- Errors still point into the user's file: they come from the first
  parse, before the rewrite. The guard's own failure (re-validation or
  writing) is reported as a Flint bug and the style draws a passthrough;
  the unguarded text is never built.

**On the web the guard is not run.** The player has no naga on wasm (§6):
shipping `wgsl-in`, the validator and `wgsl-out` would put most of naga
into the web build against the 4 MB gzip line (#123), to guard styles that
`flint bundle` has already checked natively. The cheaper road is for the
bundle to carry the guarded module, written at pack time, so the browser
compiles that with no naga at all (#550). Until then a
runaway loop in a bundle loses the device in the browser, as before (not
tried here).

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
  `PlayerApp` (ADRs [0071](0071-player-extensions-in-a-game-binary.md)-[0075](0075-screen-to-ground-picking.md)), so a game could register a Rust style.
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
  built-in's panic still marks the device lost ([ADR 0111](0111-stylized-rendering.md)).
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

## Built in #413: images and string options

- **`[images.<name>] file = "..."`** binds a PNG or JPEG beside the
  description in group 3 (the slot section 2 left free), 8 bits a
  channel, one mip, `Rgba8Unorm` (stored values) or with `srgb = true`
  `Rgba8UnormSrgb`. Up to 8 a style. The helpers are
  `flint_image_<name>(uv)`, `flint_image_tiled_<name>(uv)` (manual
  bilinear over `textureLoad`, so it wraps without a repeat sampler),
  `flint_image_size_<name>()` and `flint_lut_<name>(rgb)` (a strip LUT,
  trilinear from two bilinear reads).
- **No new sampler.** The images are read with group 1's
  `flint_sampler`, so group 3 holds textures only and adds nothing to the
  DX12 sampler heap. Its bind group is the pass's own, made again only
  when a view changes.
- **`[options.<name>]`**, kind `choice` (default) or `image`. Options sit
  in `Params` after the floats, one `u32` each (the index into the
  choices), and a choice that is a word gets `FLINT_<OPTION>_<CHOICE>`.
  An image option is also an image binding. With no `choices` it lists
  the images in its default's folder **once, at load**, and leaks them as
  a fixed list. That makes every user option closed, so the #318 warning
  and `--style`'s refusal apply with no new code. The cost: an image added
  to that folder appears after the `.shader.toml` is next saved (or the
  next run), not on its own.
- **Failure.** Every file an image may read is checked to exist at load
  (`flint bundle` refuses a missing one). Images decode on the first draw,
  through the vfs. Until each has read once the style is a passthrough
  with its error; afterwards a file that fails keeps the last good
  picture. Image errors are kept apart from the shader's, since a good
  shader build would clear them otherwise.
- **Reload.** Every 15 frames, natively, the pass stats each image's file
  and decodes one whose modified time moved. A new choice from the scene
  or F4 loads on the frame it's made.
- **Per-entity rows (#414).** A mask-reading style's options sit in every
  row of the settings table, so an entity's `<name>_style` can set a
  choice option of its own. An image option is one binding for the
  frame: it shows the scene's file, and an entity's value changes only
  its index in `params`.

## Built in #412: passes and targets

The "TOML-described pass graph" this ADR set aside is built, kept as a
list, not a graph. Still format `version = 1`: a description with a
top-level `shader` reads as before.

- **`[[passes]]` in order, not sorted.** Each pass names its WGSL (a
  `fn style` of its own), the `target` it writes (`format` rgba16f /
  rgba8 / r32f, `scale` 1/16..1 of the stage), and the targets it `reads`.
  Every pass but the last writes a target; the last writes the output.
  Running passes in the order written keeps the file the explanation: a
  topological sort would accept a file whose order says something else.
- **Checked at load, naming the passes.** A read before its writer is an
  error that says which pass to move; when the writer depends back on the
  reader it is reported as a cycle with the chain (`"a" -> "b" -> "a"`).
  One writer a target, no target on the last pass, a target nobody reads
  is a warning.
- **Targets in group 3, after the images** (#413 put a style's images
  there first, at bindings 0..n). A pass that reads targets gets a group 3
  layout of its own: the images, then its reads at binding `n + i` in
  `reads` order, so neither feature moves the other's bindings. A pass
  that reads none binds the images' own group. Its bind group is keyed by
  each read's size, which half of a history pair it is this frame, and the
  images' generation (bumped when an image is decoded again). With 8
  images and 4 targets, plus the input and the three optional buffers, a
  pass binds at most 16 sampled textures, every backend's limit. Helpers:
  `flint_target_<name>(uv)` / `_load(texel)` / `_size()`. `r32f` reads
  nearest, since `R32Float` isn't filterable without a feature most web
  GPUs lack. Group 2 gets an empty layout when a pass reads targets or
  images but no depth, mask or edges (wgpu numbers groups by place). A
  target holds what its pass returns, with no sRGB encode; only the last
  pass's result is encoded.
- **Per pass, on #415's path.** Each pass has its own `Build` (a pending
  pipeline settled from the compiler's report and the error scope), its
  own frame uniform and its own error; the style's error joins them. The
  settings table (#414), the images and the loop guard (#411) are the
  style's, shared by every pass. `check`, `compose` and `compiler_error`
  mean the last pass (the only one, for a style of one pass); the
  `_pass` variants take a pass index.
- **Targets come from `StyleFrame::target_sized`** as `<style>.<target>`,
  so they live with the chain, are rebuilt on resize and show up in
  `StyleTargets::entries()`, which `flint render --gpu-time` lists with
  size, format and bytes. Not keyed by install generation: the same name,
  format and size is the same texture after a reload.
- **Caps:** 8 passes, 4 targets, and 32 bytes a stage pixel in all (four
  full-size rgba16f; 66 MB at 1080p), a history target counting twice.
- **History** (`history = true`): a pair swapped each frame. Reads at or
  before the writer get last frame's, later ones this frame's. Trusted only
  on the frame after, in the same `history_epoch`, into the same textures
  (held, so their addresses can't be reused); otherwise the past is cleared
  to transparent black and `flint.history` is 0.
- **Failure:** each pass is checked and built alone and keeps its last good
  build; until every pass has built once the style draws the passthrough.
  `FlintFrame` grew `pass_size`, `history` and `pass_index` (144 bytes, one
  buffer a pass, since a buffer holds one value a frame).

Measured: `demo/styles/orton.*` (expose, blur across, blur down at half
size, screen over the frame) costs 0.032 ms for all four passes at
1280x720 on an RTX 4070, with 5.3 MB of targets.

## Addendum (#426): which of two styles of one name wins

**Decided: option 2, scope by folder.** A scene draws the user style
nearest to it, and only one it can see. The registry stays one for the
process (an append-only snapshot, indices fixed, as in decision 3); what
is scoped is which of its entries a scene's tables reach.

The question, from #275: `load_dir` searched `<scene>/styles/` and the
folder above, but filled a registry that is global for the run. So the
first scene to load a `glow` decided it for every later scene, the
player warned "already loaded" again at every transition, and a scene
could draw a style that only an earlier scene's folder had. Three
options were on the ticket: keep first-wins and warn once a pair, scope
by folder, or refuse duplicates.

- **The nearest file wins a name** (built in #781, ADR 0114 step 3).
  `load_for(&scope)` registers `scope.list(AssetKind::Styles)`, the
  project's one asset search, nearest of a name first. A file whose name
  is loaded from another file takes over at the same registry index, as
  a hot reload does (`install(.., replace = true)`), with an `info!`, not
  a warning: a transition between two folders is not a mistake, and
  `flint bundle` must not refuse it. Loading the same file again does
  nothing, so nothing repeats on a transition. In one `styles/` folder a
  style is its file's name, so there is only one of a name.
- **A style the scene can't see is not drawn** (#426).
  `custom::drop_unseen(&mut tables, &scope)` takes out of a scene's
  `[post_process.styles]` each table naming a registered user style
  whose file is in none of the scope's `style_dirs`, with a warning
  naming the file. The player calls it each time it reads the scene's
  tables into a config (`scene_post_config`), on a copy, so the scene's
  own tables keep it: a file the scene can see that appears later takes
  the name over at the next reload poll, and the table is drawn.
  `flint check` calls it per scene, so a folder's report doesn't depend
  on the order it meets the scenes in. It reads no folder (a path compare
  against `search_dirs`), so a reload can call it on the frame thread.
- **Not chosen.** First-wins with one warning a pair keeps a scene's look
  dependent on what was played before it. Refusing duplicates would make
  two folders' `glow` an error although each scene sees only one of
  them, and would break a game that keeps a style per level folder.
  Per-scene registries, or names qualified by folder, would change the
  registry's indices and the `StyleSet` keyed on them for a case no game
  has yet.

Proof (#426's PR): a project of two folders, `atelier/` with
`styles/risograph.*` and `cellar/` naming `risograph` with no styles
folder. Played atelier then cellar in `flint-player --frame-step`, the
cellar's frame before this change was drawn in the atelier's risograph;
after it, it is byte-identical to `flint render` of the cellar alone,
with the warning. Copying the style into `cellar/styles/` while the
cellar played drew it there at the next poll. `flint check` of the
project reported nothing before and the cellar's warning after.

Left as it was: the viewer and `flint render` show one scene a process,
and `flint render --list-styles` and `flint style list` list every
registered style, which for one scene is the ones it sees.

## Follow-ups

Filed from this spike (numbers in the PR): the loop guard (naga IR
rewrite), own textures and a second pass (`[[passes]]`), string options
and user images (an `[images]` table: LUTs, paper, glyph sheets),
per-entity user styles (a generated component, as `ascii_style`), web
error reporting, a template generator (`flint style new <name>`), and
user styles in the goldens (`render-suite` cases from `demo/styles/`).
