# The Scene Viewer

> This page is a stub. Content coming soon.

The Flint viewer is a real-time 3D window for validating scenes. This page will cover:

- Launching the viewer with `flint serve --watch`
- Camera controls (orbit, pan, zoom)
- How hot-reload works (file watching and re-parsing)
- Archetype-based coloring (rooms=blue, doors=orange, furniture=green, characters=yellow)
- Headless rendering with `flint render`

Quick start:

```bash
flint serve levels/tavern.scene.toml --watch --schemas schemas
```

| Input | Action |
|-------|--------|
| Left-drag | Orbit |
| Right-drag | Pan |
| Scroll | Zoom |
| Space | Reset camera |
| R | Force reload |
| Escape | Quit |
