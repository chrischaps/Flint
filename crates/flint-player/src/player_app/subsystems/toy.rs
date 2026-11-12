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

#[derive(Default)]
pub(crate) struct Toy {
    pub(crate) frames: u32,
}

/// A toy for a scene with a `[toy]` table.
pub(crate) fn for_scene(ctx: &SceneCtx) -> Option<Box<dyn Subsystem>> {
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

    fn update(&mut self, stage: Stage, _ctx: &mut StageCtx) {
        if stage == Stage::Sync {
            self.frames += 1;
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
    app.clock.frame_step = Some(0.25);
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
        let toy = app.subsystems.find::<Toy>().expect("a [toy] scene has one");
        assert_eq!(toy.frames, 2);

        assert!(app.execute_scene_transition("without.scene.toml"));
        assert!(
            app.subsystems.find::<Toy>().is_none(),
            "it went with its scene"
        );

        let app = app_in("toys/scenes/without.scene.toml");
        assert!(app.subsystems.find::<Toy>().is_none());
    });
}
