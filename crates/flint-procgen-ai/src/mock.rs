//! [`MockAgent`]: a local, deterministic stand-in for a real AI agent.
//!
//! It knows nothing about language beyond a small keyword table, but every
//! spec it writes is one the generator accepts: params come from the
//! generator's own `param_schema()` defaults, jitter stays inside the
//! schema's ranges, and the same prompt always yields the same spec. That
//! lets the create → interpret → refine pipeline be tested end to end (and
//! fed to `flint gen`) with no network and no API key.

use flint_procgen::{
    register_built_in_generators, Generator, GeneratorRegistry, OutputKind, ProcGenSpec,
    SeedConfig, SeedMode, SeededRng, SpecMeta,
};
use serde_json::Value as Json;

use crate::agent::{ProcGenAgent, SpecFeedback};
use crate::error::{ProcGenAiError, Result};
use crate::schema::{check_params, default_of, prop_type, repair_params, round3, NumBounds};

/// A mock AI agent for testing without network calls.
///
/// Deterministic: outputs depend only on the inputs (prompt, spec, reference
/// bytes), never on a clock or a process-wide RNG.
#[derive(Debug, Default, Clone, Copy)]
pub struct MockAgent;

/// Specs for generators whose params have no usable defaults (a creature
/// needs a skeleton). The mock starts from these and applies the prompt.
const CREATURE_TEMPLATE: &str = include_str!("../templates/creature_v1.procgen.toml");

/// How a prompt word changes a param.
#[derive(Debug, Clone, Copy)]
enum Nudge {
    Scale(f64),
    SetF(f64),
    SetS(&'static str),
}

/// Prompt words and what they do. A row applies only when the generator has
/// that param, so "tall" shapes a tree and is ignored by a texture.
const KEYWORDS: &[(&[&str], &str, Nudge)] = &[
    (
        &["tall", "towering", "giant"],
        "trunk_height",
        Nudge::Scale(1.6),
    ),
    (
        &["tall", "towering", "giant"],
        "crown_height_ratio",
        Nudge::Scale(1.2),
    ),
    (
        &["short", "small", "stunted", "dwarf"],
        "trunk_height",
        Nudge::Scale(0.6),
    ),
    (
        &["short", "small", "stunted", "dwarf"],
        "crown_radius",
        Nudge::Scale(0.75),
    ),
    (
        &["wide", "broad", "spreading"],
        "crown_radius",
        Nudge::Scale(1.4),
    ),
    (
        &["oak", "old", "ancient"],
        "trunk_radius_base",
        Nudge::Scale(1.4),
    ),
    (&["oak"], "crown_shape", Nudge::SetS("ellipsoid")),
    (
        &["weeping", "willow"],
        "crown_shape",
        Nudge::SetS("hemisphere"),
    ),
    (&["weeping", "willow"], "lsystem_tropism", Nudge::SetF(-0.4)),
    (
        &["dense", "lush", "thick", "full"],
        "leaf_density",
        Nudge::SetF(0.95),
    ),
    (
        &["sparse", "thin", "bare"],
        "leaf_density",
        Nudge::Scale(0.4),
    ),
    (&["golden", "yellow"], "leaf_season", Nudge::SetF(0.4)),
    (
        &["autumn", "autumnal", "fall", "orange"],
        "leaf_season",
        Nudge::SetF(0.7),
    ),
    (
        &["red", "crimson", "scarlet"],
        "leaf_season",
        Nudge::SetF(1.0),
    ),
    (
        &["dead", "winter", "leafless"],
        "leaf_style",
        Nudge::SetS("none"),
    ),
    (
        &["brick", "bricks", "wall"],
        "pattern",
        Nudge::SetS("voronoi_brick"),
    ),
    (
        &[
            "stone", "rock", "moss", "mossy", "organic", "marble", "lava",
        ],
        "pattern",
        Nudge::SetS("perlin_organic"),
    ),
    (
        &["tile", "tiles", "tiled", "panel", "grid"],
        "pattern",
        Nudge::SetS("tiling_grid"),
    ),
    (
        &["smooth", "polished", "glossy"],
        "roughness_base",
        Nudge::Scale(0.5),
    ),
    (
        &["rough", "weathered", "worn"],
        "roughness_base",
        Nudge::Scale(1.2),
    ),
    (
        &["rough", "weathered", "worn"],
        "detail_strength",
        Nudge::Scale(2.0),
    ),
];

/// FNV-1a: a stable 64-bit hash, so seeds survive Rust and platform changes.
fn fnv1a(parts: &[&[u8]]) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for part in parts {
        for b in part.iter().chain(&[0xff]) {
            h ^= u64::from(*b);
            h = h.wrapping_mul(0x0100_0000_01b3);
        }
    }
    h
}

fn registry() -> GeneratorRegistry {
    let mut r = GeneratorRegistry::new();
    register_built_in_generators(&mut r);
    r.register(Box::new(flint_procgen::mock::MockGenerator));
    r
}

/// The registered name for `generator`: exact, or with `_v1` appended
/// (so `"tree"` finds `tree_v1`).
fn resolve_name(registry: &GeneratorRegistry, generator: &str) -> Option<String> {
    let g = generator.trim();
    if registry.get(g).is_some() {
        return Some(g.to_string());
    }
    let v1 = format!("{g}_v1");
    registry.get(&v1).is_some().then_some(v1)
}

fn unknown_generator(registry: &GeneratorRegistry, generator: &str) -> ProcGenAiError {
    let mut names = registry.generator_names();
    names.sort_unstable();
    ProcGenAiError::SpecError(format!(
        "unknown generator '{generator}'; known: {}",
        names.join(", ")
    ))
}

fn words(prompt: &str) -> Vec<String> {
    prompt
        .split(|c: char| !c.is_alphanumeric())
        .filter(|w| !w.is_empty())
        .map(str::to_lowercase)
        .collect()
}

fn props(schema: &Json) -> Option<&serde_json::Map<String, Json>> {
    schema.get("properties").and_then(Json::as_object)
}

/// Scale every float param that has a schema default by `1 ± amount`,
/// clamped into its range and rounded to three decimals. Params are visited
/// in sorted order so the RNG stream, and the result, is reproducible.
fn jitter_floats(schema: &Json, params: &mut toml::Value, rng: &mut SeededRng, amount: f32) {
    let (Some(props), Some(table)) = (props(schema), params.as_table_mut()) else {
        return;
    };
    let mut keys: Vec<String> = table.keys().cloned().collect();
    keys.sort();
    for key in keys {
        let Some(prop) = props.get(&key) else {
            continue;
        };
        if prop_type(prop) != "number" {
            continue;
        }
        let Some(toml::Value::Float(v)) = table.get(&key).cloned() else {
            continue;
        };
        let factor = 1.0 + f64::from(rng.next_range(-amount, amount));
        let bounds = NumBounds::of(prop);
        table.insert(key, toml::Value::Float(round3(bounds.clamp(v * factor))));
    }
}

/// Apply the keyword table. Returns the words that changed something.
fn apply_keywords(schema: &Json, params: &mut toml::Value, prompt: &str) -> Vec<String> {
    let (Some(props), Some(table)) = (props(schema), params.as_table_mut()) else {
        return Vec::new();
    };
    let said = words(prompt);
    let mut used = Vec::new();
    for (triggers, param, nudge) in KEYWORDS {
        let Some(word) = said.iter().find(|w| triggers.contains(&w.as_str())) else {
            continue;
        };
        let Some(prop) = props.get(*param) else {
            continue;
        };
        let bounds = NumBounds::of(prop);
        let current = table
            .get(*param)
            .and_then(|v| v.as_float().or(v.as_integer().map(|i| i as f64)));
        let value = match (nudge, current) {
            (Nudge::Scale(f), Some(v)) => toml::Value::Float(round3(bounds.clamp(v * f))),
            (Nudge::SetF(v), _) => toml::Value::Float(round3(bounds.clamp(*v))),
            (Nudge::SetS(s), _) => toml::Value::String((*s).to_string()),
            (Nudge::Scale(_), None) => continue,
        };
        table.insert((*param).to_string(), value);
        if !used.contains(word) {
            used.push(word.clone());
        }
    }
    used
}

fn slug(prompt: &str) -> String {
    let w = words(prompt);
    if w.is_empty() {
        "untitled".to_string()
    } else {
        w.into_iter().take(4).collect::<Vec<_>>().join("_")
    }
}

/// `1.2.3` → `1.2.4`; anything else is left alone.
fn bump_patch(version: &str) -> String {
    let mut parts: Vec<&str> = version.split('.').collect();
    if let Some(last) = parts.last_mut().and_then(|p| p.parse::<u64>().ok()) {
        let bumped = (last + 1).to_string();
        let n = parts.len();
        let mut out: Vec<String> = parts.drain(..n - 1).map(str::to_string).collect();
        out.push(bumped);
        return out.join(".");
    }
    version.to_string()
}

/// The spec with its generator name resolved and its params repaired
/// against the schema. Returns the repaired spec and what was fixed.
fn repaired(
    registry: &GeneratorRegistry,
    spec: &ProcGenSpec,
) -> Option<(ProcGenSpec, Vec<String>)> {
    let name = resolve_name(registry, &spec.generator)?;
    let generator = registry.get(&name)?;
    let schema = generator.param_schema();
    let mut fixed = spec.clone();
    let mut notes: Vec<String> = check_params(&schema, &spec.params)
        .iter()
        .map(ToString::to_string)
        .collect();
    if name != spec.generator {
        notes.insert(
            0,
            format!(
                "generator \"{}\" is registered as \"{name}\"; use that name",
                spec.generator
            ),
        );
        fixed.generator = name;
    }
    repair_params(&schema, &mut fixed.params);
    Some((fixed, notes))
}

impl MockAgent {
    /// The generator a spec names, if it is registered.
    fn with_generator<T>(
        &self,
        generator: &str,
        f: impl FnOnce(&str, &dyn Generator) -> Result<T>,
    ) -> Result<T> {
        let registry = registry();
        let name = resolve_name(&registry, generator)
            .ok_or_else(|| unknown_generator(&registry, generator))?;
        let g = registry.get(&name).expect("resolved names are registered");
        f(&name, g)
    }
}

impl ProcGenAgent for MockAgent {
    fn interpret_spec(&self, spec: &ProcGenSpec) -> Result<SpecFeedback> {
        let registry = registry();
        let Some((fixed, mut suggestions)) = repaired(&registry, spec) else {
            return Ok(SpecFeedback {
                suggestions: vec![unknown_generator(&registry, &spec.generator).to_string()],
                confidence: 0.0,
                revised_spec: None,
            });
        };
        let problems = suggestions.len();
        let mut revised = fixed;

        if spec.seed.mode == SeedMode::Random {
            suggestions.push(
                "the seed is random, so every run differs; fix it to make the asset reproducible"
                    .to_string(),
            );
            revised.seed = SeedConfig {
                mode: SeedMode::Fixed,
                value: Some(fnv1a(&[spec.meta.name.as_bytes()]) % 100_000),
                derive_from: None,
            };
        }
        let generator = registry
            .get(&revised.generator)
            .expect("repaired names resolve");
        if generator.output_kind() == OutputKind::Mesh && spec.lod.is_none() {
            suggestions.push(
                "no [[lod]] levels: add a few so the mesh stays cheap at a distance".to_string(),
            );
        }
        if suggestions.is_empty() {
            suggestions.push(format!(
                "params fit {}'s schema; try a few other seeds to see this spec's siblings",
                revised.generator
            ));
        }

        let confidence = if problems == 0 {
            0.9
        } else {
            (0.9 / (1.0 + problems as f32)).max(0.1)
        };
        Ok(SpecFeedback {
            suggestions,
            confidence,
            revised_spec: (revised != *spec).then_some(revised),
        })
    }

    fn create_spec_from_prompt(&self, prompt: &str, generator: &str) -> Result<ProcGenSpec> {
        self.with_generator(generator, |name, g| {
            let schema = g.param_schema();
            let seed = fnv1a(&[name.as_bytes(), prompt.as_bytes()]);
            let mut rng = SeededRng::new(seed);

            // Start from a template where defaults can't build anything.
            let mut params = match name {
                "creature_v1" => ProcGenSpec::parse(CREATURE_TEMPLATE)?.params,
                _ => toml::Value::Table(Default::default()),
            };
            // Write out every scalar default, so the spec says what it makes.
            if let (Some(props), Some(table)) = (props(&schema), params.as_table_mut()) {
                for (key, prop) in props {
                    let scalar =
                        matches!(prop_type(prop), "number" | "integer" | "boolean" | "string");
                    if scalar && !table.contains_key(key) {
                        if let Some(v) = default_of(prop) {
                            table.insert(key.clone(), v);
                        }
                    }
                }
            }
            jitter_floats(&schema, &mut params, &mut rng, 0.05);
            let used = apply_keywords(&schema, &mut params, prompt);
            repair_params(&schema, &mut params);

            let mut tags = vec!["ai-generated".to_string(), "mock".to_string()];
            tags.extend(used);
            Ok(ProcGenSpec {
                meta: SpecMeta {
                    name: slug(prompt),
                    description: Some(prompt.to_string()),
                    version: "1.0.0".to_string(),
                    tags,
                },
                generator: name.to_string(),
                seed: SeedConfig {
                    mode: SeedMode::Fixed,
                    value: Some(seed % 100_000),
                    derive_from: None,
                },
                params,
                lod: None,
            })
        })
    }

    fn refine_spec(&self, spec: &ProcGenSpec, reference: Option<&[u8]>) -> Result<ProcGenSpec> {
        let registry = registry();
        let (mut refined, _) = repaired(&registry, spec)
            .ok_or_else(|| unknown_generator(&registry, &spec.generator))?;
        let schema = registry
            .get(&refined.generator)
            .expect("repaired names resolve")
            .param_schema();

        // The mock can't see the reference; it only lets it pick the variation.
        let seed_bytes = spec.seed.value.unwrap_or(0).to_le_bytes();
        let mut rng = SeededRng::new(fnv1a(&[
            spec.meta.name.as_bytes(),
            &seed_bytes,
            reference.unwrap_or(&[]),
        ]));
        jitter_floats(&schema, &mut refined.params, &mut rng, 0.08);
        refined.meta.version = bump_patch(&refined.meta.version);
        Ok(refined)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn agent() -> Box<dyn ProcGenAgent> {
        Box::new(MockAgent) as Box<dyn ProcGenAgent>
    }

    fn schema_of(generator: &str) -> Json {
        registry().get(generator).unwrap().param_schema()
    }

    #[test]
    fn mock_agent_round_trip() {
        let agent = MockAgent;
        let spec = agent
            .create_spec_from_prompt("tall oak tree", "tree")
            .unwrap();
        let feedback = agent.interpret_spec(&spec).unwrap();
        let refined = agent.refine_spec(&spec, None).unwrap();
        assert!(!feedback.suggestions.is_empty());
        assert_eq!(refined.generator, "tree_v1");
    }

    #[test]
    fn tree_alias_resolves_to_the_registered_generator() {
        let spec = MockAgent
            .create_spec_from_prompt("tall oak tree", "tree")
            .unwrap();
        assert_eq!(spec.generator, "tree_v1");
        assert_eq!(spec.meta.name, "tall_oak_tree");
        assert_eq!(spec.meta.description.as_deref(), Some("tall oak tree"));
        assert!(spec.meta.tags.contains(&"tall".to_string()));
        assert!(spec.meta.tags.contains(&"oak".to_string()));
    }

    #[test]
    fn created_specs_fit_their_schemas_and_generate() {
        let registry = registry();
        for (prompt, generator) in [
            ("tall oak tree", "tree"),
            ("mossy stone", "texture"),
            ("a big beetle", "creature"),
            ("a box", "mock"),
        ] {
            let mut spec = MockAgent
                .create_spec_from_prompt(prompt, generator)
                .unwrap();
            let issues = check_params(&schema_of(&spec.generator), &spec.params);
            assert!(issues.is_empty(), "{prompt}: {issues:#?}");
            // Keep the texture small so the test stays quick in debug.
            if spec.generator == "texture_v1" {
                let t = spec.params.as_table_mut().unwrap();
                t.insert("width".into(), toml::Value::Integer(32));
                t.insert("height".into(), toml::Value::Integer(32));
            }
            registry
                .generate_from_spec(&spec)
                .unwrap_or_else(|e| panic!("{prompt}: {e}"));
        }
    }

    #[test]
    fn created_spec_survives_a_toml_round_trip() {
        let spec = MockAgent
            .create_spec_from_prompt("tall oak tree", "tree")
            .unwrap();
        let text = toml::to_string(&spec).unwrap();
        let back = ProcGenSpec::parse(&text).unwrap();
        assert_eq!(back, spec);
    }

    #[test]
    fn same_prompt_same_spec() {
        let a = MockAgent
            .create_spec_from_prompt("tall oak tree", "tree")
            .unwrap();
        let b = MockAgent
            .create_spec_from_prompt("tall oak tree", "tree")
            .unwrap();
        assert_eq!(a, b);
        let c = MockAgent
            .create_spec_from_prompt("tall oak trees", "tree")
            .unwrap();
        assert_ne!(a.seed.value, c.seed.value);
    }

    #[test]
    fn prompt_words_shape_the_params() {
        let height = |p: &str| {
            MockAgent.create_spec_from_prompt(p, "tree").unwrap().params["trunk_height"]
                .as_float()
                .unwrap()
        };
        assert!(height("tall tree") > height("tree") * 1.3);
        assert!(height("short tree") < height("tree") * 0.8);
        let autumn = MockAgent
            .create_spec_from_prompt("autumn maple", "tree")
            .unwrap();
        assert_eq!(autumn.params["leaf_season"].as_float(), Some(0.7));
        let brick = MockAgent
            .create_spec_from_prompt("old brick wall", "texture")
            .unwrap();
        assert_eq!(brick.params["pattern"].as_str(), Some("voronoi_brick"));
        // A tree word means nothing to a texture.
        let tex = MockAgent
            .create_spec_from_prompt("tall stone", "texture")
            .unwrap();
        assert!(tex.params.get("trunk_height").is_none());
    }

    #[test]
    fn unknown_generator_is_an_error_naming_the_known_ones() {
        let err = MockAgent
            .create_spec_from_prompt("x", "spaceship")
            .unwrap_err()
            .to_string();
        assert!(
            err.contains("spaceship") && err.contains("tree_v1"),
            "{err}"
        );
    }

    #[test]
    fn interpret_spec_returns_suggestions() {
        let spec = MockAgent.create_spec_from_prompt("test", "mock").unwrap();
        let feedback = MockAgent.interpret_spec(&spec).unwrap();
        assert!(!feedback.suggestions.is_empty());
        assert!((0.0..=1.0).contains(&feedback.confidence));
    }

    #[test]
    fn interpret_finds_and_fixes_real_problems() {
        let spec = ProcGenSpec::parse(
            r#"
generator = "tree"
[meta]
name = "broken"
version = "1.0.0"
[seed]
mode = "random"
[params]
trunk_height = -2.0
trunk_radius = 0.3
crown_shape = "pyramid"
"#,
        )
        .unwrap();
        let fb = MockAgent.interpret_spec(&spec).unwrap();
        let all = fb.suggestions.join("\n");
        assert!(all.contains("tree_v1"), "{all}");
        assert!(all.contains("`trunk_radius` is not a parameter"), "{all}");
        assert!(all.contains("`trunk_height` = -2"), "{all}");
        assert!(all.contains("pyramid"), "{all}");
        assert!(all.contains("seed is random"), "{all}");
        assert!(fb.confidence < 0.5);

        let revised = fb.revised_spec.expect("problems produce a revision");
        assert_eq!(revised.generator, "tree_v1");
        assert_eq!(revised.seed.mode, SeedMode::Fixed);
        assert!(check_params(&schema_of("tree_v1"), &revised.params).is_empty());
        // The revision is itself clean apart from advice.
        let again = MockAgent.interpret_spec(&revised).unwrap();
        assert!(again.confidence > 0.8, "{:?}", again.suggestions);
    }

    #[test]
    fn interpret_reports_an_unknown_generator_as_feedback() {
        let mut spec = MockAgent.create_spec_from_prompt("x", "mock").unwrap();
        spec.generator = "spaceship".into();
        let fb = MockAgent.interpret_spec(&spec).unwrap();
        assert_eq!(fb.confidence, 0.0);
        assert!(fb.suggestions[0].contains("spaceship"));
        assert!(MockAgent.refine_spec(&spec, None).is_err());
    }

    #[test]
    fn refine_stays_in_bounds_and_is_deterministic() {
        let spec = MockAgent
            .create_spec_from_prompt("wide dense oak", "tree")
            .unwrap();
        let a = MockAgent.refine_spec(&spec, None).unwrap();
        let b = MockAgent.refine_spec(&spec, None).unwrap();
        assert_eq!(a, b);
        assert_ne!(a.params, spec.params, "refining changes something");
        assert_eq!(a.meta.version, "1.0.1");
        assert!(check_params(&schema_of("tree_v1"), &a.params).is_empty());

        let with_ref = MockAgent
            .refine_spec(&spec, Some(b"reference png bytes"))
            .unwrap();
        assert_ne!(
            with_ref.params, a.params,
            "a reference picks another variation"
        );

        // Small steps: no float moves by more than 8% (plus rounding).
        for (k, v) in spec.params.as_table().unwrap() {
            if let (Some(old), Some(new)) = (v.as_float(), a.params[k].as_float()) {
                if old.abs() > 0.01 {
                    assert!((new / old - 1.0).abs() <= 0.081, "{k}: {old} -> {new}");
                }
            }
        }
    }

    #[test]
    fn full_pipeline_through_a_trait_object() {
        let agent = agent();
        let spec = agent
            .create_spec_from_prompt("tall oak tree", "tree")
            .unwrap();
        let fb = agent.interpret_spec(&spec).unwrap();
        let base = fb.revised_spec.unwrap_or(spec);
        let refined = agent.refine_spec(&base, None).unwrap();
        assert_eq!(refined.generator, "tree_v1");
        registry().generate_from_spec(&refined).unwrap();
    }

    #[test]
    fn version_bump() {
        assert_eq!(bump_patch("1.0.0"), "1.0.1");
        assert_eq!(bump_patch("2.9"), "2.10");
        assert_eq!(bump_patch("draft"), "draft");
    }
}
