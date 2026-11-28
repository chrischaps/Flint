//! A flipbook on animation's clock, through the player (#1620, ADR 0139
//! section 4): a sequence's crossfade cuts a sprite to another flipbook, a
//! cue the sequence passes reaches the entity's script while the sprite
//! shows the new clip's frame, and the one-shot's end reaches it as
//! `on_animation_end`. Before #1620 a sprite had no sequences and no cues.

use crate::player_app::scene_entry::SceneEntry;
use crate::player_app::PlayerApp;
use flint_core::project::{SchemaDirs, SchemaSource};
use flint_core::vfs::{self, MemorySource};
use std::sync::Arc;

/// Two clips of one sheet: a walk on row 0, a wave on row 1 played once.
const SHEET: &str = r#"
[animation.walk]
texture = "hero.png"
frame_size = [32, 32]
frames = [
    { col = 0, row = 0, duration_ms = 100 },
    { col = 1, row = 0, duration_ms = 100 },
    { col = 2, row = 0, duration_ms = 100 },
    { col = 3, row = 0, duration_ms = 100 },
]

[animation.wave]
texture = "hero.png"
frame_size = [32, 32]
frames = [
    { col = 0, row = 1, duration_ms = 100 },
    { col = 1, row = 1, duration_ms = 100 },
]
loop_mode = "once"
"#;

/// At 0.2 s the walk crossfades to the wave (a cut, for a flipbook), and a
/// moment later a cue says so.
const STROLL: &str = r#"
name = "stroll"

[[events]]
time = 0.2
kind = "blend"
clip = "wave"
duration = 0.1

[[events]]
time = 0.25
kind = "cue"
name = "waved"
"#;

const SCENE: &str = r#"
[scene]
name = "Stroll"

[entities.hero.transform]
position = [0.0, 0.0, 0.0]

[entities.hero.sprite]
mode = "sprite2d"
texture = "hero.png"

[entities.hero.animator]
clip = "walk"
playing = true
sequence = "stroll"

[entities.hero.probe]
cue = ""
row = -1.0
ended = ""

[entities.hero.script]
source = "hero.rhai"
"#;

/// What the script heard, and which row of the sheet the sprite showed then.
const HERO: &str = r#"
fn on_sequence_cue(sequence, cue) {
    let me = self_entity();
    set_field(me, "probe", "cue", sequence + ":" + cue);
    let rect = get_field(me, "sprite", "source_rect");
    set_field(me, "probe", "row", rect[1] / 32.0);
}

fn on_animation_end(clip) {
    set_field(self_entity(), "probe", "ended", clip);
}
"#;

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
            packages: None,
        },
    );
    app.core.clock.frame_step = Some(0.05);
    app.enter_scene(SceneEntry::Start)
        .expect("the scene starts");
    app
}

fn probe(app: &PlayerApp, field: &str) -> toml::Value {
    let id = app.core.world.get_id("hero").expect("the hero");
    app.core
        .world
        .get_components(id)
        .and_then(|c| c.get_field("probe", field))
        .cloned()
        .expect("the probe")
}

/// The issue's new test: a sequence cue on a flipbook's frame reaches the
/// script, the frame it fires on is the clip the sequence cut to, and the
/// one-shot it cut to says when it ends.
#[test]
fn a_sequence_cue_on_a_flipbooks_frame_reaches_its_script() {
    if !crate::packages::registry().has("flipbook") {
        return;
    }
    let mut files = MemorySource::new();
    files.insert("game/flint.toml", "");
    files.insert("game/scenes/stroll.scene.toml", SCENE);
    files.insert("game/scenes/animations/hero.sprite.toml", SHEET);
    files.insert("game/scenes/animations/stroll.sequence.toml", STROLL);
    files.insert("game/scripts/hero.rhai", HERO);
    vfs::scoped(Arc::new(files), || {
        let mut app = app_in("game/scenes/stroll.scene.toml");
        for _ in 0..4 {
            app.tick();
        }
        assert_eq!(probe(&app, "cue").as_str(), Some(""), "not yet at 0.2 s");
        for _ in 0..4 {
            app.tick();
        }
        assert_eq!(probe(&app, "cue").as_str(), Some("stroll:waved"));
        assert_eq!(
            flint_core::toml_util::toml_f64(&probe(&app, "row")),
            Some(1.0),
            "the cue fired on the wave's frame"
        );
        for _ in 0..6 {
            app.tick();
        }
        assert_eq!(probe(&app, "ended").as_str(), Some("wave"));
        let errors = app.core.script.errors().all();
        assert!(errors.is_empty(), "{errors:?}");
    });
}
