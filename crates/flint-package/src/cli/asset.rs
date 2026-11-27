//! `flint asset resolve`'s point for the package that hands a scene's
//! missing assets to a provider or a person (`--strategy ai_generate |
//! human_task | ai_then_human`, the ai package's batch; #1572).
//!
//! The kernel's command reads the scene, finds every asset reference and
//! says what it found; a batch strategy then hands the missing ones to the
//! [`BatchResolve`] the package registered. Without it the strategy is
//! refused, naming the package from its manifest.

use anyhow::Result;
use flint_core::project::AssetKind;
use serde_json::Value;

/// The package's batch, as the run's points hold it.
pub struct BatchResolve(pub fn(Batch) -> Result<()>);

/// One reference nothing holds.
pub struct MissingAsset {
    /// The entity, or `[environment]` for a scene table.
    pub place: String,
    /// `model.asset`, or `environment.skybox`.
    pub field: String,
    /// `crate.model.asset`, or `[environment].skybox`.
    pub field_of: String,
    pub name: String,
    /// What the field takes, in the engine's search order.
    pub kinds: Vec<AssetKind>,
    /// The reference as `--json` lists it.
    pub json: Value,
}

/// A scene's references, as the command found them, and what the line
/// asked.
pub struct Batch<'a> {
    pub scene: &'a str,
    /// The strategy as written (`ai_generate`).
    pub strategy: &'a str,
    /// `--style`: the style guide's name.
    pub style: Option<&'a str>,
    /// `--output-dir`.
    pub output_dir: Option<&'a str>,
    /// `--max-cost`, in USD.
    pub max_cost: Option<f64>,
    /// Every reference the scene makes.
    pub references: usize,
    /// How many of them something holds.
    pub found: usize,
    pub missing: Vec<MissingAsset>,
}
