//! `flint render`'s registration point for the package that clocks a scene
//! (ADR 0137 "As built: the CLI launcher", #1558): the living package's
//! props and wooze field, which a render stage grows, sweeps and draws
//! over time. The kernel's render declares [`StagePart`] here, and the
//! package that fills it (living, from its CLI extension's `register`)
//! brings everything the render used to do behind its `living` feature:
//! `--sweep wooze.*|living.*`, the props at `--time`, the shading boil read
//! off the field, `--report json`'s props. With no part registered, the
//! render grows no props and runs no field, and a sweep of either names
//! the package.
//!
//! The report's shape ([`PropReport`] and the rest) is the kernel's, so
//! `--report json` is one document whoever fills it.

use crate::{CaptureGame, CaptureGameMut, Resources};
use flint_core::field_frame::BoilFrame;
use flint_core::EntityId;
use flint_ecs::FlintWorld;
use flint_render::{PostProcessConfig, SceneRenderer};
use flint_scene::SceneFile;
use serde::Serialize;
use std::collections::BTreeMap;
use std::path::Path;

/// Where `render`'s package flags go ([`RenderFlags`]).
pub struct RenderPlace;

impl super::flags::FlagPlace for RenderPlace {
    const PATH: &'static [&'static str] = &["render"];
}

/// `render`'s slot for the flags a package adds to it (#1572): the ai
/// package's `--ai-refine` group, at the place its help heading has always
/// had.
pub type RenderFlags = super::flags::PackageFlags<RenderPlace>;

/// The wooze values one sweep cell turns: each axis of it that names a
/// `wooze.<knob>`, as `(knob, value)` the line wrote them (`("glide",
/// "0.5")`, `("preset", "linklater")`).
pub type WoozeAxes<'a> = &'a [(&'a str, &'a str)];

/// The scene-table values one sweep cell turns: each axis of it whose key
/// is a table's field, as `(key, value)` the line wrote them
/// (`("living.echoes.spread", "0.9")`, #1595).
pub type TableAxes<'a> = &'a [(&'a str, &'a str)];

/// What a render stage asks of the package that clocks a scene.
pub trait StagePart: Send + Sync {
    /// Whether `world` grows props (an entity this package's props grow
    /// from).
    fn grows(&self, world: &FlintWorld) -> bool;

    /// A sweep cell's wooze values, held to the field before anything
    /// loads: a preset file must read, a knob take its value. `sweep` is
    /// the sweep's name, for the message.
    fn check_cell(&self, scene: &Path, sweep: &str, axes: WoozeAxes) -> anyhow::Result<()>;

    /// A `living.echoes.<knob>` value, held to what a spec's echoes take.
    fn check_echo(&self, knob: &str, value: &str) -> Result<(), String>;

    /// Write a sweep cell's table values into its own copy of the scene's
    /// top-level tables (#531, #1595), right after the cell's scene
    /// settles and before anything reads them: each key's field, its
    /// tables made when the scene has none, every other field left as the
    /// scene wrote it. The cell then draws what a scene with those tables
    /// draws.
    fn lay_tables(&self, tables: &mut toml::Table, axes: TableAxes);

    /// The stage's cell, after its assets are made and before its
    /// subsystems are built: lay a preset column over the props `grows`,
    /// and leave the stage's field (the scene's with the cell's values
    /// over it) in `resources` for the capture's subsystem to take.
    fn stage(
        &self,
        scene: &SceneFile,
        path: &Path,
        axes: WoozeAxes,
        grows: bool,
        resources: &mut Resources,
    ) -> anyhow::Result<Box<dyn StageClock>>;

    /// Blend every prop where its clock stands and write what moved, as a
    /// frame of the player does, before a capture.
    fn upload(
        &self,
        game: CaptureGameMut,
        renderer: &mut SceneRenderer,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
    );

    /// Every prop as it stands, by entity name (`--report json`'s `props`).
    fn props(&self, game: CaptureGame) -> Vec<PropReport>;

    /// How many props the stage grew.
    fn count(&self, game: CaptureGame) -> usize;

    /// The entities that grew into props.
    fn entities(&self, game: CaptureGame) -> Vec<EntityId>;

    /// What a wooze column's props read, against the value it `set`: empty
    /// when they agree.
    fn read_note(&self, game: CaptureGame, knob: &str, set: Option<f32>) -> String;

    /// The zones of the stage's field, with one.
    fn zones(&self, game: CaptureGame) -> Option<usize>;
}

/// One stage's field, as the render's post reads it.
pub trait StageClock {
    /// A preset column's shading boil laid over `config`; whether it laid
    /// one (the render then lays its own flags over it again).
    fn lay_boil(&self, config: &mut PostProcessConfig) -> bool;

    /// Whether the stage has a field at all.
    fn has_field(&self) -> bool;

    /// The boil's drawing at `time` of the field, seen from `eye`, held
    /// there (`--boil-time`); `None` with no field.
    fn boil_at(&self, eye: [f32; 3], time: f64, boil_on: u32) -> Option<BoilFrame>;
}

/// One living prop at one capture.
#[derive(Debug, Serialize)]
pub struct PropReport {
    pub entity: String,
    pub asset: String,
    pub lineage_seed: u64,
    pub position: [f32; 3],
    pub phase_offset: f32,
    /// Its clock, in steps it has run through.
    pub beats: f64,
    /// The seconds its words have drifted along their lines through (#576): 0
    /// until a form with `row_pace` runs, moving at `row_rate`.
    pub row_time: f64,
    /// `None` only if it hasn't posed, which `flint render` doesn't let happen.
    pub poses: Option<PoseReport>,
    pub wooze: WoozeReport,
    pub echoes: usize,
    /// Its spec's name, which is what `[living.species.<spec>]` is keyed by.
    pub species: String,
    /// Every value a species can set (#246), as the prop runs on it, and the layer
    /// that set it: `spec`, `entity`, `global`, `zone:<name>` or `species`.
    pub values: BTreeMap<&'static str, ValueReport>,
}

/// One resolved value and who said so.
#[derive(Debug, PartialEq, Serialize)]
pub struct ValueReport {
    pub value: ReportValue,
    pub layer: String,
}

/// A value as JSON writes it: an `f32` as briefly as it reads back, as the rest
/// of the report does.
#[derive(Debug, PartialEq, Serialize)]
#[serde(untagged)]
pub enum ReportValue {
    Number(f32),
    Count(u32),
    Flag(bool),
    Name(String),
}

/// Which poses a prop was drawn between, from the blend that drew it. A staggered
/// tree's parts aren't all on one: `lowest` and `highest` are how far they spread.
#[derive(Debug, PartialEq, Serialize)]
pub struct PoseReport {
    /// The trunk's pose: the one it's leaving.
    pub index: u64,
    /// How far the trunk has gone toward the next, 0 to 1.
    pub weight: f32,
    /// That pose's shape seed, which `pose()` would regrow it from.
    pub shape_seed: u64,
    pub lowest: u64,
    pub highest: u64,
    /// How many parts were between poses, not holding on one.
    pub gliding_parts: usize,
    pub parts: usize,
}

/// What the wooze field said about where a prop stands.
#[derive(Debug, PartialEq, Serialize)]
pub struct WoozeReport {
    pub amplitude: f32,
    pub step: f32,
    pub glide: f32,
    pub jitter: f32,
}
