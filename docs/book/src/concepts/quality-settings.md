# Quality Settings

A game can change what the renderer spends while it runs: the scene's
render scale, MSAA, the shadow pass and the post-process effects. A
graphics menu offers them as tiers, and an auto-tuner steps them down when
the frame runs long or the device gets hot. One set of settings,
`QualitySettings` in `flint_core::quality`, reaches the renderer from every
source, and the renderer applies it between frames.

| Key | Values | Default | What it costs |
|---|---|---|---|
| `render_scale` | 0.25 to 1 | 1 | The 3D scene is drawn at this fraction of the surface on each side, then upscaled under a full-resolution HUD. A ceiling: the web's pixel-ratio cap can lower it further |
| `render_scale_auto` | bool | false | Step under `render_scale` to hold the frame rate (`--render-scale auto`) |
| `msaa` | 1 or 4 | 1 | Samples per pixel in the scene passes. 2 reads as 1, 8 as 4 |
| `shadows` | bool | true | The shadow pass |
| `shadow_resolution` | 256 to 4096, a power of two | 2048 | Each cascade's map. Other sizes round to the nearest power of two |
| `shadow_cascades` | 1 to 3 | 3 | Cascades drawn. Each one draws every shadow caster, so this is the shadow pass's cost more than its size is |
| `shadow_distance` | 10 to 500 m | 200 | How far shadows reach from the camera. A shorter reach spreads the same texels over less ground |
| `post_process` | bool | true | The whole post chain: bloom, SSAO, fog, vignette, grade |
| `bloom`, `ssao`, `fog`, `volumetric` | bool | true | Caps on the scene's effects (below) |
| `ssao_samples` | 1 to 64 | 64 | The most SSAO samples a pixel takes |

## Caps, not settings

The post-process keys are caps. `ssao = false` turns the scene's SSAO off;
`ssao = true` leaves it as the scene authored it, so a scene with no SSAO
still has none. A tier can't add an effect the scene's look didn't ask for.
`ssao_samples = 16` lowers a scene asking for 64 and leaves one asking for 8
alone. `fog = false` turns off distance fog, height fog and ground fog.

The renderer keeps the scene's look whole (`post_process_config()`) and
draws it under the caps (`effective_post_process_config()`). A scene load
replaces the look and keeps the caps, so a tier holds from track to track,
and lifting a cap brings the scene's effect back.

## From a script

```rhai
// A graphics menu's "Low"
set_quality(#{
    render_scale: 0.6,
    ssao: false,
    bloom: false,
    shadow_cascades: 1,
    shadow_distance: 60.0,
});

// One key
set_quality("msaa", 4);

// Read them back: a map of every key
let q = get_quality();
log("scale " + q.render_scale + ", " + q.shadow_cascades + " cascades");
let samples = get_quality("msaa");     // () for a key that doesn't exist

// What the GPU is, to pick a starting tier
let gpu = gpu_info();
if gpu.class == "mobile" { set_quality(#{ render_scale: 0.75, ssao_samples: 16 }); }
```

`set_quality` merges the keys it's given into the settings in force and
returns `true` when it took them all. A key it doesn't know or a value of
the wrong type is skipped with a warning in the log, and the rest are still
applied. Numbers are clamped to the ranges above, and an integer is fine
where a float is wanted. The change lands at the end of the frame, after
every script's `on_update` and before the frame is drawn. `get_quality()`
reads the request back at once, clamped.

`gpu_info()` returns a map:

| Key | Example | Notes |
|---|---|---|
| `name` | `"NVIDIA GeForce RTX 4070"`, `"Mali-G715"` | Often empty in a browser, which hides it |
| `vendor` | `"nvidia"`, `"arm"`, `"qualcomm"`, `"intel"`, `"apple"` | From the PCI vendor id; `"unknown"` otherwise |
| `device_type` | `"discrete"`, `"integrated"`, `"virtual"`, `"cpu"`, `"other"` | wgpu's |
| `backend` | `"vulkan"`, `"dx12"`, `"metal"`, `"gl"`, `"webgpu"` | |
| `driver` | `"NVIDIA 566.36"` | Where the driver says |
| `class` | `"desktop"`, `"integrated"`, `"mobile"`, `"software"`, `"unknown"` | What to pick a tier from. Android, iOS and the ARM, Qualcomm and Imagination GPUs are `"mobile"` whatever the driver calls them |
| `platform` | `"windows"`, `"macos"`, `"linux"`, `"android"`, `"ios"`, `"web"` | |

The engine doesn't save a player's choice. A game keeps it with
`persist_set` and calls `set_quality` again on start.

## From the command line, a page and a phone

The same `key=value` list, comma-separated, works everywhere:

```bash
flint-player scenes/track.scene.toml --quality "shadow_cascades=2,ssao_samples=16"
flint render scenes/track.scene.toml --output low.png \
    --quality "render_scale=0.6,ssao=0,bloom=0,shadow_cascades=1,shadow_distance=60"
```

- **flint-player**: `--quality` is applied over `--msaa`, `--render-scale`
  and `FLINT_SHADOWS` / `FLINT_SHADOW_RES`. A key it doesn't know stops the
  player at start rather than run with the setting dropped.
- **flint render**: `--quality` is applied after every other flag, so a
  headless snapshot can render a tier.
- **The web player**: `?quality=shadow_cascades=1,ssao=0`, over `?msaa=` and
  `?scale=`.
- **Android**: a debug build's `dev_profile.toml` takes a `[quality]` table
  or a `quality = "..."` string (flint-android's `DevProfile`, written over
  `adb shell run-as`).
  A shipped game's choice comes from its scripts.

## What a change rebuilds

`SceneRenderer::set_quality(device, &settings)` applies only what changed,
so calling it every frame with the same settings costs nothing. Call it
between frames, never with a frame's passes half encoded.

- **MSAA** rebuilds every scene pipeline, whose sample count is baked in,
  and drops the MSAA targets; the next frame makes them at the new count.
  Meshes, textures and bind groups are kept: the new pipelines are made
  against bind group layouts equal to the old ones, which wgpu treats as the
  same layout (WebGPU's group-equivalence). That's 33 pipelines, compiled
  again: about 15 ms on an RTX 4070 in a release build (a one-frame hitch),
  0.2 to 1.3 s in a debug build, and not yet measured on a phone, where
  shader compiles are slower. It belongs in a menu or a tier change, not a
  per-frame tuner.
- **Render scale** rebuilds the HDR chain at the new size.
- **Shadow resolution** makes new shadow maps and the light bind group.
- **Cascades and reach** change uniforms only: the shadow texture keeps all
  three layers.
- **Shadows off** writes uniforms that light everything, since the shaders
  sample the maps whatever the pass does.
- **Post caps** recompute the drawn config.

On Android the renderer outlives a suspend; on resume the player applies the
settings again over the resized chain, so a tier survives the app going to
the background.

## How a tier measures

FlintKart's profile on a Pixel 8 Pro (FlintKart `docs/spikes/pixel-8-pro-load-levers.md`)
found SSAO the heaviest GPU pass, shadows next, with the shadow pass's cost
in its cascades rather than its map size, and MSAA free on the tiled Mali.
Those are the levers here. The CPU side of a frame (scripts, the HUD) isn't
a quality setting.

`tests/quality.rs` in `flint-render` toggles every setting fifty times on a
live renderer, a frame at each, under a validation error scope, and checks
the default settings then draw the same frame, pixel for pixel. It also
checks that each lever changes the frame, and that one cascade still draws
a shadow.
