# Terrain System Roadmap

## Current State

The terrain system provides heightmap-based outdoor environments with full authoring tools and runtime integration.

### Core (`flint-terrain` crate — pure data, no GPU dependency)

- **Heightmap loading** — grayscale PNG (8/16-bit) with bilinear interpolation sampling
- **Chunked mesh generation** — grid divided into chunks, each with precomputed positions, normals, UVs, indices, and AABB bounding box
- **Height sampling** — `sample_world(x, z, config)` for exact world-position queries (matches rendered surface)
- **Trimesh export** — `trimesh_data()` produces vertex/triangle arrays for physics colliders

### Procedural Generation

- **Noise layers** — FBM with Perlin, Simplex, Value, and Ridged noise types; configurable octaves, frequency, lacunarity, persistence
- **Blend modes** — add, multiply, max, min, overlay for combining layers
- **Flatten layers** — clamp above/below height thresholds with falloff
- **Erosion simulation** — thermal and hydraulic erosion passes
- **Automatic splat map generation** — rule-based layer assignment from height range, slope limits, and noise modulation with smooth edge fades
- **Seeded generation** — fixed or random seed modes for reproducibility
- **Spec format** — `.terrain.toml` files define geometry, layers, and splat rules

### Terrain Editor (`flint edit *.terrain.toml`)

- **Generate mode** — tweak spec parameters (geometry, noise layers, erosion, splat rules) with real-time regeneration (200ms debounce)
- **Sculpt mode** — height brushes (raise, lower, smooth, flatten, noise) with Gaussian falloff, applied via ray-heightmap intersection
- **Paint mode** — splat brushes (paint, erase) for per-layer weight editing with automatic normalization
- **Camera** — orbit controller with right-click drag
- **Keyboard shortcuts** — Tab (toggle UI), 1/2/3 (mode switch), R (randomize seed), Ctrl+S (save), Ctrl+E (export), [/] (brush size)
- **File watching** — auto-reloads spec on external edit
- **Export** — saves heightmap and splat map as 16-bit and 8-bit PNG respectively

### Rendering (`flint-render`)

- **Terrain pipeline** — dedicated wgpu pipeline with splat map + 4 layer textures (8 texture bindings)
- **PBR shader** — Cook-Torrance BRDF, cascaded shadow maps, point/spot/directional lights, hemisphere ambient
- **Splat blending** — splat sampled at global UV (0..1 across terrain), layer textures tiled at world position
- **Tonemapping** — optional ACES tonemapping in shader
- **Per-chunk draw calls** — one vertex/index buffer pair per chunk, frustum-culled by AABB
- **Headless support** — works in both `flint play` and `flint render` (snapshot)
- **Grass layer** — GPU-instanced blades placed by a compute shader from splat-map density (`grass.*` keys on the `terrain` component), with wind sway, bend-on-contact around entities, distance fade, shadow casting into the two nearest cascades, MSAA support, and a live `Grass Debug` panel (F3) that commits values back to the scene file

### Runtime Integration

- **Physics** — terrain exported as single static trimesh collider via Rapier
- **Scripting** — `terrain_height(x, z)` API exposed to Rhai scripts for gameplay logic (returns 0.0 if no terrain)
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

### Known Limitations

- **One terrain per scene** — only the first terrain entity is loaded; additional terrain entities are ignored
- **No LOD** — all chunks render at full resolution regardless of camera distance
- **No normal maps** — all splat layers share the same geometric normal
- **No triplanar mapping** — UV stretching visible on steep slopes
- **Fixed PBR parameters** — metallic and roughness are uniform across the entire terrain
- **No runtime deformation** — terrain is static after loading (editor brushes work, but not in gameplay)

## Future Features — Prioritized by Impact

### ~~Priority 1: Frustum Culling~~ Done

Chunks are tested against the camera frustum by AABB before submission (`flint-render/src/frustum.rs`); off-screen chunks cost nothing.

### Priority 2: Distance-Based LOD

**Impact: High | Effort: Medium**

Generate 2-3 index buffers per chunk at different resolutions (full, half, quarter). Select LOD level based on camera distance. The chunk grid structure already supports this — each chunk just needs multiple index buffers. T-junction stitching (skirt vertices or degenerate triangles) is needed to prevent cracks between adjacent chunks at different LOD levels.

### Priority 3: Per-Layer Normal Maps

**Impact: High | Effort: Medium**

Add `layer0_normal` through `layer3_normal` fields to the terrain component schema. Sample normal maps in the fragment shader, blend by the same splat weights already computed, and perturb the geometric normal. The bind group has room for 4 additional texture slots. This is the single biggest visual quality improvement — it adds surface detail that makes grass, rock, dirt, and sand look distinct up close.

### Priority 4: Triplanar Mapping

**Impact: Medium | Effort: Low**

Project textures along all three world axes (XZ, XY, YZ), blending by the surface normal direction. This eliminates texture stretching on cliff faces and steep slopes. Shader-only change — no CPU-side modifications needed. The normal data is already available in the fragment shader.

### Priority 5: Multi-Terrain Support

**Impact: Medium | Effort: Medium-High**

Remove the single-terrain limitation. Support multiple terrain entities per scene with independent heightmaps, splat maps, and transforms. The renderer already stores `terrain_draw_calls` as a `Vec`, so the main work is in the scene loader (loading multiple heightmaps) and physics registration (multiple trimesh colliders). Enables biome transitions, chunked open worlds, and mega-terrain via heightmap stitching.

### Later: Visual Polish

- **Detail textures** — high-frequency micro-detail overlaid at close range, fading with distance
- **Terrain normal smoothing** — weighted average of face normals for smoother lighting on coarse meshes
- **GPU-driven clipmap LOD** — continuous LOD centered on camera with seamless transitions

### Later: World Features

- **Water planes** — flat or animated water surface with reflection/refraction, foam at terrain intersection
- **Terrain holes** — alpha mask regions for cave entrances, tunnels, mine shafts
- **Decals** — projected textures (paths, scorch marks, tire tracks) on terrain surface
- **Streaming/paging** — load chunks on demand for worlds larger than memory

### Later: Integration

- **AI terrain generation** — integrate with asset generation pipeline for AI-created heightmaps and splat maps
- **NavMesh generation** — build navigation mesh from terrain + static geometry for AI pathfinding
- **Terrain-aware audio** — surface material detection for footstep sounds (grass, dirt, rock, sand)
- **Weather interaction** — snow accumulation on upward-facing surfaces, rain puddles in concavities
