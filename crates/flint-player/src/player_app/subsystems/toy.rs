#![cfg(test)]
//! A toy subsystem, to hold #724's third box: adding a subsystem is one new
//! file here and touches no other file in `player_app/`. `build.rs` lists this
//! file's `for_scene`; a scene with a `[toy]` table gets one, which counts the
//! frames it hears.

use super::{Lasts, SceneCtx, Stage, StageCtx, Subsystem};
use crate::player_app::PlayerApp;
use flint_core::project::{SchemaDirs, SchemaSource};
use flint_core::vfs::{self, MemorySource};
use std::sync::Arc;

/// It reads no game-state policy.
pub(crate) const POLICY: Option<&str> = None;

/// Its script module, `toy` (#698): `toy_frames()`, enabled in a scene
/// with a toy.
pub(crate) const SCRIPT_MODULES: &[&dyn flint_script::ScriptModule] = &[&ToyModule];

/// What the toy hands its module's functions: the frames it has heard.
#[derive(Debug, Default)]
struct ToyFrames(u32);

struct ToyModule;

impl flint_script::ScriptModule for ToyModule {
    fn name(&self) -> &'static str {
        "toy"
    }

    fn enabled_when(&self) -> &'static str {
        "the scene has a [toy] table"
    }

    fn declare(&self, m: &mut flint_script::api::Module<'_>) {
        m.func("toy_frames", "The frames the toy has heard.")
            .returns(flint_script::api::builder::kind::Int, "frames")
            .run(|call, ()| call.ctx().module_state.get_mut::<ToyFrames>().0 as i64);
    }
}

#[derive(Default)]
pub(crate) struct Toy {
    pub(crate) frames: u32,
}

/// A toy for a scene with a `[toy]` table.
pub(crate) fn for_scene(ctx: &mut SceneCtx) -> Option<Box<dyn Subsystem>> {
    ctx.tables
        .contains_key("toy")
        .then(|| Box::new(Toy::default()) as Box<dyn Subsystem>)
}

impl Subsystem for Toy {
    fn name(&self) -> &'static str {
        "toy"
    }

    fn lasts(&self) -> Lasts {
        Lasts::Scene
    }

    fn script_modules(&self) -> &[&'static str] {
        &["toy"]
    }

    fn update(&mut self, stage: Stage, ctx: &mut StageCtx) {
        if stage == Stage::Sync {
            self.frames += 1;
            ctx.script.set_module_state(ToyFrames(self.frames));
        }
    }
}

fn app_in(scene: &str) -> PlayerApp {
    let registry = flint_schema::SchemaRegistry::new();
    let (world, file) = flint_scene::load_scene(scene, &registry).expect("the scene loads");
    let mut app = PlayerApp::new(world, scene.into(), false, None, None);
    app.apply_scene_settings(&file);
    app.set_schemas(
        registry,
        SchemaDirs {
            dirs: Vec::new(),
            source: SchemaSource::Flags,
            dropped: Vec::new(),
        },
    );
    app.core.clock.frame_step = Some(0.25);
    app.enter_scene(crate::player_app::scene_entry::SceneEntry::Start)
        .expect("the scene starts");
    app
}

#[test]
fn a_toy_in_its_own_file_comes_with_a_scene_that_asks_and_hears_its_frames() {
    let mut files = MemorySource::new();
    files.insert("toys/flint.toml", "");
    files.insert(
        "toys/scenes/with.scene.toml",
        "[scene]\nname = \"With\"\n\n[toy]\nsize = 1\n",
    );
    files.insert(
        "toys/scenes/without.scene.toml",
        "[scene]\nname = \"Without\"\n",
    );
    vfs::scoped(Arc::new(files), || {
        let mut app = app_in("toys/scenes/with.scene.toml");
        app.tick();
        app.tick();
        let toy = app
            .core
            .subsystems
            .find::<Toy>()
            .expect("a [toy] scene has one");
        assert_eq!(toy.frames, 2);

        assert!(app.execute_scene_transition("without.scene.toml"));
        assert!(
            app.core.subsystems.find::<Toy>().is_none(),
            "it went with its scene"
        );

        let app = app_in("toys/scenes/without.scene.toml");
        assert!(app.core.subsystems.find::<Toy>().is_none());
    });
}

/// A script asks the toy's module from its `on_init` and every frame: the
/// subsystem is built before the scripts load, so the module is enabled
/// for their first call (#698). In a scene with no toy the same call is
/// the "not enabled" error, and nothing it would have written is.
#[test]
fn a_scene_with_a_toy_lets_its_scripts_call_the_toy_module_from_on_init() {
    const COUNTER: &str = r#"
fn on_init() { set_field(self_entity(), "probe", "init", toy_frames()); }
fn on_update() { set_field(self_entity(), "probe", "seen", toy_frames()); }
"#;
    let scripted = |name: &str, toy: bool| {
        format!(
            "[scene]\nname = \"{name}\"\n{}\n[entities.counter]\n[entities.counter.probe]\n\
             init = -1\nseen = -1\n[entities.counter.script]\nsource = \"counter.rhai\"\n",
            if toy { "\n[toy]\nsize = 1\n" } else { "" }
        )
    };
    let mut files = MemorySource::new();
    files.insert("toys/flint.toml", "");
    files.insert("toys/scenes/with.scene.toml", scripted("With", true));
    files.insert("toys/scenes/without.scene.toml", scripted("Without", false));
    files.insert("toys/scripts/counter.rhai", COUNTER);
    vfs::scoped(Arc::new(files), || {
        let probe = |app: &PlayerApp, field: &str| {
            let id = app.core.world.get_id("counter").expect("the counter");
            app.core
                .world
                .get_components(id)
                .and_then(|c| c.get_field("probe", field))
                .and_then(|v| v.as_integer())
        };
        let mut app = app_in("toys/scenes/with.scene.toml");
        assert_eq!(app.core.script.enabled_modules(), ["toy"]);
        assert_eq!(probe(&app, "init"), Some(0), "on_init reached the module");
        app.tick();
        app.tick();
        assert_eq!(probe(&app, "seen"), Some(1), "last frame's count");

        let mut app = app_in("toys/scenes/without.scene.toml");
        assert!(app.core.script.enabled_modules().is_empty());
        app.tick();
        assert_eq!(probe(&app, "init"), Some(-1), "the call was an error");
        assert_eq!(probe(&app, "seen"), Some(-1));
    });
}
