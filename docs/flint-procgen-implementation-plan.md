# Flint Procgen — Implementation Plan

**Reference:** `flint-procgen-design.md`
**Task size constraint:** No task exceeds 3 days of effort.
**Estimates:** Given in ideal dev-days (1 day ≈ focused day of work by a developer or AI coding agent).

> **Progress:** Phases 1–6 complete. Phase 7.1 (crate scaffolding + MockAgent) complete. Phases 7.2–7.3 and Phase 8 are pending.

---

## Phase 1: Foundation (5 days)

The core crate structure, traits, and spec parsing. No generators yet — just the skeleton that everything plugs into.

### Task 1.1 — Crate scaffolding and core types (1 day)
- Create `flint-procgen` crate with Cargo.toml, feature flags
- Define `OutputKind`, `GeneratorOutput`, `MeshData`, `ImageData`, `BoundingBox` structs
- Define `ProcGenError` error enum
- Define `Seed` enum with `Fixed`, `Random`, `Derived` variants
- Implement `Seed::resolve(&self) -> u64` (hashing for `Derived`, thread-local RNG for `Random`)
- Unit tests for seed determinism

### Task 1.2 — Generator trait and registry (1.5 days)
- Define `Generator` trait (`type_name`, `output_kind`, `generate`, `param_schema`, `estimate_cost`)
- Define `GenerationCost` struct
- Implement `GeneratorRegistry` with `register()`, `get()`, `generate_from_spec()`
- Write a trivial `MockGenerator` that returns a hardcoded cube mesh for testing
- Tests: registry lookup, unknown generator error, generate-from-spec round trip

### Task 1.3 — Spec parsing and discovery (1.5 days)
- Define `ProcGenSpec`, `SpecMeta`, `SeedConfig` structs with serde Deserialize
- Implement TOML loading from file path
- Implement spec discovery: scan `assets/specs/*.procgen.toml`, return list of parsed specs
- Implement `spec_content_hash` (SHA256 of the canonical TOML bytes, used for cache keys)
- Tests: parse example tree spec, parse example texture spec, discovery from a temp directory, hash stability

### Task 1.4 — Seeded RNG wrapper (1 day)
- Create `SeededRng` wrapper around a deterministic PRNG (e.g., `rand_chacha::ChaCha8Rng`)
- Convenience methods: `next_f32()`, `next_f64()`, `next_range()`, `next_vec3()`, `next_color_variation()`
- Ensure identical seed → identical sequence across platforms
- Tests: determinism across multiple instantiations, statistical distribution sanity checks

---

## Phase 2: Algorithmic Building Blocks (7 days)

Shared algorithms used by multiple generators. Built and tested independently before any generator uses them.

### Task 2.1 — Noise functions (2 days)
- Implement Perlin noise (2D and 3D)
- Implement simplex noise (2D and 3D)
- Implement Worley/cellular noise (2D)
- Implement FBM (fractal Brownian motion) as a combinator over any noise function
- All functions take `&SeededRng` or a seed for determinism
- Tests: determinism, known-value snapshots, visual sanity (output a debug PNG of each noise type)

### Task 2.2 — Mesh builder utilities (2 days)
- `MeshBuilder` struct: add vertices, indices, compute normals, compute tangents
- Primitive helpers: cylinder, sphere, cone, tapered cylinder (for trunk/branches)
- Extrude-along-curve: take a cross-section and a 3D spline, produce a tube mesh
- UV mapping strategies: cylindrical, planar projection
- Mesh simplification: implement quadric edge collapse for LOD generation
- Tests: vertex/index counts for known primitives, normal correctness, LOD triangle count targets

### Task 2.3 — L-system engine (1.5 days)
- String rewriting engine: axiom + production rules → expanded string
- Turtle interpreter: F (forward), + - (yaw), ^ & (pitch), \ / (roll), [ ] (push/pop state)
- Parameterized symbols: `F(length, radius)` with length/radius decay per level
- Output: list of `BranchSegment { start, end, radius_start, radius_end, depth }`
- Tests: known L-system expansions, segment count verification, determinism

### Task 2.4 — Space colonization algorithm (1.5 days)
- Implement the space colonization algorithm for branching structures
- Input: attraction point cloud (scattered in a crown volume — sphere, ellipsoid, or hemisphere)
- Parameters: kill distance, influence distance, step size
- Output: tree graph of `BranchNode { position, radius, parent, depth }`
- Convert branch graph → list of `BranchSegment` (same format as L-system output)
- Tests: basic convergence, attraction point consumption, determinism with seed

---

## Phase 3: Tree Generator (5 days)

The first real generator. Consumes the algorithmic building blocks from Phase 2.

### Task 3.1 — Trunk generation (1.5 days)
- Generate trunk spine: vertical curve with Perlin displacement based on `trunk_curve_noise`
- Extrude tapered cylinder along spine using mesh builder
- Apply bark material properties from spec (color, roughness)
- Generate bark normal map procedurally (layered noise → normal derivation)
- Tests: mesh validity (no degenerate triangles), bounding box sanity, determinism

### Task 3.2 — Branch generation (2 days)
- Wire up L-system path: spec params → L-system rules → turtle interpretation → branch segments
- Wire up space colonization path: spec params → attraction cloud → algorithm → branch segments
- Branch segments → mesh via extrude-along-curve with taper
- Attach branches to trunk at appropriate attachment points
- Merge all branch meshes + trunk into single `MeshData`
- Tests: branch count matches spec, no mesh interpenetration at joints, both methods produce valid output

### Task 3.3 — Leaves and LOD (1.5 days)
- Billboard cluster: place camera-facing quads at branch tips, randomize orientation/scale
- Mesh cluster: instance small leaf geometry at branch tips
- Leaf color variation using `SeededRng` and spec parameters
- LOD generation: run mesh simplification at each target triangle count from spec
- Output: `Vec<MeshData>` (one per LOD level, LOD 0 = highest detail)
- Final assembly: register `TreeGenerator` with the registry, implement `param_schema()`
- Tests: LOD triangle counts within 10% of targets, leaf density correlates with spec param

---

## Phase 4: Texture Generator (4.5 days)

The second generator. Validates that the abstraction works for a fundamentally different output type.

### Task 4.1 — Pattern generators (2 days)
- Voronoi brick: Voronoi decomposition → cell IDs → mortar borders → heightfield
- Perlin organic: layered noise → heightfield with color mapping
- Tiling grid: regular grid with per-cell noise variation
- All patterns output a `PatternField` (per-pixel: cell_id, height, distance_to_edge)
- Seamless tiling support: toroidal domain wrapping
- Tests: seamless continuity at edges, determinism, visual sanity PNGs

### Task 4.2 — Map derivation (albedo, normal, roughness) (1.5 days)
- Albedo: map `PatternField` → RGBA using spec color params + per-cell variation
- Normal: Sobel filter on heightfield → tangent-space normal map
- Roughness: derive from pattern (mortar vs. surface) + detail noise overlay
- FBM detail overlay applied to all maps based on `detail_*` spec params
- Output: `Vec<ImageData>` for each requested map in `output_maps`
- Tests: normal map Z channel always positive, roughness values in 0–1 range

### Task 4.3 — Integration and registration (1 day)
- Assemble `TextureGenerator` implementing `Generator` trait
- Implement `param_schema()` returning JSON Schema for texture params
- Register with `GeneratorRegistry`
- PNG encoding for tool-time output
- End-to-end test: spec → generator → PNG files on disk, verify determinism

---

## Phase 5: CLI Integration (4 days)

Wire procgen into the `flint` CLI as the `flint gen` subcommand.

### Task 5.1 — Basic CLI (1.5 days)
- Add `gen` subcommand to `flint` CLI (clap)
- Arguments: spec path, `-o` output path, `--seed` override, `--dry-run`
- Load spec → resolve generator → run → serialize output (GLB via `gltf` crate, PNG via `image` crate)
- `--dry-run` mode: run `estimate_cost()` and print stats without generating
- Tests: CLI integration tests with example specs

### Task 5.2 — Batch generation and content store registration (1.5 days)
- `--batch N --seed-start S` → generate N variants with seeds S..S+N
- Output path template: `oak_tree_{seed}.glb`
- `--register` flag: store output in content store via `flint-asset`, write `.asset.toml` sidecar with provenance (generator, spec hash, seed, timestamp)
- Tests: batch output file count, sidecar provenance fields, content store dedup (same spec+seed = same hash)

### Task 5.3 — Validation integration (1 day)
- After generation, optionally run validation against `StyleGuide` constraints
- Check: triangle count within budget, UV coverage, roughness ranges, texture dimensions
- Report validation results to stdout (pass/warn/fail per check)
- `--validate` flag to enable, `--strict` to fail on warnings
- Tests: intentionally over-budget spec triggers warning

---

## Phase 6: Runtime Integration (4.5 days)

Wire procgen into `flint-player` so games can generate assets at load time.

### Task 6.1 — ProcGen cache (1.5 days)
- Implement `ProcGenCache` with `HashMap<(u64, u64), CachedAsset>` keyed by (spec_hash, seed)
- LRU eviction when `max_memory_bytes` exceeded
- Memory tracking: each `CachedAsset` reports its byte size
- Configuration from `Flint.toml` `[procgen]` section
- Tests: cache hit/miss, eviction order, memory budget enforcement

### Task 6.2 — Runtime resolution integration (1.5 days)
- Hook into `flint-player` asset resolution: after catalog miss, check for matching procgen spec
- Spec matching: entity's asset name → scan loaded specs for matching `meta.name`
- On match: check cache → generate if miss → return GPU-uploadable handle
- Placeholder display: if generation is deferred, return placeholder handle and swap later
- Tests: resolution priority (catalog > procgen > file fallback), placeholder swap

### Task 6.3 — Frame-budgeted generation (1.5 days)
- Generation queue: when multiple assets need procgen in a single frame, queue them
- Per-frame time budget from `runtime_max_generation_ms_per_frame`
- Process queue entries until budget exhausted, continue next frame
- Priority ordering: closer to camera = higher priority (requires camera position input)
- Eager LOD: generate all LOD levels for each asset as a single queued unit
- Tests: budget enforcement, queue drain over multiple frames, priority ordering

---

## Phase 7: AI-Assisted Layer (5 days)

The `flint-procgen-ai` crate. Tool-time only.

### Task 7.1 — Crate scaffolding and agent trait (1 day)
- Create `flint-procgen-ai` crate, depends on `flint-procgen`
- Define `ProcGenAgent` trait: `interpret_spec()`, `create_spec_from_prompt()`, `refine_spec()`
- Define `AgentConfig` (provider, model, max iterations)
- Implement `MockAgent` that returns specs with slightly randomized params (for testing)
- Tests: mock agent round-trip

### Task 7.2 — Anthropic provider implementation (2 days)
- Implement `AnthropicAgent` using the Claude API
- Prompt construction: system prompt with generator's `param_schema()` + spec format docs
- `interpret_spec()`: take existing spec, ask AI to evaluate and suggest improvements, return tuned spec
- `create_spec_from_prompt()`: take natural language description, return a complete `ProcGenSpec`
- `refine_spec()`: take spec + reference image (base64), ask AI to adjust params to better match
- Structured output: AI responds with JSON that maps to spec params
- Iteration loop: generate → validate → if issues, send validation report back to AI → re-tune (up to `max_iterations`)
- Tests: mock HTTP responses, prompt snapshot tests

### Task 7.3 — CLI integration and cost tracking (2 days)
- `--ai-assist` flag: run spec through agent before generation
- `--ai-create "description"` flag: create spec from natural language
- `--ai-refine --reference image.png` flag: refine spec against reference
- Implement `CostLedger`: append-only JSON file at `.flint/procgen_costs.json`
- Wrap all API calls in cost-tracking middleware (input/output tokens, estimated USD)
- `flint gen --cost-report`: print summary (total cost, per-provider breakdown, per-spec breakdown)
- `--max-cost` flag: abort if estimated cost exceeds threshold
- Tests: cost ledger writes, ceiling enforcement, CLI flag parsing

---

## Phase 8: Polish and Documentation (3 days)

### Task 8.1 — Example specs and documentation (1.5 days)
- Write 3–4 example specs: oak tree, pine tree, stone wall texture, wood plank texture
- Write `flint-procgen/README.md` covering: concepts, spec format reference, CLI usage, runtime integration
- Write `CONTRIBUTING.md` section on adding new generators
- Document the `Generator` trait contract and `param_schema()` expectations

### Task 8.2 — Integration tests and hardening (1.5 days)
- End-to-end integration test: spec → CLI → content store → runtime resolution → cache hit
- Cross-generator determinism sweep: generate 100 variants, regenerate, diff outputs
- Edge cases: empty spec params, zero seed, extremely high LOD targets, 1x1 texture
- Performance benchmarks: generation time for reference specs at various complexity levels
- CI integration: add procgen tests to Flint's test suite

---

## Summary

| Phase | Description | Days | Cumulative |
|-------|-------------|------|------------|
| 1 | Foundation | 5 | 5 |
| 2 | Algorithmic Building Blocks | 7 | 12 |
| 3 | Tree Generator | 5 | 17 |
| 4 | Texture Generator | 4.5 | 21.5 |
| 5 | CLI Integration | 4 | 25.5 |
| 6 | Runtime Integration | 4.5 | 30 |
| 7 | AI-Assisted Layer | 5 | 35 |
| 8 | Polish and Documentation | 3 | 38 |

**Total estimated effort: ~38 dev-days**

### Recommended parallelism

- Phases 1–4 are sequential (each builds on the last).
- Phase 5 (CLI) and Phase 6 (Runtime) can be parallelized once Phase 4 is complete.
- Phase 7 (AI layer) can begin once Phase 5 is complete (needs CLI hooks).
- Phase 8 can overlap with Phase 7.

### Critical path

Phases 1 → 2 → 3 → 4 → 6 (runtime integration) — this is the shortest path to a working runtime demo.

### Suggested first milestone

After Phase 5 (Task 5.1), you'll be able to run:
```bash
flint gen specs/oak_tree.procgen.toml -o tree.glb
```
and open the result in a 3D viewer. That's a satisfying checkpoint at ~25.5 days.
