# Terrain System Roadmap

## Current State (MVP)

The terrain system provides heightmap-based outdoor environments with:

- **Heightmap mesh generation** — grayscale PNG (8/16-bit) parsed into chunked grid patches via `flint-terrain` crate
- **Splat-map texture blending** — 4 texture layers controlled by RGBA splat map, tiled from world position
- **PBR lighting + shadows** — full Cook-Torrance BRDF, cascaded shadow maps, point/spot/directional lights
- **Physics collider** — trimesh collision via Rapier `register_static_trimesh()`
- **Height sampling API** — `terrain_height(x, z)` exposed to Rhai scripts for gameplay logic
- **Headless rendering** — works in both `flint play` (interactive) and `flint render` (snapshot)
- **Scene transitions** — terrain cleared and reloaded on scene change

### Scene Usage

```toml
[entities.ground]
archetype = "terrain"
[entities.ground.transform]
position = [-128, 0, -128]
[entities.ground.terrain]
heightmap = "terrain/heightmap.png"
splat_map = "terrain/splatmap.png"
layer0_texture = "terrain/grass.png"
layer1_texture = "terrain/dirt.png"
layer2_texture = "terrain/rock.png"
layer3_texture = "terrain/sand.png"
height_scale = 50.0
width = 256.0
depth = 256.0
texture_tile = 16.0
```

## Future Features

### Tier 1: Visual Quality

- **Per-layer normal maps** — add normal map paths to terrain component, sample in shader for surface detail
- **Triplanar mapping** — project textures along world axes for cliff faces where UV stretching is severe
- **Detail textures** — high-frequency micro-detail overlaid at close range, fading with distance
- **Terrain normal smoothing** — weighted average of face normals for smoother lighting on coarse meshes

### Tier 2: Performance

- **Distance-based LOD switching** — 2-3 mesh resolution levels per chunk, selected by camera distance
- **T-junction stitching** — skirt vertices or degenerate triangles to prevent cracks between LOD levels
- **GPU-driven clipmap LOD** — continuous LOD centered on camera with seamless transitions
- **Frustum culling** — skip chunks outside the camera frustum using AABB tests (already have AABB data)
- **Streaming/paging** — load chunks on demand for worlds larger than memory

### Tier 3: Authoring Tools

- **Terrain sculpting in flint-viewer** — brush tools for raise/lower/smooth/flatten/paint operations
- **Auto-splatting** — generate splat map from height (grass below threshold, rock above) and slope (cliff faces)
- **Procedural noise generation** — Perlin/Simplex/FBM noise for heightmap creation without external tools
- **Erosion simulation** — hydraulic/thermal erosion passes for realistic mountain/valley shapes
- **CLI tooling** — `flint terrain generate` (noise → heightmap), `flint terrain splat-auto` (rules → splat map)

### Tier 4: World Features

- **Water planes** — flat or animated water surface with reflection/refraction, foam at terrain intersection
- **Foliage/grass scattering** — instanced vegetation placed by splat map density + noise
- **Terrain holes** — alpha mask regions for cave entrances, tunnels, mine shafts
- **Multi-heightmap stitching** — tile adjacent heightmaps for seamless mega-terrain
- **Decals** — projected textures (paths, scorch marks, tire tracks) on terrain surface

### Tier 5: Integration

- **AI terrain generation** — integrate with asset generation pipeline for AI-created heightmaps and splat maps
- **NavMesh generation** — build navigation mesh from terrain + static geometry for AI pathfinding
- **Terrain-aware audio** — surface material detection for footstep sounds (grass, dirt, rock, sand)
- **Weather interaction** — snow accumulation on upward-facing surfaces, rain puddles in concavities
