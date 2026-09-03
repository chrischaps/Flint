# Debug Panels

Flint's runtime tuning surface is a set of egui **debug panels** that live in the `flint-debug-ui` crate and are hosted by the player and, for one of them, by the scene viewer. They exist so that a tuning session ends as a diff to the scene file rather than as notes: every panel edits live values, and most can write those values back into the scene TOML.

Panels are dev surface. They compile only when the player's `debug-hud` cargo feature is on (it is on by default), and a feature-off build carries zero panel code and must still compile.

## Keys

| Key | What it toggles | Host |
|-----|-----------------|------|
| `F3` | Every scene-component panel the current scene created (ocean, day/time, weather, grass, camera, reality, visitor, dead calm) | player |
| `F4` | The **Rendering & Effects** panel, on its own so it never flips out of phase with F3 | player and scene viewer |
| `` ` `` (backquote) | **Music Guide**, only while a [music session](../concepts/music-sessions.md) runs | player |
| `\` (backslash) | **Manifest Map** timeline strip, same condition | player |
| `Escape` | Releases the mouse so panels can be clicked; clicking the world recaptures it | player |

Opening any panel releases the cursor; closing the last one recaptures it if the scene has a player entity. When a key has nothing to toggle (no panels in the scene, no music session, no renderer) the player logs a note instead of failing.

The old per-effect function keys (F1 debug-mode cycle, F4 shadows, F5 to F10 per-effect toggles, F12 kuwahara) are gone. Everything they flipped now lives in the Rendering & Effects panel, along with the non-binary parameters those keys could never expose (ADR 0053, consolidated render debug menu).

## Layout

Side panels are fold-open headers distributed across up to three columns. Column assignment is a greedy lightest-bin pass over a per-panel weight so the tall panels do not all land in one column: Rendering & Effects weighs 60, Ocean Debug 46, Grass Debug 22, Reality 20, Weather 15, Day / Time 10, and anything else, including every game-supplied panel, 6. The assignment is deterministic and never produces an empty column.

A panel can instead ask for the `Bottom` layout, a full-width strip. The Manifest Map uses it; timeline-shaped panels should.

## The Panel Roster

Scene-component panels are created **only when their driving component is present** on some entity, so a scene with no ocean never sees an ocean panel.

| Panel | Component | What it tunes |
|-------|-----------|---------------|
| Ocean Debug | `ocean` | Wave spectrum, colours, foam, contact foam, cel band edges, clarity and turbidity, grid, CPU/GPU parity probe |
| Day / Time | `time_of_day` | Clock readout, 0 to 24 h scrub, preset hours, natural-advance toggle, day counter, day length, sun path tilt |
| Weather | `weather` | Weather state, with one-shot triggers |
| Camera | `camera_tuning` | Vertical FOV (the same component is applied even without the panel) |
| Grass Debug | `terrain` with `grass.enabled` | Every field of the [grass](../concepts/terrain.md#grass) block |
| Reality | `reality` | Read-only status of a script-scheduled "reality tear" (active render mode, mix, time to next) plus trigger and end-now buttons |
| Visitor | `raft_visitor` | Visitor state |
| Dead Calm | `dead_calm` | Dead-calm state |
| Rendering & Effects | always, when a renderer exists | See below |
| Music Guide | active music session | Upcoming input windows and per-channel targets |
| Manifest Map | active music session | Suite structure, playhead, this run's history |

`time_of_day`, `weather`, `reality`, `raft_visitor` and `dead_calm` are game-side component conventions; the engine ships the panel, the game ships the schema.

### Rendering & Effects (F4)

One consolidated home for every render and post-effect control. Its groups, top to bottom:

- **Post chain**: "Freeze script post overrides" (player only; see below), post-processing on/off, exposure, vignette on/off, vignette intensity and smoothness, chromatic aberration, radial blur, desaturate.
- **SSAO**: enabled, radius, intensity, bias, samples (1 to 64; 16 is the quality/cost sweet spot).
- **Depth of field**: strength, focus distance, focus range.
- **Fog**: enabled, colour, density, start, end, height fog on/off, height falloff, height origin.
- **Bloom**: enabled, intensity, threshold, soft threshold.
- **Grade / Grain / FXAA**: lift, gamma, gain, a "Neutral grade" reset, film grain, FXAA.
- **Kuwahara**: enabled, radius, sharpness, hardness, anisotropy.
- **Render mode**: mode combo (none, matrix, blood, drunk, tron, underwater), mix, the four mode params.
- **Dither / Volumetric**: dither on/off and intensity, volumetric on/off, samples, density, decay.
- **Shadows**: enabled, resolution combo (512, 1024, 2048, 4096). Changing resolution rebuilds the shadow pass.
- **Lighting**: ambient sky, ambient ground, diffuse wrap, Oren-Nayar, sheen colour and strength, "Reset lighting". These are the [lighting levers](../concepts/lighting.md).
- **Camera**: vertical FOV.
- **Debug mode**: the shading combo (PBR, wireframe overlay, wireframe only, normals, depth, UV checker, unlit, metallic/roughness).

Every field maps onto a `[post_process]`, `[environment]` or `[camera]` key documented in [Post-Processing](../concepts/post-processing.md) and [File Formats](../formats/overview.md), or onto a `flint render` flag, so a value found in the panel can be authored.

In the scene viewer the same panel gains two extras: a switch between the scene's authored `[post_process]` block and the viewer's own defaults, and **DoF follow**, which keeps the focus plane on the last selected entity.

## Live Mirror and Write-Through

Every panel follows one ownership model:

1. While the panel is **clean** the host refreshes it from live engine state every frame. What you see is the truth about the current value, including values a script is driving.
2. When you **edit** a field the panel becomes dirty. The host applies the panel's state back to the engine, routing expensive work by per-group change flags (a post-config upload is cheap; a shadow-pass rebuild or a debug-mode pipeline swap is not), then clears the dirty flag.
3. Fields a script stamps each frame are visibly **reclaimed** on the next frame. The panel does not pretend to own a value it does not own.

For the render panel the player adds **Freeze script post overrides**: while frozen, script and ladder stamps on the post fields are skipped so panel edits stick. It is a tuning aid, not a way to override a game.

**Commit to File** on a scene-component panel writes its current values back into the scene TOML through the scene document patcher, field by field, keeping the rest of the file untouched. Values a game script owns each frame (a day counter, a published factor) are deliberately never committed.

## Adding a Panel from a Game

A panel is any type implementing the `DebugPanel` trait from `flint-debug-ui`:

```rust
pub trait DebugPanel {
    fn name(&self) -> &str;                 // egui id and title
    fn ui(&mut self, ui: &mut egui::Ui);   // draw the contents
    fn is_open(&self) -> bool;
    fn toggle(&mut self);
    fn layout(&self) -> PanelLayout { PanelLayout::SideRight }
    fn is_dirty(&self) -> bool;             // unapplied edits?
    fn clear_dirty(&mut self);
    fn as_any(&self) -> &dyn std::any::Any;
    fn as_any_mut(&mut self) -> &mut dyn std::any::Any;
}
```

The player holds `Vec<Box<dyn DebugPanel>>` and renders them generically. Construction follows one pattern: look for the first entity carrying the driving component, build a config from that component, and push the panel with the scene path and entity name so Commit to File knows where to write. The host then downcasts through `as_any_mut` when it needs the concrete config to apply. Unknown panel names take the default column weight; nothing else needs registering.

Keep panels out of the felt experience. They are judgment-shaped by design and should never double as a HUD; the script-driven [UI](../concepts/scripting.md) is for that.

## Further Reading

- [Post-Processing](../concepts/post-processing.md): every field in the render panel
- [Lighting](../concepts/lighting.md): the lighting levers group
- [Music Sessions](../concepts/music-sessions.md): the Music Guide and Manifest Map
- [The Scene Viewer](../getting-started/viewing.md): the viewer's F4 extras
