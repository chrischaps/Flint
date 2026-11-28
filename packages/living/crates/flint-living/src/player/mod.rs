//! The living package's part of the player (#1558): what flint-player did
//! behind its `living` feature, built against the Package API
//! (`flint-package`), so the player names none of it.
//!
//! - [`PACKAGE`]: what a launcher lists. It registers the living subsystem's
//!   builder ([`subsystem::LivingSubsystem`]: the wooze field and its clock,
//!   the shading boil, shallow wooze, woozy grass, the hand tuning and the
//!   A/B), its policy name, script modules and debug keys, the props as a
//!   run resource, the scene settle of `[living]` / `[wooze]`, and its step
//!   of procgen's load (the props, [`props::resolve`]). With the debug HUD
//!   (this crate's `debug-hud` feature, the manifest's `[with.debug-hud]`)
//!   the Living Props panel joins the scene's F3 panels.
//! - `--compare A,B` and `--split` are its own flags ([`CompareArgs`]),
//!   which the launchers add to `flint-player` and `flint play` from the
//!   registry (`Registry::player_args`, #1585); their value reaches the
//!   builder in the run's resources.

pub mod ab;
pub mod api;
pub mod props;
pub mod ramps;
pub mod subsystem;
pub mod tuning;

use flint_core::package::InUse;
use flint_ecs::FlintWorld;
use flint_package::{Package, Registry, SceneCtx, Subsystem};
use flint_render::GrassConfig;

/// A game state pauses the grove's clock by `living: "pause"` (#727). The
/// kernel keeps the name in a build without the package, so a state that
/// names it still registers.
pub const POLICY: &str = "living";

/// Its script modules (#698): the wooze field, the knobs and the A/B,
/// enabled in a scene the subsystem runs in.
pub const SCRIPT_MODULES: &[&dyn flint_script::ScriptModule] =
    &[&api::WOOZE, &api::LIVING, &api::DEBUG];

/// Its debug keys (#717): the A/B of #238 and the pick of #248. Live only
/// in a scene the subsystem runs in; elsewhere F5-F8 are the game's.
pub const DEBUG_KEYS: flint_runtime::DebugKeyGroup = flint_runtime::DebugKeyGroup {
    owner: "living",
    when: "a scene with living props or wooze",
    keys: KEYS,
};

const A_B: [flint_runtime::DebugKey; 3] = [
    flint_runtime::DebugKey {
        action: "living_freeze",
        key: "F5",
        does: "Freeze or thaw the living clock",
    },
    flint_runtime::DebugKey {
        action: "living_flip",
        key: "F6",
        does: "A/B: flip between the two presets",
    },
    flint_runtime::DebugKey {
        action: "living_split",
        key: "F7",
        does: "A/B: split the view between the two presets",
    },
];
#[cfg(feature = "debug-hud")]
const KEYS: &[flint_runtime::DebugKey] = &[
    A_B[0],
    A_B[1],
    A_B[2],
    flint_runtime::DebugKey {
        action: "living_pick",
        key: "F8",
        does: "Pick the living prop at the view centre",
    },
    flint_runtime::DebugKey {
        action: "living_pick_next",
        key: "Shift+F8",
        does: "Pick the next living prop by name",
    },
];
#[cfg(not(feature = "debug-hud"))]
const KEYS: &[flint_runtime::DebugKey] = &A_B;

/// Living's flags on the player's command line (#238): what `--compare`
/// and `--split` asked, in the run's resources (`Registry::player_args`,
/// #1585). A build without the package has neither flag. A game binary
/// that compares from the start inserts one into `app.core.resources`
/// before the first scene.
#[derive(clap::Args, Clone, Debug, Default, PartialEq, Eq)]
#[command(next_help_heading = "Living props")]
pub struct CompareArgs {
    /// Compare two Living Props presets on the same grove from the start
    /// (#238): A,B, each a preset's name, or nothing for the knobs as the scene
    /// loaded them (",linklater"). F6 flips between them, F7 splits the view
    #[arg(long, value_name = "A,B", value_parser = parse_compare)]
    pub compare: Option<Presets>,

    /// With --compare: start split, A on the left and B on the right
    #[arg(long, requires = "compare")]
    pub split: bool,
}

/// The two presets `--compare A,B` names: A may be empty (the scene's own
/// knobs), B is a preset's name.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Presets {
    pub a: String,
    pub b: String,
}

/// `--compare A,B`, checked: two names, B not empty.
pub fn parse_compare(spec: &str) -> Result<Presets, String> {
    let (a, b) = spec
        .split_once(',')
        .ok_or_else(|| format!("{spec}: two presets, as A,B"))?;
    if b.trim().is_empty() {
        return Err(format!("{spec}: B needs a preset's name"));
    }
    Ok(Presets {
        a: a.trim().to_owned(),
        b: b.trim().to_owned(),
    })
}

/// The living package, as a launcher lists it.
pub static PACKAGE: Living = Living;

/// The living package's player part.
pub struct Living;

impl Package for Living {
    fn name(&self) -> &'static str {
        "living"
    }

    fn register(&self, registry: &mut Registry) {
        registry
            .policy(POLICY)
            .scene_subsystem(for_scene)
            .when_left_out(left_out)
            .script_modules(SCRIPT_MODULES)
            .debug_keys(DEBUG_KEYS)
            .player_args::<CompareArgs>()
            .run_resources(|resources| {
                resources.insert(crate::LivingProps::default());
            })
            .settle_scene(flint_procgen::settle_scene);
        // The props take their `living_prop` entities' names off procgen's
        // plain path at a scene's load (procgen's load steps, #1557): the
        // procgen package registers first, as living `[requires]` it
        if let Some(steps) = registry.point_mut::<flint_procgen::player::LoadSteps>() {
            steps.0.push(resolve_props);
        }
        // The Living Props panel (#218), in a scene with a living prop,
        // where the player has the debug HUD's panels
        #[cfg(feature = "debug-hud")]
        if let Some(panels) = registry.point_mut::<flint_debug_ui::ScenePanels>() {
            panels.0.push(("living", living_panel));
        }
    }
}

/// Living's step of a scene's procgen load: the scene's `living_prop`
/// entities become props, each pointed at its lineage's mesh, before
/// procgen makes the other names. A project whose `[packages]` leaves
/// `living` out grows none: each takes the plain path, a still model, as
/// in a build without it (#1528).
fn resolve_props(
    ctx: &mut flint_procgen::player::LoadCtx,
    sink: &mut dyn flint_procgen::player::AssetSink,
    mode: flint_package::assets::MakeMode,
) {
    if ctx.leaves_out("living") {
        return;
    }
    let (props, resolver) = subsystem::props_and_resolver(ctx.resources);
    // What the scene says about living props, for the props this load adds
    // (their clocks and field are the living subsystem's)
    let living = subsystem::tables_of(ctx.tables).living;
    props.set_defaults(props::defaults_of(living.as_ref()));
    props::resolve(ctx.world, ctx.config, resolver, props, sink, mode);
}

/// The subsystem for the scene being entered, if it has living props or
/// wooze; in a capture, the grove posed at once (`ctx.capture`, #1252).
fn for_scene(ctx: &mut SceneCtx) -> Option<Box<dyn Subsystem>> {
    debug_assert_eq!(POLICY, subsystem::LivingSubsystem::NAME);
    subsystem::LivingSubsystem::for_scene(ctx)
}

/// The Living Props panel, for a scene with any `living_prop` entity
/// (#218). The subsystem's drain mirrors the grove into it.
#[cfg(feature = "debug-hud")]
fn living_panel(world: &FlintWorld) -> Option<Box<dyn flint_debug_ui::DebugPanel>> {
    (!world
        .entities_with_component(crate::components::LIVING_PROP)
        .is_empty())
    .then(|| Box::new(crate::panel::LivingPropsDebugPanel::new()) as Box<_>)
}

/// What a scene that uses living props hears in a project that leaves the
/// package out: the line to add to `[packages]`.
fn left_out(ctx: &SceneCtx) -> Option<String> {
    let grass = ctx.renderer.as_deref().and_then(|r| r.grass_config());
    left_out_line(ctx.packages?, ctx.world, ctx.tables, grass)
}

/// What of living props a scene uses, in the order the line names them.
pub fn uses(
    world: &FlintWorld,
    tables: &toml::Table,
    grass: Option<&GrassConfig>,
) -> Vec<&'static str> {
    let has = |name: &str| !world.entities_with_component(name).is_empty();
    let mut uses = Vec::new();
    if has(crate::components::LIVING_PROP) {
        uses.push("living props");
    }
    if tables.contains_key("living") {
        uses.push("[living]");
    }
    if tables.contains_key("wooze") {
        uses.push("[wooze]");
    }
    if has(flint_wooze::components::SHALLOW_WOOZE) {
        uses.push("shallow_wooze meshes");
    }
    if grass.and_then(GrassConfig::wooze_comb).is_some() {
        uses.push("woozy grass");
    }
    uses
}

/// `flint_package::left_out_line` of living props: what [`uses`] finds
/// stands still.
pub fn left_out_line(
    in_use: &InUse,
    world: &FlintWorld,
    tables: &toml::Table,
    grass: Option<&GrassConfig>,
) -> Option<String> {
    flint_package::left_out_line(in_use, "living", &uses(world, tables, grass), "stand still")
}
