# Adding a generator

A generator is one Rust type that turns a spec's `[params]` and a seeded RNG into an asset. This guide builds a complete one, `pebbles_v1`, an image of scattered round pebbles. You can follow it without reading the built-in generators first. The code blocks here are compiled and run by `cargo test -p flint-procgen --doc`, so they work as written.

The checklist, before the detail:

1. Implement [`Generator`](src/generator.rs): `type_name`, `output_kind`, `generate`, `param_schema`, `estimate_cost`.
2. Parse `[params]` into a typed struct with defaults, and refuse bad values with `ProcGenError::InvalidParameter`.
3. Draw every random number from the `SeededRng` you're given, forked per stage.
4. Write a schema that describes every key: type, range, default, `description`, and `x-when` for keys only one mode reads.
5. Register it in `register_built_in_generators`.
6. Add tests: determinism, a nudge test that holds the schema to the code, edge cases.
7. Add an example spec, regenerate the README catalog, and add a section to the book.

## 1. The type and its parameters

Keep parameters in a plain struct with a `Default`, filled from the spec's table. A missing key takes its default; a bad one is an error that names the key and says why, never a panic and never a silent clamp. The helpers in `flint_procgen::generators::util` (`toml_f32`, `toml_u32`, `toml_string`, `parse_hex_color`) give consistent messages.

```rust
use flint_procgen::generators::util::{parse_hex_color, toml_f32, toml_u32};
use flint_procgen::{
    ChannelSemantics, GenerationCost, Generator, GeneratorOutput, ImageData, OutputKind,
    ProcGenError, ProcGenSpec, Result, SeededRng,
};

/// Scattered round pebbles on a flat ground colour.
pub struct PebblesGenerator;

struct PebbleParams {
    size: u32,
    count: u32,
    radius: f32,
    ground: [f32; 4],
    pebble: [f32; 4],
}

impl Default for PebbleParams {
    fn default() -> Self {
        Self {
            size: 128,
            count: 40,
            radius: 0.04,
            ground: parse_hex_color("#4A4036").unwrap(),
            pebble: parse_hex_color("#A39886").unwrap(),
        }
    }
}

impl PebbleParams {
    fn from_toml(params: &toml::Value) -> Result<Self> {
        let mut p = Self::default();
        let Some(t) = params.as_table() else {
            return Ok(p);
        };
        if let Some(v) = t.get("size") {
            p.size = toml_u32(v, "size")?;
            if !(8..=4096).contains(&p.size) {
                return Err(ProcGenError::InvalidParameter {
                    name: "size".into(),
                    reason: "must be 8 to 4096".into(),
                });
            }
        }
        if let Some(v) = t.get("count") {
            p.count = toml_u32(v, "count")?;
        }
        if let Some(v) = t.get("radius") {
            p.radius = toml_f32(v, "radius")?;
            if p.radius <= 0.0 {
                return Err(ProcGenError::InvalidParameter {
                    name: "radius".into(),
                    reason: "must be positive".into(),
                });
            }
        }
        if let Some(v) = t.get("ground_color") {
            p.ground = parse_hex_color(v.as_str().unwrap_or(""))?;
        }
        if let Some(v) = t.get("pebble_color") {
            p.pebble = parse_hex_color(v.as_str().unwrap_or(""))?;
        }
        Ok(p)
    }
}
```

`parse_hex_color` returns *linear* RGBA, the space to do arithmetic in. Convert back to sRGB only when writing pixels (step 2).

## 2. `generate`: deterministic by construction

The RNG is the only source of randomness. Fork it once per stage with a fixed label (`rng.fork("placement")`), so adding a stage later doesn't shift the numbers an earlier one draws, and every existing asset stays the same. Never use `rand::thread_rng`, clocks, `HashMap` iteration order or anything that differs between runs or machines. Generators don't touch the filesystem either: specs arrive parsed, and the CLI and player write the output.

```rust
# use flint_procgen::generators::util::{parse_hex_color, toml_f32, toml_u32};
# use flint_procgen::{ChannelSemantics, GenerationCost, Generator, GeneratorOutput, ImageData,
#     OutputKind, ProcGenError, ProcGenSpec, Result, SeededRng};
# pub struct PebblesGenerator;
# struct PebbleParams { size: u32, count: u32, radius: f32, ground: [f32; 4], pebble: [f32; 4] }
# impl PebbleParams { fn from_toml(_: &toml::Value) -> Result<Self> { Ok(Self { size: 64,
#     count: 10, radius: 0.05, ground: [0.1; 4], pebble: [0.5; 4] }) } }
fn to_srgb_u8(linear: f32) -> u8 {
    let c = linear.clamp(0.0, 1.0);
    let s = if c <= 0.003_130_8 { c * 12.92 } else { 1.055 * c.powf(1.0 / 2.4) - 0.055 };
    (s * 255.0).round() as u8
}

impl PebblesGenerator {
    fn paint(&self, p: &PebbleParams, rng: &mut SeededRng) -> ImageData {
        let mut placement = rng.fork("placement");
        let pebbles: Vec<(f32, f32, f32)> = (0..p.count)
            .map(|_| {
                let r = p.radius * placement.next_range(0.6, 1.4);
                (placement.next_f32(), placement.next_f32(), r)
            })
            .collect();

        let mut image = ImageData::new(p.size, p.size, ChannelSemantics::Color);
        for y in 0..p.size {
            for x in 0..p.size {
                let (u, v) = ((x as f32 + 0.5) / p.size as f32, (y as f32 + 0.5) / p.size as f32);
                // Distances wrap, so the texture tiles.
                let on_pebble = pebbles.iter().any(|&(px, py, r)| {
                    let dx = (u - px).abs().min(1.0 - (u - px).abs());
                    let dy = (v - py).abs().min(1.0 - (v - py).abs());
                    dx * dx + dy * dy < r * r
                });
                let c = if on_pebble { p.pebble } else { p.ground };
                let i = ((y * p.size + x) * 4) as usize;
                for ch in 0..3 {
                    image.pixels[i + ch] = to_srgb_u8(c[ch]);
                }
                image.pixels[i + 3] = 255;
            }
        }
        image
    }
}
```

## 3. The `Generator` impl and its schema

`param_schema` returns a JSON Schema for `[params]`, and it does more than document. The previewer builds its panel from it, `flint gen` checks specs against it (`lint_params`), and the README catalog is printed from it. Hold it to these rules:

- **List every key the code reads**, and close the object with `"additionalProperties": false`, so a typo is reported instead of ignored.
- **Give every key a `description`.** One sentence on what it does, with units, written for someone tuning it. The previewer shows it on hover, and a test fails without it (`every_built_in_parameter_is_described`).
- **Defaults and ranges match the code.** `default` is what `Default` holds; `minimum`/`maximum` (or `exclusiveMinimum`) are what `from_toml` accepts.
- **Colours are strings with pattern `^#?[0-9a-fA-F]{6,8}$`.** The previewer gives them a colour picker.
- **Mode-specific keys get `"x-when"`.** If a key is read only for some values of another key, say so: `"x-when": { "pattern": ["voronoi_brick"] }`. Editors hide the key under other modes, and `flint gen` warns when a spec sets it there. `tree_v1` and `texture_v1` both do this.
- **Retire a key with `"deprecated": true`** rather than deleting it, so old specs get a warning that explains instead of an "unknown key".

The name carries a version (`pebbles_v1`). If you change what a spec produces, every stored asset made from it changes too, so a breaking change is a new name (`pebbles_v2`), registered beside the old one.

```rust
# use flint_procgen::generators::util::{parse_hex_color, toml_f32, toml_u32};
# use flint_procgen::{ChannelSemantics, GenerationCost, Generator, GeneratorOutput, ImageData,
#     OutputKind, ProcGenError, ProcGenSpec, Result, SeededRng};
# pub struct PebblesGenerator;
# struct PebbleParams { size: u32, count: u32, radius: f32, ground: [f32; 4], pebble: [f32; 4] }
# impl PebbleParams { fn from_toml(_: &toml::Value) -> Result<Self> { Ok(Self { size: 64,
#     count: 10, radius: 0.05, ground: [0.1; 4], pebble: [0.5; 4] }) } }
# impl PebblesGenerator { fn paint(&self, p: &PebbleParams, _: &mut SeededRng) -> ImageData {
#     ImageData::new(p.size, p.size, ChannelSemantics::Color) } }
impl Generator for PebblesGenerator {
    fn type_name(&self) -> &str {
        "pebbles_v1"
    }

    fn output_kind(&self) -> OutputKind {
        OutputKind::Image
    }

    fn generate(&self, spec: &ProcGenSpec, rng: &mut SeededRng) -> Result<GeneratorOutput> {
        let params = PebbleParams::from_toml(&spec.params)?;
        Ok(GeneratorOutput::Image(self.paint(&params, rng)))
    }

    fn param_schema(&self) -> serde_json::Value {
        let hex = "^#?[0-9a-fA-F]{6,8}$";
        serde_json::json!({
            "type": "object",
            "properties": {
                "size": { "type": "integer", "minimum": 8, "maximum": 4096, "default": 128,
                    "description": "Side of the square image, pixels" },
                "count": { "type": "integer", "minimum": 0, "default": 40,
                    "description": "Pebbles scattered over the image" },
                "radius": { "type": "number", "exclusiveMinimum": 0.0, "default": 0.04,
                    "description": "Typical pebble radius as a fraction of the image; each varies ±40%" },
                "ground_color": { "type": "string", "pattern": hex, "default": "#4A4036",
                    "description": "Colour between the pebbles, sRGB hex" },
                "pebble_color": { "type": "string", "pattern": hex, "default": "#A39886",
                    "description": "Pebble colour, sRGB hex" }
            },
            "additionalProperties": false
        })
    }

    /// Cheap and rough is fine: `flint gen --dry-run` shows it, and the
    /// player's queue uses it to budget a frame. Never run the generator here.
    fn estimate_cost(&self, spec: &ProcGenSpec) -> GenerationCost {
        let p = PebbleParams::from_toml(&spec.params).unwrap_or_else(|_| PebbleParams {
            size: 128, count: 40, radius: 0.04, ground: [0.0; 4], pebble: [0.0; 4],
        });
        let pixels = (p.size as u64).pow(2);
        GenerationCost {
            estimated_texture_bytes: pixels * 4,
            estimated_generation_ms: pixels as f64 * p.count as f64 * 2e-6,
            ..Default::default()
        }
    }
}
```

## 4. Register it

Built-in generators are registered in one place, `register_built_in_generators` in [`src/generators/mod.rs`](src/generators/mod.rs). The CLI (`flint gen`, `flint edit`), the player's resolver and `flint-procgen-ai` all build their registries from it, so one line there makes the generator available everywhere:

```rust,ignore
registry.register(Box::new(pebbles::PebblesGenerator));
```

A registry of your own, for tests or tools, takes it the same way:

```rust
# use flint_procgen::generators::util::{parse_hex_color, toml_f32, toml_u32};
# use flint_procgen::{ChannelSemantics, GenerationCost, Generator, GeneratorOutput, ImageData,
#     OutputKind, ProcGenError, ProcGenSpec, Result, SeededRng};
# pub struct PebblesGenerator;
# impl Generator for PebblesGenerator {
#     fn type_name(&self) -> &str { "pebbles_v1" }
#     fn output_kind(&self) -> OutputKind { OutputKind::Image }
#     fn generate(&self, _: &ProcGenSpec, rng: &mut SeededRng) -> Result<GeneratorOutput> {
#         let mut img = ImageData::new(8, 8, ChannelSemantics::Color);
#         img.pixels[0] = (rng.fork("placement").next_f32() * 255.0) as u8;
#         Ok(GeneratorOutput::Image(img)) }
#     fn param_schema(&self) -> serde_json::Value { serde_json::json!({ "type": "object",
#         "properties": { "count": { "type": "integer", "description": "Pebbles" } },
#         "additionalProperties": false }) }
# }
use flint_procgen::{lint_params, GeneratorRegistry};

let mut registry = GeneratorRegistry::new();
registry.register(Box::new(PebblesGenerator));

let spec = ProcGenSpec::parse(r#"
    generator = "pebbles_v1"
    [meta]
    name = "river_bed"
    version = "1.0.0"
    [seed]
    mode = "fixed"
    value = 9
    [params]
    count = 60
"#)?;

// The spec uses only keys the schema knows...
let schema = registry.get("pebbles_v1").unwrap().param_schema();
assert!(lint_params(&schema, &spec.params).is_empty());
// ...and the same seed gives the same image.
let a = registry.generate_from_spec(&spec)?;
let b = registry.generate_from_spec(&spec)?;
assert_eq!(a, b);
# Ok::<(), ProcGenError>(())
```

## 5. Tests

Put unit tests in the generator's module. Every generator needs at least:

- **Determinism.** Generate twice from one spec and seed and assert the outputs are equal (`GeneratorOutput` is `PartialEq`). Then a different seed gives a different output.
- **A nudge test for the schema.** For every mode and every key: change the value a little, regenerate, and assert "the output changed" equals "the schema says this key applies here". This catches keys the code reads but the schema omits, `x-when` conditions that are wrong, and knobs that do nothing. Nudge integers *down* where you can, because "up" may run into a cap and change nothing. See `schema_says_exactly_which_keys_each_pattern_reads` (texture) and `tree_schema_says_which_method_reads_each_key` (tree). The tree's found `branch_levels`, a key parsed and validated for months that nothing read.
- **Edge cases.** Empty `[params]` (all defaults), zero counts, the smallest and largest sizes, and every invalid value `from_toml` refuses. Each one is an `Err` naming the key, never a panic.
- **Output validity.** `MeshData::validate()` / `ImageData::validate()` pass. Meshes have no degenerate triangles and normals are unit length. Images are exactly `width × height × 4` bytes.

The crate-wide tests then cover the rest once the generator is registered: `every_built_in_parameter_is_described`, `shipped_specs_lint_clean` (every spec in the repo), and `tests/examples.rs` (the example specs and this README's catalog).

```bash
cargo test -p flint-procgen                      # everything, including these doc examples
cargo test -p flint-procgen pebbles              # just yours
```

## 6. Docs and an example spec

- Add a commented spec to `specs/examples/`, in the style of the others: a header that says what the asset is and how the generator thinks, a comment on every non-obvious key, and the `flint gen` line that makes it. `tests/examples.rs` generates every spec there with `--strict` semantics.
- Regenerate the README catalog: `cargo run -p flint-procgen --example param_catalog`, pasted between the `catalog` markers. The test prints the difference if you forget.
- Add a section to the book's [procgen page](../../docs/book/src/concepts/procgen.md), and check it builds: `mdbook build docs/book`.

## Style

Follow the tree and texture generators:

- Module per generator under `src/generators/<name>/`, with `mod.rs` holding the `Generator` impl and the params struct, and the stages in sibling files.
- Algorithms another generator could use (noise, L-systems, mesh building) go in `src/algorithms/`, not in the generator.
- Doc comments say what a value *means* (units, ranges, what 0 and 1 do). The schema descriptions are those comments, shortened.
- Generators are stateless (`Send + Sync`), and they never read files, clocks or globals. They must build for `wasm32` too: the web player runs them (`bash scripts/wasm-check.sh`).
- Run `rustfmt` on the files you changed only. `cargo fmt -p flint-procgen` reformats unrelated files.
