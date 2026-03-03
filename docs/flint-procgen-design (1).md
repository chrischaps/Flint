# Flint Procedural Generation Subsystem — Design Document

**Status:** Draft v1
**Scope:** Architecture for `flint-procgen`, a procedural asset generation subsystem for the Flint game engine
**V1 Asset Types:** Trees (3D mesh) and Textures (2D image)

---

## 1. Motivation

Flint needs a way to produce game assets algorithmically — both at tool-time (developer runs a CLI command to bake assets into the project) and at runtime (the game generates assets on launch or during play from a compact spec). This enables:

- **Variety without storage cost.** A single tree spec + 100 seeds = 100 unique trees, stored as one small TOML file rather than 100 GLB meshes.
- **Determinism when desired.** A fixed seed always reproduces the same asset, enabling reproducible worlds without shipping every generated file.
- **Controlled randomness when desired.** Omit the seed (or pass `seed = "random"`) and get a fresh variant every time.
- **AI-assisted authoring at tool-time.** An agentic AI layer can interpret high-level specs, reason about aesthetic intent, and tune algorithmic parameters — without requiring external API calls at runtime.

---

## 2. Relationship to Existing Asset Pipeline

The procedural generation subsystem is designed to complement, not replace, the existing Flint asset pipeline:

```
┌─────────────────────────────────────────────────────────────────────┐
│                        Flint Asset Ecosystem                        │
│                                                                     │
│  flint-asset          Content store, catalog, AssetRef resolution   │
│  flint-asset-gen      AI provider-based generation (Flux, Meshy…)   │
│  flint-procgen  ←NEW  Algorithmic procedural generation             │
│  flint-player         Runtime resolution + fallback                 │
└─────────────────────────────────────────────────────────────────────┘
```

### Integration Points

**Tool-time outputs feed the content store.** When `flint gen` produces a GLB or PNG, it stores the result in `.flint/assets/` via the content store and writes an `.asset.toml` sidecar. From that point, the asset is indistinguishable from a hand-authored one. The sidecar records provenance:

```toml
[provenance]
generator = "tree_v1"
spec = "specs/oak_tree.procgen.toml"
spec_hash = "sha256:9f3a..."
seed = 42
generated_at = "2026-03-01T12:00:00Z"
```

**Runtime generation bypasses the content store.** Assets generated at runtime are ephemeral — they live in an in-memory cache keyed by `(spec_hash, seed)`. The runtime resolution flow becomes:

```
entity needs "oak_tree"
  → catalog lookup → found? → use content store path ✓
  → not found, but has procgen spec? → generate in-memory, cache, return handle ✓
  → fallback file search ✓
```

**`flint-asset-gen` can delegate to `flint-procgen`.** The existing `BatchStrategy` enum gains a new variant:

```rust
enum BatchStrategy {
    AiGenerate,
    HumanTask,
    AiThenHuman,
    Procedural,       // ← delegates to flint-procgen
    ProceduralThenAi, // ← procgen for base, AI for refinement
}
```

---

## 3. Crate Architecture

### `flint-procgen`

A standalone crate with no network dependencies. Can be compiled into the runtime binary.

```
flint-procgen/
├── src/
│   ├── lib.rs              // Public API surface
│   ├── registry.rs         // GeneratorRegistry — maps type names to generators
│   ├── spec.rs             // ProcGenSpec — deserialized from TOML
│   ├── seed.rs             // Seed handling (fixed, random, derived)
│   ├── output.rs           // GeneratorOutput enum (Mesh, Image, Audio…)
│   ├── traits.rs           // Generator trait
│   ├── generators/
│   │   ├── mod.rs
│   │   ├── tree.rs         // TreeGenerator (V1)
│   │   └── texture.rs      // TextureGenerator (V1)
│   ├── algorithms/         // Shared algorithmic building blocks
│   │   ├── mod.rs
│   │   ├── noise.rs        // Perlin, simplex, Worley, etc.
│   │   ├── lsystem.rs      // L-system string rewriting + turtle interpretation
│   │   ├── space_colonization.rs  // Space colonization for branching
│   │   └── mesh_builder.rs // Vertex/index buffer construction, UV mapping
│   └── util/
│       ├── color.rs        // Color spaces, palette generation
│       └── rng.rs          // Seeded RNG wrapper (deterministic)
├── tests/
│   ├── determinism.rs      // Same spec + seed = identical output
│   ├── tree_tests.rs
│   └── texture_tests.rs
└── Cargo.toml
```

### `flint-procgen-ai` (optional, tool-time only)

A separate crate that depends on `flint-procgen` and adds the agentic AI layer. Not compiled into the game runtime.

```
flint-procgen-ai/
├── src/
│   ├── lib.rs
│   ├── agent.rs            // Agentic interface — takes a spec, reasons about it,
│   │                       //   returns tuned algorithmic parameters
│   ├── providers/
│   │   ├── mod.rs
│   │   ├── anthropic.rs    // Claude API for spec interpretation
│   │   └── mock.rs         // Deterministic mock for testing
│   ├── cost.rs             // Usage tracking / cost ledger
│   └── prompt_templates/   // Prompt templates for different generator types
│       ├── tree.md
│       └── texture.md
└── Cargo.toml
```

---

## 4. Core Traits and Types

### Generator Trait

```rust
/// A procedural generator that produces assets from a typed parameter set.
pub trait Generator: Send + Sync {
    /// Unique identifier for this generator type (e.g., "tree_v1", "texture_v1").
    fn type_name(&self) -> &str;

    /// The kind of asset this generator produces.
    fn output_kind(&self) -> OutputKind;

    /// Generate an asset from raw TOML parameters and a seed.
    /// The generator is responsible for deserializing `params` into its
    /// own strongly-typed config.
    fn generate(&self, params: &toml::Value, seed: u64) -> Result<GeneratorOutput, ProcGenError>;

    /// Returns a JSON Schema describing the expected parameters.
    /// Used by the AI agent layer to understand what it can tune.
    fn param_schema(&self) -> serde_json::Value;

    /// Optional: estimate generation cost/time for a given param set.
    /// Useful for runtime budgeting.
    fn estimate_cost(&self, params: &toml::Value) -> GenerationCost {
        GenerationCost::default()
    }
}
```

### Key Types

```rust
/// What the generator produces.
pub enum OutputKind {
    Mesh,    // → GLB/glTF
    Image,   // → PNG
    Audio,   // → OGG (future)
}

/// The concrete output of a generation pass.
pub enum GeneratorOutput {
    Mesh(MeshData),
    Image(ImageData),
    Audio(AudioData),
}

/// Mesh data ready for serialization to GLB or direct GPU upload.
pub struct MeshData {
    pub vertices: Vec<Vertex>,
    pub indices: Vec<u32>,
    pub materials: Vec<MaterialData>,
    pub bounding_box: BoundingBox,
}

/// Image data ready for PNG encoding or direct texture upload.
pub struct ImageData {
    pub pixels: Vec<u8>,   // RGBA
    pub width: u32,
    pub height: u32,
    pub channel_semantics: ChannelSemantics, // Color, Normal, Roughness, etc.
}

/// Seed handling.
pub enum Seed {
    Fixed(u64),
    Random,                // Resolved to a random u64 at generation time
    Derived(String),       // Hash of a string → u64 (e.g., "world_42_chunk_7_tree_3")
}

/// Cost estimate for runtime budgeting.
pub struct GenerationCost {
    pub estimated_ms: f64,
    pub estimated_triangles: Option<u32>,  // For mesh generators
    pub estimated_pixels: Option<u32>,     // For image generators
}
```

### Generator Registry

```rust
pub struct GeneratorRegistry {
    generators: HashMap<String, Arc<dyn Generator>>,
}

impl GeneratorRegistry {
    pub fn new() -> Self { /* ... */ }

    pub fn register(&mut self, generator: Arc<dyn Generator>) {
        self.generators.insert(generator.type_name().to_string(), generator);
    }

    pub fn get(&self, type_name: &str) -> Option<&Arc<dyn Generator>> {
        self.generators.get(type_name)
    }

    /// Convenience: load a spec, resolve the generator, run it.
    pub fn generate_from_spec(&self, spec: &ProcGenSpec) -> Result<GeneratorOutput, ProcGenError> {
        let gen = self.get(&spec.generator)
            .ok_or(ProcGenError::UnknownGenerator(spec.generator.clone()))?;
        let seed = spec.seed.resolve();
        gen.generate(&spec.params, seed)
    }
}
```

---

## 5. Spec Format

All specs use `.procgen.toml` extension for easy discovery and editor association.

### Tree Spec Example

```toml
# specs/oak_tree.procgen.toml

[meta]
name = "oak_tree"
generator = "tree_v1"
version = "1.0"

[seed]
mode = "fixed"   # "fixed" | "random" | "derived"
value = 42

[params]
# Trunk
trunk_height = 4.0          # meters
trunk_radius_base = 0.3
trunk_radius_top = 0.15
trunk_segments = 8
trunk_curve_noise = 0.05    # How much the trunk wanders

# Branching
branch_method = "space_colonization"  # "lsystem" | "space_colonization"
branch_levels = 3
branch_angle_min = 25.0     # degrees
branch_angle_max = 55.0
branch_length_falloff = 0.7 # Each level is 70% of parent
branch_count_per_level = [5, 4, 3]

# Canopy / Leaves
leaf_style = "billboard_cluster"  # "billboard_cluster" | "mesh_cluster" | "none"
leaf_density = 0.8           # 0.0–1.0
leaf_color_base = "#4a7c3f"
leaf_color_variation = 0.15

# LOD
lod_levels = 3               # Number of LOD variants to generate
lod_triangle_targets = [5000, 1500, 400]

# Material
bark_roughness = 0.85
bark_color_base = "#5c4033"
bark_normal_strength = 1.0
```

### Texture Spec Example

```toml
# specs/stone_wall.procgen.toml

[meta]
name = "stone_wall"
generator = "texture_v1"
version = "1.0"

[seed]
mode = "random"

[params]
width = 1024
height = 1024
output_maps = ["albedo", "normal", "roughness"]  # Which maps to generate

# Pattern
pattern = "voronoi_brick"     # "voronoi_brick" | "perlin_organic" | "tiling_grid" | …
cell_count = 12               # Approximate number of stones
mortar_width = 0.02
mortar_color = "#8a8070"

# Surface
base_color = "#9e9585"
color_variation = 0.2
roughness_base = 0.75
roughness_variation = 0.15

# Detail
detail_noise = "fbm"          # Fractal brownian motion layered on top
detail_scale = 4.0
detail_strength = 0.3

# Seamless
seamless = true               # Ensure tileable output
```

### Spec Deserialization

```rust
#[derive(Debug, Deserialize)]
pub struct ProcGenSpec {
    pub meta: SpecMeta,
    pub seed: SeedConfig,
    pub params: toml::Value, // Generator-specific, deserialized by the generator itself
}

#[derive(Debug, Deserialize)]
pub struct SpecMeta {
    pub name: String,
    pub generator: String,
    pub version: String,
}

#[derive(Debug, Deserialize)]
pub struct SeedConfig {
    pub mode: SeedMode,
    pub value: Option<u64>,
    pub derive_from: Option<String>,
}
```

---

## 6. Runtime Flow

```
Game Launch
  │
  ├─ Load ProcGenSpec files from assets/specs/*.procgen.toml
  ├─ Initialize GeneratorRegistry with built-in generators
  │
  ▼
Scene Loading
  │
  ├─ Entity references asset "oak_tree_variant"
  ├─ Catalog lookup → miss (no baked asset)
  ├─ ProcGen spec found for "oak_tree_variant"?
  │     │
  │     ├─ YES → Check runtime cache (spec_hash, seed)
  │     │         ├─ Cache hit → return handle
  │     │         └─ Cache miss → registry.generate_from_spec(spec)
  │     │                          → cache result → return handle
  │     │
  │     └─ NO → Fall through to file-based fallback
  │
  ▼
Rendering
  │
  └─ GeneratorOutput::Mesh → upload to GPU as normal mesh
     GeneratorOutput::Image → upload to GPU as normal texture
```

### Runtime Cache

```rust
pub struct ProcGenCache {
    /// Cache key is (spec_content_hash, seed).
    /// Value is the generated output + GPU resource handle.
    entries: HashMap<(u64, u64), CachedAsset>,
    budget: CacheBudget,
}

pub struct CacheBudget {
    pub max_memory_bytes: usize,
    pub max_generation_ms_per_frame: f64,  // Spread generation across frames
}
```

Generation can be async/deferred — if an asset isn't ready yet, the renderer can show a placeholder and swap in the generated asset once it's done. This avoids frame hitches.

---

## 7. Tool-Time Flow (CLI)

### Basic CLI

```bash
# Generate a single asset from a spec
flint gen specs/oak_tree.procgen.toml -o assets/models/oak_tree.glb

# Generate with a specific seed override
flint gen specs/oak_tree.procgen.toml --seed 99 -o oak_tree_99.glb

# Batch generate: 10 variants with sequential seeds
flint gen specs/oak_tree.procgen.toml --batch 10 --seed-start 0 -o assets/models/oak_tree_{seed}.glb

# Generate and register in the content store (writes .asset.toml sidecar)
flint gen specs/oak_tree.procgen.toml --register

# Preview without writing (prints stats: triangle count, texture size, estimated quality)
flint gen specs/oak_tree.procgen.toml --dry-run
```

### Tool-Time with AI Assistance

```bash
# AI interprets the spec and suggests parameter tuning
flint gen specs/oak_tree.procgen.toml --ai-assist

# AI generates a spec from a natural language description
flint gen --ai-create "a gnarled dead oak tree with no leaves, spooky, ~3000 tris" -o dead_oak.glb

# AI refines an existing spec to better match a reference image
flint gen specs/oak_tree.procgen.toml --ai-refine --reference ref_photo.png
```

### AI-Assisted Pipeline (Tool-Time Only)

```
Developer provides spec (or natural language prompt)
  │
  ▼
AI Agent (flint-procgen-ai)
  │
  ├─ Reads the spec + generator's param_schema()
  ├─ Reasons about aesthetic intent, structural constraints
  ├─ Produces refined/tuned parameter values
  │   (The AI's output is always a valid ProcGenSpec — it tunes
  │    algorithmic parameters, not raw pixels/vertices)
  │
  ▼
Algorithmic Generator (flint-procgen)
  │
  ├─ Runs with AI-tuned parameters
  ├─ Produces deterministic output
  │
  ▼
Validation (optional)
  │
  ├─ Check against StyleGuide constraints (tri count, UV coverage, roughness ranges)
  ├─ If validation fails, AI can iterate on parameters
  │
  ▼
Output → Content store + .asset.toml sidecar
```

The key insight: **the AI never directly produces geometry or pixels.** It reads the schema, understands what parameters are available, and produces better parameter values. This keeps the pipeline deterministic and auditable while leveraging AI for the "creative direction" layer.

---

## 8. V1 Generator Walkthroughs

### Tree Generator (`tree_v1`)

**Algorithm overview:**

1. **Trunk generation.** Extrude a cylinder along a noisy curve (Perlin-displaced spine). Taper radius from base to top. Segment count from spec.

2. **Branch generation.** Two methods available:
   - *L-system:* Classic string rewriting → turtle graphics interpretation → branch skeleton. Good for stylized/regular trees.
   - *Space colonization:* Scatter attraction points in a crown volume, grow branches toward them. Better for natural-looking, organic canopies.

3. **Bark material.** Generate a procedural bark texture (layered Perlin noise) or reference an external texture asset. Apply as material on the mesh.

4. **Leaf clusters.** Based on `leaf_style`:
   - *billboard_cluster:* Place camera-facing quads at branch tips with a leaf texture atlas.
   - *mesh_cluster:* Small low-poly leaf geometry instanced at branch tips.

5. **LOD generation.** Run a mesh simplification pass (quadric edge collapse) for each LOD target triangle count.

6. **Output.** `MeshData` with vertices, indices, materials, and bounding box. Serialized to GLB for tool-time, uploaded to GPU for runtime.

### Texture Generator (`texture_v1`)

**Algorithm overview:**

1. **Base pattern.** Select algorithm by `pattern`:
   - *voronoi_brick:* Voronoi cell decomposition with mortar borders. Good for stone, brick, tile.
   - *perlin_organic:* Layered Perlin/simplex noise. Good for dirt, wood grain, clouds.
   - *tiling_grid:* Regular grid with per-cell variation. Good for panels, floors.

2. **Color mapping.** Map pattern output to color using `base_color` + `color_variation`. Each cell/region gets a slightly different hue/value shift.

3. **Detail overlay.** Layer FBM noise on top for surface micro-detail (scratches, dust, weathering).

4. **Normal map generation.** Derive normals from the heightfield (Sobel filter on the combined noise layers).

5. **Roughness map.** Derive from pattern (mortar = smoother, stone faces = rougher) + detail noise.

6. **Seamless tiling.** If `seamless = true`, use domain warping to blend edges or generate in a toroidal domain.

7. **Output.** `ImageData` for each requested map (albedo, normal, roughness). Serialized to PNG for tool-time, uploaded as textures for runtime.

---

## 9. Cost Tracking (Tool-Time)

For AI-assisted generation, track costs in a local ledger:

```rust
pub struct CostLedger {
    entries: Vec<CostEntry>,
    path: PathBuf,  // e.g., .flint/procgen_costs.json
}

pub struct CostEntry {
    pub timestamp: DateTime<Utc>,
    pub provider: String,           // "anthropic", "openai", etc.
    pub operation: String,          // "spec_interpretation", "param_tuning"
    pub input_tokens: u32,
    pub output_tokens: u32,
    pub estimated_cost_usd: f64,
    pub spec_name: String,
    pub duration_ms: u64,
}
```

CLI integration:

```bash
# Show cost summary
flint gen --cost-report

# Set a cost ceiling per generation
flint gen specs/oak_tree.procgen.toml --ai-assist --max-cost 0.10
```

---

## 10. Configuration

Global procgen configuration lives in `Flint.toml` (the engine's project config):

```toml
[procgen]
# Runtime settings
runtime_enabled = true
runtime_cache_mb = 256
runtime_max_generation_ms_per_frame = 5.0
placeholder_asset = "assets/placeholder.glb"  # Shown while generating

# Tool-time settings
default_output_dir = "assets/generated"
auto_register = true           # Automatically register in content store

[procgen.ai]
enabled = true
provider = "anthropic"
model = "claude-sonnet-4-5-20250929"
max_iterations = 3             # Max AI refinement loops
cost_ceiling_usd = 1.00        # Per-session ceiling
```

---

## 11. Future Considerations (Post-V1)

These are explicitly **out of scope** for V1 but worth tracking:

- **Additional generators:** Buildings (modular assembly), props (shape grammar), terrain chunks, audio (synthesis), particle effect parameters.
- **External AI image generation:** Adding Flux/DALL-E as an optional tool-time step that produces a texture which is then analyzed and parameterized into an algorithmic spec.
- **Streaming/progressive generation:** Generate coarse LOD first, refine in background. Especially useful for open-world scenarios.
- **Spec inheritance:** A "dead_oak" spec that inherits from "oak_tree" and overrides `leaf_density = 0.0`, `trunk_curve_noise = 0.15`.
- **World-level orchestration:** A biome spec that says "this region has 60% oak, 30% pine, 10% birch" and drives procedural placement + generation together.
- **Collaborative spec editing:** AI agent proposes spec changes, developer reviews diffs, iterative refinement loop with visual preview.
- **GPU-accelerated generation:** Compute shaders for noise evaluation and mesh generation. Particularly valuable for runtime texture generation.

---

## 12. Design Decisions (Resolved)

1. **Runtime resolution priority.** Catalog → procgen → file fallback. Procgen is intentional; file fallback is a convenience.

2. **Spec discovery.** Specs live in `assets/specs/*.procgen.toml`, co-located with the rest of the asset tree.

3. **Asset registry integration depth.** Runtime-generated assets are **not** queryable via `AssetRef::by_name()` for now. They use a distinct resolution path through the procgen cache. This avoids adding complexity to the catalog and can be revisited once usage patterns are clearer.

4. **LOD strategy at runtime.** All LOD levels are generated eagerly at load time. Simpler to reason about, avoids pop-in from lazy generation, and the cost is bounded by the spec's `lod_levels` count.

5. **Validation at runtime.** No runtime validation. Validation is purely a tool-time concern. Specs are assumed correct at runtime; bad specs are caught during `flint gen` with `--dry-run` or the AI-assisted pipeline's validation step.
