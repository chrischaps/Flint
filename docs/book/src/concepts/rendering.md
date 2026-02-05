# Rendering

> This page is a stub. Content coming soon.

Flint uses wgpu for cross-platform GPU rendering. This page will cover:

- wgpu backend selection (Vulkan, Metal, DX12)
- Current renderer: archetype-based colored boxes with wireframes
- Viewer mode vs headless mode
- The `ApplicationHandler` pattern (winit 0.30)
- Camera system: orbit, pan, zoom
- Ground grid rendering
- Planned: PBR materials, shadow mapping, post-processing, glTF mesh rendering

Headless rendering for CI:

```bash
flint render levels/tavern.scene.toml --output preview.png --width 1920 --height 1080
```
