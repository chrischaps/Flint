//! The core `Generator` trait — the abstraction every procedural generator
//! implements.
//!
//! Generators are stateless: all randomness flows through [`SeededRng`], and
//! parameters come from [`ProcGenSpec`]. This keeps generators `Send + Sync`
//! and safe for future parallel generation.

use crate::living::{LivingPose, LivingSkeleton};
use crate::output::{GeneratorOutput, OutputKind};
use crate::rng::SeededRng;
use crate::spec::ProcGenSpec;
use crate::types::MeshData;
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

    /// The revision of what this generator makes. Bump it in any change after
    /// which the same spec and seed produce different output: `flint gen
    /// --register` keys its cache on the spec's bytes and the seed, and
    /// treats an entry recorded under another revision as stale. Changes
    /// that leave every output byte-identical (a refactor, a faster path)
    /// keep the number.
    ///
    /// There's no default: a generator says which revision it is.
    fn revision(&self) -> u32;

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

    /// This generator's living side, for callers holding a `dyn Generator`
    /// from the registry. `None` for one that can't separate a structure
    /// from its poses.
    fn as_living(&self) -> Option<&dyn LivingGenerator> {
        None
    }
}

/// A generator whose output can keep drifting: one fixed structure (its
/// identity, from a lineage seed) and a stream of poses that vary only
/// continuous values (from shape seeds). Topology can't be interpolated, so
/// this split is what gives procgen a time axis.
///
/// Optional: a generator that can't make the split doesn't implement it.
/// Like [`Generator`], it's stateless and deterministic.
pub trait LivingGenerator: Send + Sync {
    /// Whether this spec can be split, and if not, why:
    /// [`ProcGenError::LivingUnsupported`](crate::ProcGenError) names the
    /// reason. [`structure`](Self::structure) and [`pose`](Self::pose) refuse
    /// the same specs.
    fn check_living(&self, spec: &ProcGenSpec) -> Result<()>;

    /// The skeleton every pose of this lineage shares: segments with stable
    /// ids and parents, and the leaf cards bound to them.
    fn structure(&self, spec: &ProcGenSpec, lineage_seed: u64) -> Result<LivingSkeleton>;

    /// One variant of `skeleton` as this shape seed grows it, undamped: every
    /// segment as far from rest as the seed took it. A ring kept in this form
    /// can be re-held at a new variance with
    /// [`LivingSkeleton::with_variance`] and no regrowing, which is what makes
    /// the variance knobs live. The same arguments give the same bits.
    fn full_pose(
        &self,
        spec: &ProcGenSpec,
        skeleton: &LivingSkeleton,
        shape_seed: u64,
    ) -> Result<LivingPose>;

    /// One variant of `skeleton`, as per-segment transforms relative to it,
    /// held to the spec's `[living] variance` (its defaults if the spec has
    /// no `[living]` table). The same arguments give the same bits.
    fn pose(
        &self,
        spec: &ProcGenSpec,
        skeleton: &LivingSkeleton,
        shape_seed: u64,
    ) -> Result<LivingPose> {
        let full = self.full_pose(spec, skeleton, shape_seed)?;
        let variance = spec
            .living
            .as_ref()
            .map_or_else(Default::default, |living| living.variance);
        skeleton.with_variance(&full, &variance)
    }

    /// The whole prop as `pose` holds it, rebuilt from scratch: for offline
    /// tools, since a mesh per frame is what the skinned rest mesh avoids.
    /// `pose` may be a blend of two poses.
    fn posed_mesh(
        &self,
        spec: &ProcGenSpec,
        skeleton: &LivingSkeleton,
        pose: &LivingPose,
    ) -> Result<MeshData>;
}
