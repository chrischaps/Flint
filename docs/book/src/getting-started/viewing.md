# The Scene Viewer

The Flint viewer is a real-time 3D window for validating scenes. It renders your scene with full PBR shading and shadows, and provides an egui inspector panel for browsing entities and editing component properties.

## Launching the Viewer

```bash
flint serve levels/tavern.scene.toml --watch --schemas schemas
```

The `--watch` flag enables hot-reload: edit the scene TOML file, and the viewer re-parses and re-renders automatically. The entire file is re-parsed on each change (not incremental), which keeps the implementation simple and avoids synchronization issues.

## Camera Controls

The viewer uses an orbit camera that rotates around a focus point:

| Input | Action |
|-------|--------|
| Left-drag | Orbit around focus (or drag gizmo axis when hovering) |
| Right-drag | Pan the view |
| Scroll | Zoom in/out |
| Space | Reset camera |
| R | Force reload |
| Escape | Quit / cancel gizmo drag |

## Transform Gizmo

When you select an entity in the inspector, a **translate gizmo** appears at its position with colored axis arrows and plane handles:

- **Red arrow** --- drag to move along X axis
- **Green arrow** --- drag to move along Y axis
- **Blue arrow** --- drag to move along Z axis
- **Plane handles** (small squares at axis intersections) --- drag to move in two axes simultaneously

The gizmo uses constraint-plane dragging: for single-axis movement, it automatically picks the plane most perpendicular to your camera view. Inactive axes dim while dragging to clearly show the active constraint.

### Editing Shortcuts

| Input | Action |
|-------|--------|
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
- Debug rendering modes (cycle with **F1**)
- Shadow toggle (**F4**)
- Fullscreen toggle (**F11**)

## Playing a Scene

To experience a scene in first-person with physics, use `play` instead of `serve`:

```bash
flint play levels/tavern.scene.toml
```

See the [CLI Reference](../cli-reference/overview.md) for full `play` command details and controls.

## Headless Rendering

For CI pipelines and automated screenshots, render to PNG without opening a window:

```bash
flint render levels/tavern.scene.toml --output preview.png --width 1920 --height 1080
```
