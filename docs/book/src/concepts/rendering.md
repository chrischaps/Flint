# Rendering

Flint uses wgpu 23 for cross-platform GPU rendering, providing physically-based rendering (PBR) with a Cook-Torrance BRDF, cascaded shadow mapping, and full glTF mesh support.

## PBR Shading

The renderer implements a metallic-roughness PBR workflow based on the Cook-Torrance specular BRDF:

- **Base color** --- the surface albedo, optionally sampled from a texture
- **Roughness** --- controls specular highlight spread (0.0 = mirror, 1.0 = diffuse)
- **Metallic** --- interpolates between dielectric and metallic response
- **Emissive** --- self-illumination for light sources and glowing objects

Materials are defined in scene TOML via the `material` component, matching the fields in `schemas/components/material.toml`.

## Shadow Mapping

Directional lights cast shadows via cascaded shadow maps. Multiple shadow cascades cover different distance ranges from the camera, giving high-resolution shadows close up and broader coverage at distance.

Toggle shadows at runtime with **F4**.

## Camera Modes

The renderer supports two camera modes that share the same view/projection math:

| Mode | Usage | Controls |
|------|-------|----------|
| **Orbit** | Scene viewer (`serve`) | Left-drag to orbit, right-drag to pan, scroll to zoom |
| **First-person** | Player (`play`) | WASD to move, mouse to look, Space to jump, Shift to sprint |

The camera mode is determined by the entry point: `serve` uses orbit, `play` uses first-person. Both produce the same view and projection matrices.

## glTF Mesh Rendering

Imported glTF models are rendered with their full mesh geometry and materials. The `flint-import` crate extracts meshes, materials, and textures from `.glb`/`.gltf` files, which the renderer draws with PBR shading.

## Viewer vs Headless

The renderer operates in two modes:

**Viewer mode** (`flint serve --watch`) opens an interactive window with:
- Real-time PBR rendering
- egui inspector panel (entity tree, component editor, constraint overlay)
- Hot-reload: edit the scene TOML and the viewer updates automatically
- Debug rendering modes (cycle with **F1**)

**Headless mode** (`flint render`) renders to a PNG file without opening a window --- useful for CI pipelines and automated screenshots:

```bash
flint render levels/tavern.scene.toml --output preview.png --width 1920 --height 1080
```

## Technology

The rendering stack uses winit 0.30's `ApplicationHandler` trait pattern (not the older event-loop closure style). wgpu 23 provides the GPU abstraction, selecting the best available backend (Vulkan, Metal, or DX12) at runtime.

## Further Reading

- [The Scene Viewer](../getting-started/viewing.md) --- getting started with the viewer
- [Physics and Runtime](physics-and-runtime.md) --- the game loop and first-person gameplay
- [Headless Rendering](../guides/headless-rendering.md) --- CI integration guide
