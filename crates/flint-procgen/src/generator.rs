//! The core `Generator` trait — the abstraction every procedural generator
//! implements.
//!
//! Generators are stateless: all randomness flows through [`SeededRng`], and
//! parameters come from [`ProcGenSpec`]. This keeps generators `Send + Sync`
//! and safe for future parallel generation.

use crate::output::{GeneratorOutput, OutputKind};
use crate::rng::SeededRng;
use crate::spec::ProcGenSpec;
use crate::Result;

/// Estimated resource cost of running a generator with a given spec.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct GenerationCost {
    /// Estimated vertex count in the output.
    pub estimated_vertices: u32,
    /// Estimated triangle count in the output.
    pub estimated_triangles: u32,
    /// Estimated total texture bytes.
    pub estimated_texture_bytes: u64,
    /// Estimated wall-clock generation time in milliseconds.
    pub estimated_generation_ms: f64,
}

impl Default for GenerationCost {
    fn default() -> Self {
        Self {
            estimated_vertices: 0,
            estimated_triangles: 0,
            estimated_texture_bytes: 0,
            estimated_generation_ms: 0.0,
        }
    }
}

/// A procedural content generator.
///
/// Implementations produce [`GeneratorOutput`] (mesh, image, or sound) from a
/// [`ProcGenSpec`] and a deterministic [`SeededRng`]. Generators are stateless
/// — randomness lives in the RNG, parameters in the spec.
///
/// Object-safe: `Box<dyn Generator>` works.
pub trait Generator: Send + Sync {
    /// The unique type name used to look up this generator in the registry.
    fn type_name(&self) -> &str;

    /// What kind of output this generator produces.
    fn output_kind(&self) -> OutputKind;

    /// Run the generator, producing output deterministically from the spec and RNG.
    fn generate(&self, spec: &ProcGenSpec, rng: &mut SeededRng) -> Result<GeneratorOutput>;

    /// JSON Schema describing the expected `params` table.
    fn param_schema(&self) -> serde_json::Value;

    /// Estimate the resource cost of generation without actually running it.
    fn estimate_cost(&self, _spec: &ProcGenSpec) -> GenerationCost {
        GenerationCost::default()
    }
}
