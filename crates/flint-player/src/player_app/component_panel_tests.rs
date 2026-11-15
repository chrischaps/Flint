//! A game's own schema gives its component an F3 panel, with no Rust
//! (#744). The game is a `MemorySource` with its own `flint.toml` and
//! `schemas/`, read through the one schema resolver as `flint-player` reads
//! it, so the panel comes from nothing but the schema's `panel` lines.

use super::PlayerApp;
use flint_core::vfs::{self, MemorySource};
use flint_debug_ui::{ComponentPanel, DebugPanel};
use std::path::Path;
use std::sync::Arc;

const SCENE: &str = "coast/scenes/harbour.scene.toml";

/// A lighthouse's keeper script would publish `lit` and `beam`, ring the
/// bell when `ring` is set and clear it; the panel is a status line, a
/// doorbell and one dial.
const BEACON: &str = r#"
[component.beacon.panel]
title = "Lighthouse"

[component.beacon.fields]
lit = { type = "bool", default = false, description = "PUBLISHED: the lamp is burning", panel = { role = "status", label = "lamp" } }
beam = { type = "f32", default = 0.0, panel = { role = "status", format = "beam {:.0}%", scale = 100.0 } }
ring = { type = "bool", default = false, panel = { role = "trigger", label = "Ring the bell" } }
sweep_s = { type = "f32", default = 12.0, min = 1.0, max = 60.0, panel = { role = "tune", slider = true } }
keeper = { type = "string", default = "" }
"#;

const HARBOUR: &str = r#"
[scene]
name = "Harbour"

[entities.lighthouse]
[entities.lighthouse.beacon]
lit = true
beam = 0.4
sweep_s = 8.0

[entities.buoy]
"#;

fn coast() -> MemorySource {
    let mut files = MemorySource::new();
    files.insert("coast/flint.toml", "[project]\nschemas = [\"schemas\"]\n");
    files.insert("coast/schemas/components/beacon.toml", BEACON);
    files.insert(SCENE, HARBOUR);
    files
}

/// The scene brought up as `flint-player` brings it up: the schemas the
/// project's manifest names, the scene loaded against them, the scene's
/// panels made.
fn harbour() -> PlayerApp {
    let (registry, dirs) =
        flint_schema::SchemaRegistry::resolve(Some(Path::new(SCENE)), &[] as &[&str])
            .expect("the coast's schemas resolve");
    let (world, file) = flint_scene::load_scene(SCENE, &registry).expect("the harbour loads");
    let mut app = PlayerApp::new(world, SCENE.into(), false, None, None);
    app.apply_scene_settings(&file);
    app.set_schemas(registry, dirs);
    app.create_component_panels();
    app
}

fn beacon(app: &PlayerApp, field: &str) -> toml::Value {
    let id = app.core.world.get_id("lighthouse").unwrap();
    app.core
        .world
        .get_components(id)
        .unwrap()
        .get("beacon")
        .unwrap()[field]
        .clone()
}

#[test]
fn a_games_own_schema_gives_its_component_an_f3_panel() {
    vfs::scoped(Arc::new(coast()), || {
        let mut app = harbour();
        let names: Vec<&str> = app.debug_panels.iter().map(|p| p.name()).collect();
        assert_eq!(names, ["Lighthouse"], "one entity carries a beacon");

        // F3 opens it; the status line is the live component. The panels
        // come out of the app for the test, as the frame's sync lends them
        let mut panels = std::mem::take(&mut app.debug_panels);
        let panel = panels[0]
            .as_any_mut()
            .downcast_mut::<ComponentPanel>()
            .expect("a component panel");
        panel.toggle();
        panel.sync_world(&mut app.core.world);
        assert_eq!(panel.status_line(), "lamp on  ·  beam 40%");

        // It draws, and drawing writes nothing
        let ctx = egui::Context::default();
        flint_debug_ui::run_headless_frame(&ctx, egui::RawInput::default(), |ui| panel.ui(ui));
        assert!(!panel.is_dirty());

        // The bell: one write into the world at the next sync
        assert!(panel.press("ring", None));
        panel.sync_world(&mut app.core.world);
        assert_eq!(beacon(&app, "ring"), toml::Value::Boolean(true));
        assert_eq!(
            beacon(&app, "sweep_s"),
            toml::Value::Float(8.0),
            "nothing else written"
        );
    });
}

#[test]
fn a_scene_with_no_marked_component_has_no_component_panel() {
    let mut files = coast();
    files.insert(
        "coast/schemas/components/beacon.toml",
        BEACON
            .replace("panel = ", "x_panel = ")
            .replace("[component.beacon.panel]\ntitle = \"Lighthouse\"\n", ""),
    );
    vfs::scoped(Arc::new(files), || {
        let app = harbour();
        assert!(app.debug_panels.is_empty());
    });
}
