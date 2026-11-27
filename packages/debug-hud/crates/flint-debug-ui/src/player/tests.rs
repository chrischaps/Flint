//! The HUD's scene panels, held without a player (#1559): what a scene
//! gets, and what outlives it.

use super::*;
use crate::STYLES_DEBUG_PANEL;
use flint_ecs::FlintWorld;
use flint_schema::SchemaRegistry;

fn scene<'a>(
    world: &'a FlintWorld,
    schemas: &'a SchemaRegistry,
    authored: StyleSettings,
) -> HudScene<'a> {
    let project = flint_core::project::Project::at(env!("CARGO_MANIFEST_DIR")).unwrap();
    HudScene {
        world,
        scene_path: "panels_test.scene.toml",
        grass: None,
        authored,
        assets: flint_core::project::AssetScope::new(project, env!("CARGO_MANIFEST_DIR")),
        looks: Vec::new(),
        current_look: None,
        scene_file: None,
        schemas,
        packages: Vec::new(),
    }
}

fn styled() -> StyleSettings {
    let mut authored = StyleSettings::default();
    authored.set_enabled("ascii", true);
    authored
}

fn names(panels: &DebugPanels) -> Vec<&str> {
    panels.iter().map(|p| p.name()).collect()
}

/// A panel the player keeps across scenes, as a game's is.
struct Kept(bool);

impl DebugPanel for Kept {
    fn name(&self) -> &str {
        "Net"
    }
    fn ui(&mut self, _ui: &mut egui::Ui) {}
    fn is_open(&self) -> bool {
        self.0
    }
    fn toggle(&mut self) {
        self.0 = !self.0;
    }
    fn outlives_scene(&self) -> bool {
        true
    }
    fn is_dirty(&self) -> bool {
        false
    }
    fn clear_dirty(&mut self) {}
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
    fn as_any_mut(&mut self) -> &mut dyn std::any::Any {
        self
    }
}

#[test]
fn a_styled_scene_gets_the_f3_styles_panel_and_a_plain_one_does_not() {
    let (world, schemas) = (FlintWorld::new(), SchemaRegistry::new());
    let mut hud = Hud::default();

    let mut plain = DebugPanels::default();
    hud.scene_panels(
        &mut plain,
        scene(&world, &schemas, StyleSettings::default()),
    );
    assert_eq!(names(&plain), [RENDER_DEBUG_PANEL], "no style, F4's alone");

    let mut panels = DebugPanels::default();
    hud.scene_panels(&mut panels, scene(&world, &schemas, styled()));
    assert_eq!(names(&panels), [STYLES_DEBUG_PANEL, RENDER_DEBUG_PANEL]);
}

#[test]
fn f4_and_the_games_panels_outlive_a_scene_transition() {
    let (world, schemas) = (FlintWorld::new(), SchemaRegistry::new());
    let mut hud = Hud::default();
    let mut panels = DebugPanels::default();
    hud.scene_panels(&mut panels, scene(&world, &schemas, styled()));
    panels.push(Box::new(Kept(true)));
    // F4 is open going into the transition, F3's scene panels are not
    assert_eq!(hud.toggle(&mut panels, Toggle::Render), Some(true));

    hud.scene_panels(
        &mut panels,
        scene(&world, &schemas, StyleSettings::default()),
    );
    // The new scene's panels don't make a second F4, and it stays open
    assert_eq!(names(&panels), [RENDER_DEBUG_PANEL, "Net"]);
    assert!(panels[0].is_open());
    assert!(panels[1].is_open(), "the game's panel stays open too");
}

#[test]
fn f3_toggles_every_panel_but_f4s_and_says_when_there_are_none() {
    let (world, schemas) = (FlintWorld::new(), SchemaRegistry::new());
    let mut hud = Hud::default();
    let mut panels = DebugPanels::default();
    hud.scene_panels(
        &mut panels,
        scene(&world, &schemas, StyleSettings::default()),
    );
    assert_eq!(hud.toggle(&mut panels, Toggle::Scene), None, "F4's alone");

    hud.scene_panels(&mut panels, scene(&world, &schemas, styled()));
    assert_eq!(hud.toggle(&mut panels, Toggle::Scene), Some(true));
    // F4 kept its place from the scene before, so it comes first
    assert_eq!(names(&panels), [RENDER_DEBUG_PANEL, STYLES_DEBUG_PANEL]);
    let open: Vec<bool> = panels.iter().map(|p| p.is_open()).collect();
    assert_eq!(open, [false, true], "the Styles panel opened, F4 didn't");
    assert_eq!(hud.toggle(&mut panels, Toggle::Scene), Some(false));
}
