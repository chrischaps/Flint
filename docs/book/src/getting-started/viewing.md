# The Scene Viewer

The Flint viewer is a real-time 3D window for validating scenes. It renders your scene with full PBR shading and shadows, applies the scene's own post-processing, and provides an egui inspector panel for browsing entities and editing component properties.

## Launching the Viewer

```bash
flint edit levels/tavern.scene.toml --watch --schemas schemas
```

The `--watch` flag enables hot-reload: edit the scene TOML file, and the viewer re-parses and re-renders automatically. The entire file is re-parsed on each change (not incremental), which keeps the implementation simple and avoids synchronization issues.

`flint serve` still works as a hidden alias for the same viewer.

## Camera Controls

The viewer uses an orbit camera that rotates around a focus point. If the scene has a `[camera]` block, the orbit starts from that authored framing.

| Input | Action |
|-------|--------|
| Left-drag | Orbit around focus (or drag gizmo axis when hovering) |
| Right-drag | Pan the view |
| Scroll | Zoom in/out |
| W / A / S / D | Orbit by key (while no entity is selected) |
| Q / E | Zoom out / in by key (while no entity is selected) |
| Space | Return to the scene's authored `[camera]` framing (viewer default if there is none) |
| O | Toggle auto-orbit turntable |
| [ / ] | Slow down / speed up auto-orbit |
| Ctrl+R | Force reload |
| Escape | Quit / cancel gizmo drag |

Any manual orbit input cancels auto-orbit. Start in turntable mode with `--auto-orbit`.

## Transform Gizmo

When you select an entity in the inspector, a gizmo appears at its position with colored axis arrows and plane handles:

- **Red arrow** --- drag to move along X axis
- **Green arrow** --- drag to move along Y axis
- **Blue arrow** --- drag to move along Z axis
- **Plane handles** (small squares at axis intersections) --- drag to move in two axes simultaneously

While an entity is selected, **W**, **E** and **R** switch the gizmo to translate, rotate and scale instead of orbiting the camera.

The gizmo uses constraint-plane dragging: for single-axis movement, it automatically picks the plane most perpendicular to your camera view. Inactive axes dim while dragging to clearly show the active constraint.

### Editing Shortcuts

| Input | Action |
|-------|--------|
| W / E / R | Gizmo mode: translate / rotate / scale (entity selected) |
| Ctrl+S | Save scene to disk |
| Ctrl+Z | Undo position change |
| Ctrl+Shift+Z | Redo position change |
| Escape | Cancel current gizmo drag |

All position changes are tracked in an undo/redo stack. Saving writes the modified positions back to the scene TOML file.

## The Inspector Panel

The egui-based inspector panel (on the left side of the viewer) provides:

- **Entity tree** --- hierarchical list of all entities in the scene, reflecting parent-child relationships
- **Component editor** --- select an entity to view and edit its component values; position fields are editable via drag-value widgets with color-coded labels (red/green/blue matching the gizmo axes)
- **Constraint overlay** --- validation results from `flint-constraint`, highlighting any rule violations

## Rendering Features

The viewer renders scenes with the same PBR pipeline used by the player:

- Cook-Torrance physically-based shading
- Cascaded shadow mapping from directional lights
- glTF mesh rendering with material support
- The scene's `[post_process]` block, applied on load
- Live particles: `particle_emitter` and `particle_effect` entities simulate in the viewer, so inspector edits show immediately
- Render stats overlay (**F2**)
- Normal arrows (**F3**)
- The Rendering & Effects menu (**F4**)
- Fullscreen toggle (**F11**)

### The Rendering & Effects Menu (F4)

F4 opens one window holding every render and post-process control: post-processing on/off, exposure, vignette, SSAO, depth of field, fog, bloom, color grade and film grain, FXAA, Kuwahara, render modes, dither and volumetric light, shadows and their resolution, the lighting levers, vertical FOV, and the shading debug mode (PBR, wireframe overlay, wireframe, normals, depth, UV, unlit, metal/rough). Changes apply immediately.

Two controls are specific to the viewer:

- **Authored post vs viewer default** --- swap between the scene's `[post_process]` block and the viewer's neutral look, to check what a scene's grading is actually doing
- **DoF follow** --- the depth-of-field focus plane tracks the last selected entity, so you can pick a focus distance by clicking
- **Particles** --- simulate/draw toggle, pause, speed and restart for the live particle simulation, plus each `particle_effect` entity's asset name

See [Post-Processing](../concepts/post-processing.md#the-rendering--effects-menu-f4) for the full section list.

## Playing a Scene

To experience a scene in first-person with physics, use `play` instead of `edit`:

```bash
flint play levels/tavern.scene.toml
```

See the [CLI Reference](../cli-reference/overview.md) for full `play` command details and controls.

## Headless Rendering

For CI pipelines and automated screenshots, render to PNG without opening a window:

```bash
flint render levels/tavern.scene.toml --output preview.png --width 1920 --height 1080
```
