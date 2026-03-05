use flint_procgen::{ProcGenSpec, SeedConfig, SeedMode, SpecMeta};
use rand::Rng;

use crate::agent::{ProcGenAgent, SpecFeedback};
use crate::error::Result;

/// A mock AI agent for testing without network calls.
///
/// Returns plausible but non-intelligent responses. All methods are
/// purely local — no API keys or network access required.
#[derive(Default)]
pub struct MockAgent;

impl ProcGenAgent for MockAgent {
    fn interpret_spec(&self, _spec: &ProcGenSpec) -> Result<SpecFeedback> {
        Ok(SpecFeedback {
            suggestions: vec![
                "Consider adjusting parameters for more variation".to_string(),
                "Try different seed values to explore output space".to_string(),
            ],
            confidence: 0.5,
            revised_spec: None,
        })
    }

    fn create_spec_from_prompt(&self, prompt: &str, generator: &str) -> Result<ProcGenSpec> {
        let mut rng = rand::rng();

        let params = match generator {
            "tree_v1" => toml::toml! {
                trunk_height = 4.0
                trunk_radius = 0.3
                crown_radius = 2.5
                branch_levels = 3
                leaf_density = 0.7
            },
            "texture_v1" => toml::toml! {
                width = 512
                height = 512
                octaves = 4
                persistence = 0.5
                scale = 50.0
            },
            "creature_v1" => toml::toml! {
                body_length = 1.5
                leg_count = 4
                head_scale = 0.4
            },
            _ => toml::toml! {
                scale = 1.0
            },
        };

        let name = prompt
            .split_whitespace()
            .take(3)
            .collect::<Vec<_>>()
            .join("_")
            .to_lowercase();

        Ok(ProcGenSpec {
            meta: SpecMeta {
                name,
                description: Some(prompt.to_string()),
                version: "1.0.0".to_string(),
                tags: vec!["ai-generated".to_string()],
            },
            generator: generator.to_string(),
            seed: SeedConfig {
                mode: SeedMode::Fixed,
                value: Some(rng.random_range(0..10000)),
                derive_from: None,
            },
            params: toml::Value::Table(params),
            lod: None,
        })
    }

    fn refine_spec(&self, spec: &ProcGenSpec, _reference: Option<&[u8]>) -> Result<ProcGenSpec> {
        let mut refined = spec.clone();
        let mut rng = rand::rng();

        // Apply small random perturbations to numeric params
        if let toml::Value::Table(ref mut table) = refined.params {
            for (_key, value) in table.iter_mut() {
                match value {
                    toml::Value::Float(f) => {
                        let jitter: f64 = rng.random_range(-0.1..0.1);
                        *f *= 1.0 + jitter;
                    }
                    toml::Value::Integer(i) => {
                        let delta: i64 = rng.random_range(-1..=1);
                        *i = (*i + delta).max(1);
                    }
                    _ => {}
                }
            }
        }

        Ok(refined)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mock_agent_round_trip() {
        let agent = MockAgent;

        let spec = agent
            .create_spec_from_prompt("tall oak tree", "tree_v1")
            .expect("create_spec_from_prompt should succeed");

        let feedback = agent
            .interpret_spec(&spec)
            .expect("interpret_spec should succeed");

        let refined = agent
            .refine_spec(&spec, None)
            .expect("refine_spec should succeed");

        assert!(!feedback.suggestions.is_empty());
        assert_eq!(refined.generator, "tree_v1");
    }

    #[test]
    fn create_spec_uses_generator_name() {
        let agent = MockAgent;
        let spec = agent
            .create_spec_from_prompt("tall oak tree", "tree_v1")
            .expect("create_spec_from_prompt should succeed");

        assert_eq!(spec.generator, "tree_v1");
        assert_eq!(spec.meta.description.as_deref(), Some("tall oak tree"));
    }

    #[test]
    fn interpret_spec_returns_suggestions() {
        let agent = MockAgent;
        let spec = agent
            .create_spec_from_prompt("test", "mock")
            .expect("create spec");

        let feedback = agent.interpret_spec(&spec).expect("interpret spec");
        assert!(!feedback.suggestions.is_empty());
        assert!((0.0..=1.0).contains(&feedback.confidence));
    }

    #[test]
    fn trait_object_compiles() {
        let agent: Box<dyn ProcGenAgent> = Box::new(MockAgent);
        let spec = agent
            .create_spec_from_prompt("test object", "mock")
            .expect("create spec via trait object");
        assert_eq!(spec.generator, "mock");
    }
}
