# Grass on Any Ground

The [terrain](terrain.md#grass) grows grass from its own heightmap and splat map. A game whose ground is something else, such as a mesh it builds itself, a level made of boxes or a race track, can give the grass pass its own ground instead (ADR 0089). That ground is two things over a world-space rectangle:

- **a density mask.** One byte per texel: 0 grows no grass and 255 grows full density.
- **a height.** Either level ground at one `y`, or a heightmap of world `y` values.

Everything else is the same grass: the same blades, wind, bend, colours and shadows, tuned by the same `grass.*` fields.

![Mesh Meadow: grass on a plain box under a painted mask](../images/grass-ground.png)

*The Mesh Meadow demo. The ground is one box. The mask leaves the path and the clearing bare and thins the field toward its rim. Grass grows only within `max_distance` (70 m here) of the camera, so the far side is bare.*

## From a scene

A `grass_ground` component puts grass on level ground with no code. The entity's `x` and `z` are the rectangle's minimum corner, and its `y` is the ground's height:

```toml
[entities.meadow]
[entities.meadow.transform]
position = [-48, 0, -48]

[entities.meadow.grass_ground]
mask = "mesh_meadow/mask.png"     # grayscale; pixel (0,0) at the corner, rows toward +Z
width = 96.0
depth = 96.0
scale_by_mask = true
"grass.density" = 40.0
"grass.max_distance" = 70.0
"grass.blade_height" = 0.45
```

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `mask` | string | | Grayscale PNG, relative to the scene file. It stretches over the whole rectangle, whatever its resolution |
| `width` | f32 | 64.0 | World X extent the mask covers |
| `depth` | f32 | 64.0 | World Z extent the mask covers |
| `scale_by_mask` | bool | true | Grey thins the field in proportion. When `false`, anything over `grass.density_threshold` grows at full density, as a terrain's splat layer does |
| `grass.*` | | | As on the [terrain](terrain.md#grass). `grass.enabled` defaults to `true` here; `density_layer` and `density_source` don't apply |

A scene has one grass field. A `grass_ground` replaces a terrain's grass, with a warning. The **Grass Debug** panel (`F3`) works on it as on a terrain, and commits its changes back to the `grass_ground` component. `flint render` draws it too.

Try it with `flint play demo/mesh_meadow.scene.toml`.

## From Rust

A game that builds its own world sets the ground from a `PlayerExtension`, through `ctx.gpu` (an `ExtensionGpu`), whenever its world changes:

```rust
use flint_player::{GrassConfig, GrassGround, GrassHeight};

let Some(gpu) = ctx.gpu.as_mut() else { return };

let config = GrassConfig {
    enabled: true,
    density: 30.0,
    max_distance: 90.0,
    fade_start: 70.0,
    ..GrassConfig::default()
};
let ground = GrassGround {
    origin: [-2000.0, -2000.0],   // minimum corner, world X and Z
    size: [4000.0, 4000.0],
    mask: &mask,                  // width × height bytes, row by row toward +Z
    mask_width: 2048,
    mask_height: 2048,
    scale_by_mask: true,
    height: GrassHeight::Flat(0.0),
};
if let Err(e) = gpu.set_grass_ground(&config, &ground) {
    tracing::warn!("grass: {e}");
}
```

- **`set_grass_ground`** uploads the mask and height, and replaces any grass already loaded. Call it again to repaint: a road laid, a lot built on, a field cleared. It returns `Err` only when a slice doesn't match its stated size. On a device without compute shaders it does nothing.
- **`set_grass_config`** retunes the grass without uploading the ground again. If the new config needs more instances than the buffer holds, the buffer grows before the next frame.
- **`clear_grass`** removes it.

`GrassHeight::Map { heights, width, depth }` gives the ground a shape. It's a grid of world `y` values over the same rectangle, sampled nearest. The heights are world units, unlike a terrain's heightmap, which is scaled by `height_scale`.

The same calls are on `SceneRenderer` (`set_grass_ground`, `set_grass_config`, `unload_grass`) for code that drives the renderer directly. A scene transition clears the grass.

## Only the reach of the camera

The compute pass covers only the grid cells within `max_distance` of the camera, on X and Z, clipped to the ground. The instance buffer holds what that disc can grow, plus 10%, or half the ground's area if that's smaller. So the cost follows the grass near the camera, not the size of the ground. A 3 km ground at density 40 needs a 21 MB buffer. Sizing it by area, as before, would have needed 5.7 GB.

Each cell keeps its place in the whole grid, so a blade doesn't move as the window follows the camera. Terrain grass gets the same windowing, and grows exactly as it did before.

**Cost.** On an RTX 4070, the Mesh Meadow demo at 1600×900 with density 40 and a 70 m reach grows about 110,000 blades (1.3 M triangles). It costs 1.3–1.6 ms a frame: compute, the count read back, the draw and two shadow cascades. A camera standing in an unbroken field of the same density draws about 440,000 blades, at about 4.5 ms. Every blade in reach is drawn, including those behind the camera, and the CPU waits for the blade count before the draw. Frustum culling and an indirect draw would remove both ([TheCapitol #133](https://github.com/chrischaps/TheCapitol/issues/133)).
