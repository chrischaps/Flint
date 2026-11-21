# ADR 0126: What a frame capture holds, and how the debugger replays it

Status: accepted, 2026-11-16 (#1082, part of epic #1081).

## Context

The frame debugger epic (#1081) has three tickets that write or read a frame
capture (#1084 the frame's description, #1085 each pass's image, #1086 the
player's trigger), two that read it (#1087 `flint frame`, #1088 its window)
and one that draws the frame again from it (#1089, replay). What the capture
holds decides what the debugger can ever do:

- **Results only** (the pass list, draws, targets, timings and the image each
  pass produced) is simple and always opens, on any machine and any later
  engine, but can't step through a pass draw by draw or answer "what if this
  entity weren't there".
- **Results plus replay inputs** can, but needs the world, the assets and
  every per-frame input, and has to stay loadable as the engine moves.

The question the spike had to answer was whether a fresh renderer, handed
what a capture could hold, draws the captured frame byte for byte, and if
not, which state stops it. ADR 0118's host protocol
(`crates/flint-render/CLAUDE.md`, "Host protocol": `SceneBinding`,
`bind_scene`, `prepare(&FrameInputs)`, `encode_to`) suggested it would,
since every per-frame input reaches the renderer through one struct. Styles
with history, motion holds, the boil and grass compute all carry state
between frames, and were the risk.

The spike is the throwaway branch `spike/frame-capture-1082` (never merged).
`FLINT_SPIKE_CAPTURE=<dir> flint render <scene> --time T` reads back every
target the frame keeps and writes them, snapshots the world, then loads a
second `Stage` (the `flint render` host that steps the player's game core,
#725) and draws one frame with the captured inputs into the same target and
compares the pixels. Environment knobs switch each piece of captured state
off, so each row of the table below is one piece's effect.

## Decision

### 1. A capture is results first, replay inputs second, in one folder

Everything the static views need (#1087, #1088) is in the manifest and the
images, and never needs a GPU, the scene, the schemas or the engine revision
that wrote it. Replay inputs sit in their own subfolder; a capture without
them, or whose inputs a newer engine can't use, still opens in every static
view. Replay (#1089) is an extra layer on top.

### 2. Layout: a folder, not zipped

```
tavern_hub-20261116-223000-f1234.flintframe/
  manifest.json                    written last: present means complete
  targets/
    03-hdr_scene.exr               a float target, its values (half, ZIP)
    05-ssao.png                    an 8-bit target, exact
    12-shadow_cascade_0.exr        a depth target (f32, ZIP)
    ...                            NN = the pass order that wrote it
  replay/                          absent when replay inputs weren't captured
    world.toml                     every entity by name: parent, archetype, components
    inputs.json                    FrameInputs, camera, drawn config, renderer-held state
    skinning.bin                   bone palettes: entity, shadow and echo, by entity name
    particles.bin                  the packed ParticleInstance stream, per emitter
```

- **Each target is stored once, exactly**: 8-bit targets as PNG, float
  targets as OpenEXR half, depth as OpenEXR f32, both ZIP-compressed (the
  native `image` build has `exr`; the web build doesn't and doesn't capture,
  #1086). A viewable 8-bit copy (HDR tonemapped at the frame's exposure,
  depth linearized to grey) is made by the tool when asked
  (`flint frame image`), not stored beside it. The pixel readout #1088 wants
  needs the real values, and two copies double the size.
- **Not zipped.** An agent reads a PNG out of the folder directly; the writer
  streams files as their read-backs land, off the frame thread; `manifest.json`
  written last makes an interrupted capture obvious; and per-file compression
  already gets what there is to get. A zip would add a deflate pass over
  already-compressed PNGs. Sharing one is zipping the folder by hand, or a
  `flint frame pack` later if that proves common.
- Captures go under the project's ignored output folder by default (#1086's
  risk table): they hold renders of a game's content.

### 3. The manifest: JSON with an integer version

`serde_json` (a workspace dependency) over types in a new
`flint_render::frame_capture` module, which the writers (the renderer's
description, #1084; the player, #1086) and the reader (`flint frame`) share.
The top level:

```json
{
  "format": "flintframe",
  "version": 1,
  "engine": { "version": "0.1.0", "commit": "d20a9f7f", "dirty": false },
  "written_by": "flint-player",
  "captured_at": "2026-11-16T22:30:00Z",
  "scene": { "path": "levels/tavern_hub.scene.toml", "name": "Tavern Hub", "sha256": "..." },
  "frame": { "index": 1234, "game_time": 2.0, "dt": 0.016667,
             "size": [1920, 1080], "scene_size": [1920, 1080], "render_scale": 1.0, "msaa": 1,
             "adapter": "NVIDIA GeForce RTX 5070 Laptop GPU", "backend": "Vulkan",
             "present_mode": "Fifo", "capturing": true },
  "stats": { },
  "targets": [ { "id": "hdr_scene", "format": "Rgba16Float", "size": [1920, 1080],
                 "samples": 1, "bytes": 16588800, "file": "targets/03-hdr_scene.exr" } ],
  "passes": [ { "name": "Scene", "ran": true, "skipped": null, "gpu_ms": 1.31,
                "reads": ["shadow_map"], "writes": ["hdr_scene"],
                "draws": [ { "entity": "bartender", "entity_id": 7, "kind": "Skinned" } ] } ],
  "cpu": { "phases_ms": { }, "notes": [], "hitch": null },
  "replay": { "available": true, "fresh_only": ["ascii: temporal history"],
              "assets": [ { "path": "models/tavern.glb", "sha256": "...", "bytes": 1234567 } ] }
}
```

- **`version` is an integer**, 1 for the first. A field added is not a bump:
  readers ignore what they don't know (no `deny_unknown_fields`) and default
  what's missing. Removing a field, renaming one, or changing what one means
  is a bump. A reader refuses a version newer than its own with a message
  naming both, and keeps reading every older one (#1087 freezes a version-1
  manifest as its fixture).
- **`engine.commit`** is what replay checks: a capture from another commit
  replays anyway, says so, and compares its result with the captured image,
  which is the arbiter (decision 6).
- **Entities are named by name**, with the id beside it for display. Ids
  are the names' ranks at load (#688), and the spike measured them moving
  between the captured world and a replay of it (below), so nothing in a
  capture is keyed by id.
- Passes, draws, targets and stats are #1084's `FrameDescription`; `cpu` is
  the player's frame history (#1086), with the hitch line and its window
  when a hitch triggered the capture.

### 4. Replay inputs: the authored scene, the world over it by name, and what the renderer holds

What draws a frame again, as measured:

- **The scene is loaded from its authored file** (path in the project, plus
  its SHA-256), through the normal host load: `SceneBinding`, `bind_scene`,
  the model loader, splines and the procgen resolve. **Not the world saved as
  a scene**: the model loader splits a multi-node GLB into child entities
  whose `model.asset` names a sub-mesh the renderer registered at load
  (`wooden_bench/wooden_bench`), and living props draw meshes the resolver
  made, so a world saved after load isn't a scene that loads (the grove drew
  no trees, no bench and no lantern).
- **The world as it stood is laid over that load by entity name**
  (`replay/world.toml`: each entity's parent, archetype and components in a
  scene's encoding): every captured component is set, an entity the load
  doesn't have is spawned, one the capture doesn't have is despawned.
- **The replay host runs no game step**: no scripts, physics, animation,
  particle simulation or living subsystem. The spike's first replay ran the
  game's first step at load, which rewrote the torch lights' flicker and the
  essences' bob (68,691 pixels off); laying the captured world back over it
  made it exact.
- **What the renderer holds that the world doesn't** goes in
  `inputs.json` and the blobs, by entity name:
  - `FrameInputs`, every field (#739's one way in): time, dt, the two holds,
    grass time and comb, shallow wooze's draws, the boil, pixels per point,
    the 2D view, world lines, grass benders, contact velocities, ocean tint.
    None of their types are `serde` today; the capture has its own mirror
    types rather than putting serde on the renderer's.
  - The camera (every `Camera` field), the drawn `PostProcessConfig`
    (`effective_post_process_config`, with scripts' overrides and the
    quality caps laid), quality, MSAA, render scale, debug mode, the
    selected entity, a comic layout's panels.
  - **Skinning**: every entity's bone palette, shadow palette and echo
    palettes with their opacities, read back from the GPU buffers at the
    capture (living props write bones with `write_entity_bones`, which
    keeps no CPU copy), plus part tints, softness and echo blend. The bone
    buffers need `COPY_SRC`.
  - **Particles**: the packed instance stream and each emitter's draw data.
  - **Motion holds**: each held entity's kept matrix and tick. A fresh
    `MotionHold` draws the live matrix on its first frame, so a held entity
    that moved since its tick began draws where it is now, not where it was
    drawn.
  - **Assets by path and SHA-256, never copies**: the files the load
    resolved. Replay checks each hash, names a missing or changed one and
    draws its entities as placeholders.
- **Not captured in version 1**, so replay says the frame is "drawn fresh"
  for that part, beside the captured image:
  - **ASCII's temporal history** (the per-cell tone history, the edge
    history `EdgePast`, `reproject`'s last view-projection), and a user
    style's `history` targets. Measured: 112,837 of 921,600 pixels off, whole
    glyphs; exact when the captured frame was itself drawn with no history.
    Exporting it needs a hook on each style that keeps history
    (#1136). **Captured since #1136** (no version bump: a field and files
    added): `StylePass::export_history` / `import_history`
    (`flint_render::stylize::history`), the textures under
    `replay/history/`, each part keyed without the epoch or frame number,
    which are the capturing renderer's. The measurement repeated on the
    frozen bench with an orbit script: byte-identical.
  - A game's runtime meshes (`model.runtime = true`, uploaded by an
    extension), grass mask repaints (`update_grass_mask`) and runtime
    terrain edits: named as missing.
  - The water's differenced contact velocities beyond what `FrameInputs`
    carried (not measured; no water scene was replayed).

### 5. Capture never waits on the frame thread

Read back synchronously, the spike's capture costs 35-50 ms of frame
thread on the tavern and 80-130 ms on the grove at 1920 x 1080 (below): a
hitch of two to eight frames, caused by the tool that looks for hitches.
So:

- Each tap's copy is recorded into the frame's own encoder, at the tap
  (#139's one submission), into a staging buffer made for the capture.
- The staging buffers are mapped with `map_async` and polled with
  `PollType::Poll` on the frames after, as the grass count is (ADR 0092),
  never waited on. Copying out of mapped memory (18-73 ms of the spike's
  cost) and encoding the files happen on a writer thread.
- One capture is in flight at a time: its staging is the raw size of what
  it taps (110-176 MB at 1080p) until written.
- The capturing frame, and those until its read-back lands, are marked in
  the frame history (`capturing`), so the hitch detector (#1076) skips them.
- What stays on the frame's GPU timeline is the copies themselves (4.1-4.5
  ms for the tavern's 110 MB, warm). `flint render` may wait: it's headless
  and has no frame to keep.

### 6. Replay draws with the normal host and says how close it came

The replay host is the host protocol as every other host drives it, in a
headless `SceneRenderer`, with the captured state written in through the
same public entry points (`write_entity_bones`, `prepare`, the config
setters). Its final image is compared with the captured one, and the tool
says which it is: **byte-identical**, or **differs in N pixels**, with the
`fresh_only` list as the reason when one applies. A pass's replayed image
can be compared with its captured target the same way.

### 7. The tool lives in `flint frame`

`flint frame show | diff | image | open | replay` is one subcommand of the
`flint` CLI, its arguments boxed (`Commands::Frame(Box<FrameArgs>)`, as
`Commands::Check` boxes its own), so `Commands` grows by one pointer, not by
the arguments' size (the 1 MB main-thread stack note in the root CLAUDE.md).
The nested verbs keep the top level at one word, the direction #709's
command-surface tidy takes. Code in `crates/flint-cli/src/commands/frame/`,
the manifest types in `flint_render::frame_capture`.

It is a standalone tool in the sense that matters: `flint frame open x.flintframe`
runs on a capture with no game running, on any machine. A separate binary
was the alternative, but the window (#1088) and replay (#1089) need what
`flint` already links (egui, winit, wgpu, the scene loader and schemas, the
headless render host and its import memoization), so a second binary would
build and link all of that again in every worktree, and `flint --help` and
the docs-check maps are where agents look for a command.

### 8. The line with RenderDoc

Flint's debugger answers questions at Flint's level: which passes ran and
why one was skipped, what each wrote, which entity each draw is, what each
pass and phase cost, what the frame's settings were, what the CPU was doing
and the hitch around it, how two captures differ, and (replay) what the
frame looks like without an entity, a pass, or past draw N. It reads on any
machine and in CI, and agents read it as JSON.

It will not do what a GPU debugger does below that level, and RenderDoc
(Vulkan, D3D12) and PIX (D3D12) stay the tools for:

- **Pixel history**: every draw that touched a pixel, with its depth,
  stencil and blend outcomes.
- **Raw pipeline state**: bound resources, descriptor contents, blend,
  depth and raster state, root signatures and layouts per event.
- **Buffer contents**: uniforms, vertex and index buffers, storage buffers
  per event, and the mesh viewer's pre- and post-vertex-shader data.
- **Shader debugging**: stepping a shader invocation for one pixel or vertex.
- **The API call list**, and hardware counters per event.

**Capturing a Flint frame in RenderDoc today** (the steps as RenderDoc and
wgpu document them; not run for this ADR: neither RenderDoc nor PIX was
installed on the spike's machine, and #1083 checks them with a screenshot):

1. Build the player: `cargo build -p flint-player` (or `--release`).
2. RenderDoc, **Launch Application**: executable
   `target/debug/flint-player.exe`, working directory the repo (or the
   game's) root, arguments the scene (`flint-examples/showcase.scene.toml`),
   environment `WGPU_BACKEND=vulkan` or `WGPU_BACKEND=dx12` (read by
   `gpu_select`, `wgpu::Backends::from_env`), with `WGPU_ADAPTER_NAME` or
   `--gpu` to choose the adapter on a machine with two.
3. **Launch**, then **F12** or **Print Screen** in the game window captures
   the next presented frame (or **Capture Frame(s) Immediately** in
   RenderDoc). Open the capture from RenderDoc's list.
4. **PIX** is DX12 only: `WGPU_BACKEND=dx12`, **Launch Win32** with "For GPU
   capture" ticked, then Print Screen or the capture button.

`flint render` is headless, so it presents nothing and a hotkey has no
frame to cut. It needs RenderDoc's in-application API around the frame:
wgpu 30's `Device::start_graphics_debugger_capture` /
`stop_graphics_debugger_capture`, which call `StartFrameCapture` /
`EndFrameCapture` when RenderDoc is injected (wgpu-hal's `renderdoc` feature
is on in Flint's build). Flint doesn't call them yet; #1083 adds the flag.
Until #1083's labels land, a frame shows as encoder and pass labels with
unnamed draws.

## Measured

On the spike's machine: NVIDIA GeForce RTX 5070 Laptop GPU, Vulkan, driver
592.07, Windows 11. Each replay is a fresh `Stage` drawing into the same
target as the captured frame.

**Does a replay match the captured frame?**

| Scene | Frame | Pixels off | What it shows |
|---|---|---|---|
| `showcase_game/levels/tavern_hub` (36 entities, 30 particles) | `--time 2`, 1920x1080 | **0** of 2,073,600 | world, config, camera, `FrameInputs`, particles |
| same | `--script-time 2 --anim-time 0`, 1280x720 | **0** of 921,600 | scripts' lights and bob, animated patrons |
| same, the replay host stepping the game once at load | | 68,691 (max 30 levels) | replay must run no game step |
| same, without the captured bones | | 1,128 (max 133) | bones are an input |
| `flint-examples/woozy_grove_grass` (98 entities after load: 38 living trees with echoes, compute grass, shallow wooze) | `--time 3`, 1280x720 | **0** of 921,600 | authored scene + world by name + skinning from the GPU |
| same, the world saved after load and loaded as a scene | | 485,649 (max 205) | model parts and living meshes don't load back |
| same, with the captured ids kept | | 469,191 | still no trees: not the ids alone |
| same, authored scene + world, no bones | | 304,768 (max 184) | trees at their rest pose |
| same, bones but not echoes | | 218,921 (max 146) | the canopies' echoes are bones too |
| `flint-examples/ascii_motion.scene.toml` (ASCII, scripted camera) | `--script-time 0..1 --fps 30`, the frame at 29/30 s | 112,837 (max 255) | ASCII history isn't captured |
| same, that frame drawn with no history | | **0** | history is the only difference |
| `flint-examples/motion_hold.scene.toml` (`rate = 12`, animated YBot) | `--script-time 0..1 --fps 50 --anim-time 0`, at 0.98 s (four frames into a held tick) | **0** | matched because this scene's held world matrices didn't move within the tick; see decision 4 |
| `flint-examples/shading_boil.scene.toml` (boil seed 11, amount 1, step 0.25) | `--time 0..1 --fps 30`, at 29/30 s | **0** | the boil is a `FrameInputs` value |

**What a capture costs**, release build, 1920 x 1080, every target the frame
keeps read back at the frame's end in one submission (three runs of the
tavern, two of the grove):

| | tavern_hub | woozy_grove_grass |
|---|---|---|
| targets | 15 | 19 (Kuwahara adds four full-size float targets) |
| raw bytes | 110.0 MB: 4 shadow layers 67.1, float 30.4, 8-bit 12.4 | 176.3 MB |
| record + submit the copies | 0.3-0.4 ms | 0.4-0.6 ms |
| GPU wait, first capture | 13.2-16.5 ms | 57.3-58.6 ms |
| GPU wait, a second at once | 4.1-4.5 ms | 52.9-55.4 ms |
| copy out of mapped memory | 18.0-33.7 ms | 27.1-73.4 ms |
| files written (PNG for 8-bit, raw for the rest) | 101 MB in 52-284 ms | 168 MB in 87-95 ms |
| that folder deflated (zlib level 1, as EXR's ZIP) | 19.7 MB (shadows 67.1 to 0.7 MB, floats 30.4 to 9.6) | 56.8 MB (shadows to 2.0, floats 96.8 to 47.4) |
| world snapshot | 36 entities, 26.8 KB of TOML, 9-15 ms | 98 entities, 42.3 KB |
| skinning | YBot: 1 palette, 16.4 KB | 38 bone + 121 echo palettes, 2.6 MB |
| replay: load and draw | 105-122 ms | 1.9 s (debug: 13 s) |

The grove's GPU wait didn't fall on a second capture as the tavern's did,
and wasn't looked into: #1085 should time the copies with the GPU timer.
Shadow layers are most of a plain scene's bytes and almost nothing
compressed (a cleared map). A spot light's layer is kept even with no spot.

## Alternatives considered

- **Results only.** Simple, always opens, and decision 1 makes it the base
  layer. Alone it rules out #1089, and the spike showed replay is exact for
  most frames, so the extra layer earns its place.
- **Save the world as a scene and load it.** The natural reading of "the
  scene saved with `flint_scene`". It fails on any scene with a multi-node
  GLB or a living prop, because loading derives entities and meshes the
  saved file can't name. The authored scene plus the world over it by name
  is what works.
- **Record the GPU commands** (wgpu's `trace` feature and `wgpu-player`).
  Exact at the API level, but bound to the wgpu version that wrote it, large,
  and blind to entities, passes and Flint's settings. It's a worse RenderDoc,
  not a Flint debugger.
- **A zipped capture.** One file is easier to attach, but agents can't read
  an image out of it, and it adds a deflate pass over PNGs and EXRs that
  are already compressed. A pack command can come later.
- **Mirror every bone write on the CPU** instead of reading the GPU buffers
  at the capture. That costs every frame (living props write 2.6 MB of
  palettes in the grove) for a moment that is rare. The read-back costs only
  the capture.
- **A separate `flint-frame` binary.** See decision 7.
- **Make every pass's tap a blit draw** instead of a `copy_texture_to_texture`,
  which needs no `COPY_SRC`. More code at every tap for a usage flag. #1085
  checks that the flag leaves the goldens byte-identical.

## Consequences

- **Nothing the frame draws into has `COPY_SRC` today** (HDR, SSAO and its
  blur, volumetric, bloom mips, the style chain's targets, the shadow array,
  bone buffers; the headless colour target has it). The spike added it to
  each. #1085 adds it to the targets, #1086 to the bone buffers, each
  checking the goldens are byte-identical.
- Replay has a list of renderer-held state to keep up to date. A new piece of
  per-frame renderer state is a new `FrameInputs` field already (ADR 0118);
  state the renderer keeps across frames for its own reasons (a cache that
  changes what's drawn, a history) is a capture input, or goes on the
  `fresh_only` list.
- Old captures keep their static views. Their replay degrades to "differs
  in N pixels" as the renderer moves, which is the honest answer.
- Every later ticket of #1081 was edited where this changes its scope:
  #1083 gains `flint render`'s capture flag and checks this ADR's RenderDoc
  and PIX steps; #1084 puts its types in `flint_render::frame_capture` and
  names draws by entity name; #1085 has the storage formats, `COPY_SRC` and
  the read-back design; #1086 has the replay inputs and the in-flight rule;
  #1087 and #1088 take `flint frame` with boxed arguments and the version
  rules; #1089 has the replay host and the `fresh_only` list.
- Follow-up: #1136, export and import of a style's history, so ASCII
  frames replay exactly.

## Sources

The spike branch `spike/frame-capture-1082`
(`crates/flint-cli/src/commands/render_spike_capture.rs`,
`crates/flint-render/src/scene_renderer/capture_spike*.rs`); #1081, #1082
and the tickets named above; ADR 0118 and the host protocol in
`crates/flint-render/CLAUDE.md`; ADR 0092 (a read-back that never waits);
wgpu 30's `Device::start_graphics_debugger_capture` docs.
