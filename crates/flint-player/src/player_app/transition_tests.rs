//! #362: a scene transition reaches its scene whether or not a script draws
//! it. `load_scene` with no script that can call `complete_transition()`
//! loads on the next frame and goes straight back to idle; a scene whose HUD
//! draws a fade waits for it, as before, and a fade that never says it's
//! done is ended after `TRANSITION_PHASE_TIMEOUT`.
//!
//! The project is a `MemorySource`; no window, no GPU.

use super::scene_entry::SceneEntry;
use super::transition::TRANSITION_PHASE_TIMEOUT;
use super::{PlayerApp, TransitionPhase};
use flint_core::events::TRANSITION_COMPLETE;
use flint_core::project::{SchemaDirs, SchemaSource};
use flint_core::vfs::{self, MemorySource};
use flint_script::ScriptHost;
use std::sync::Arc;

/// The issue's repro: one script, which leaves, and nothing drawing a fade.
const LEAVE: &str = r#"
let gone = false;
fn on_update() {
    if gone { return; }
    if total_time() > 1.0 { gone = true; load_scene("b.scene.toml"); }
}
"#;

/// A HUD that draws its fades and says when each is done.
const HUD: &str = r#"
let t = 0.0;
fn on_update() {
    let phase = transition_phase();
    if phase == "exiting" || phase == "entering" {
        t += delta_time();
        if t >= 1.5 { t = 0.0; complete_transition(); }
    }
}
"#;

fn scene(name: &str, script: Option<&str>) -> String {
    let mut s = format!("[scene]\nname = \"{name}\"\n\n[entities.marker_{name}]\n");
    s.push_str("[entities.marker_");
    s.push_str(name);
    s.push_str(".transform]\nposition = [0.0, 0.0, 0.0]\n");
    if let Some(script) = script {
        s.push_str(&format!(
            "\n[entities.controller]\n[entities.controller.script]\nsource = \"{script}\"\n"
        ));
    }
    s
}

fn game() -> MemorySource {
    let mut files = MemorySource::new();
    files.insert("game/flint.toml", "");
    files.insert("game/scenes/a.scene.toml", scene("a", Some("leave.rhai")));
    files.insert("game/scenes/b.scene.toml", scene("b", None));
    files.insert("game/scenes/hud.scene.toml", scene("hud", Some("hud.rhai")));
    files.insert("game/scripts/leave.rhai", LEAVE);
    files.insert("game/scripts/hud.rhai", HUD);
    files
}

fn started_in(path: &str) -> PlayerApp {
    let registry = flint_schema::SchemaRegistry::new();
    let (world, file) = flint_scene::load_scene(path, &registry).expect("the scene loads");
    let mut app = PlayerApp::new(world, path.into(), false, None, None);
    app.apply_scene_settings(&file);
    app.set_schemas(
        registry,
        SchemaDirs {
            dirs: Vec::new(),
            source: SchemaSource::Flags,
            dropped: Vec::new(),
        },
    );
    app.enter_scene(SceneEntry::Start)
        .expect("the scene starts");
    app
}

fn is_in(app: &PlayerApp, scene: &str) -> bool {
    app.core.scene_path.ends_with(scene)
}

#[test]
fn a_bare_load_scene_reaches_the_next_scene() {
    vfs::scoped(Arc::new(game()), || {
        let mut app = started_in("game/scenes/a.scene.toml");
        assert!(app.core.world.get_id("controller").is_some());
        assert!(
            !app.core.script.can_complete_transition(),
            "leave.rhai never calls complete_transition"
        );

        ScriptHost::load_scene(&mut app, "b.scene.toml".into());
        assert!(matches!(
            app.transition_phase,
            TransitionPhase::Exiting { .. }
        ));

        // The next frame's advance: loaded, and nothing to fade back in
        app.advance_transition();
        assert!(is_in(&app, "b.scene.toml"), "{}", app.core.scene_path);
        assert!(app.core.world.get_id("marker_b").is_some());
        assert!(app.core.world.get_id("marker_a").is_none());
        assert!(app.transition_phase.is_idle());

        // ...so the next load_scene is heard, not dropped as mid-transition
        ScriptHost::load_scene(&mut app, "a.scene.toml".into());
        app.advance_transition();
        assert!(is_in(&app, "a.scene.toml"), "{}", app.core.scene_path);
        assert!(app.transition_phase.is_idle());
    });
}

#[test]
fn a_scene_that_draws_its_fades_waits_for_them() {
    vfs::scoped(Arc::new(game()), || {
        let mut app = started_in("game/scenes/hud.scene.toml");
        assert!(app.core.script.can_complete_transition());

        ScriptHost::reload_scene(&mut app);
        for _ in 0..3 {
            app.advance_transition();
            assert!(
                matches!(app.transition_phase, TransitionPhase::Exiting { .. }),
                "the HUD is still fading out"
            );
        }
        assert!(app.core.world.get_id("marker_hud").is_some());

        // The HUD's complete_transition()
        ScriptHost::fire_event(
            &mut app,
            TRANSITION_COMPLETE.into(),
            toml::Value::Boolean(true),
        );
        assert!(matches!(
            app.transition_phase,
            TransitionPhase::Loading { .. }
        ));
        app.advance_transition();
        assert!(
            matches!(app.transition_phase, TransitionPhase::Entering { .. }),
            "the new scene's HUD fades it in"
        );
        app.advance_transition();
        assert!(matches!(
            app.transition_phase,
            TransitionPhase::Entering { .. }
        ));
        ScriptHost::fire_event(
            &mut app,
            TRANSITION_COMPLETE.into(),
            toml::Value::Boolean(true),
        );
        assert!(app.transition_phase.is_idle());
    });
}

#[test]
fn a_fade_that_never_finishes_is_ended_after_the_timeout() {
    vfs::scoped(Arc::new(game()), || {
        let mut app = started_in("game/scenes/hud.scene.toml");

        app.transition_phase = TransitionPhase::Exiting {
            target_scene: "b.scene.toml".into(),
            elapsed: TRANSITION_PHASE_TIMEOUT - 1.0,
        };
        app.advance_transition();
        assert!(matches!(
            app.transition_phase,
            TransitionPhase::Exiting { .. }
        ));

        app.transition_phase = TransitionPhase::Exiting {
            target_scene: "b.scene.toml".into(),
            elapsed: TRANSITION_PHASE_TIMEOUT,
        };
        app.advance_transition();
        assert!(is_in(&app, "b.scene.toml"), "{}", app.core.scene_path);
        // b has no HUD: nothing to wait for on the way in
        assert!(app.transition_phase.is_idle());

        // A HUD that never ends its fade-in
        app.transition_phase = TransitionPhase::Loading {
            target_scene: "hud.scene.toml".into(),
        };
        app.advance_transition();
        assert!(matches!(
            app.transition_phase,
            TransitionPhase::Entering { .. }
        ));
        app.transition_phase = TransitionPhase::Entering {
            elapsed: TRANSITION_PHASE_TIMEOUT,
        };
        app.advance_transition();
        assert!(app.transition_phase.is_idle());
    });
}
