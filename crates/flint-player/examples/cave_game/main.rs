//! A game binary with a procgen generator of its own (#195, ADR 0119).
//!
//! The cave game's rocks come from `rock_v1` (rock.rs), which the engine
//! doesn't ship. The game embeds `PlayerApp`, and its extension hands the
//! generator to the player in `register_generators`; from then on a scene's
//! `*.procgen.toml` naming `generator = "rock_v1"` resolves like a tree.
//!
//! It also draws its own egui (ADR 0121): a survey of the stones in a side
//! panel and a field note in a floating window, both from
//! `PlayerExtension::ui` on the frame's root `egui::Ui`.
//!
//! ```text
//! cargo run -p flint-player --example cave_game
//! cargo run -p flint-player --example cave_game -- --screenshot cave.png
//! ```
//!
//! A game in its own repository is the same code in its own crate, with
//! the engine as a subtree (book: guides/player-extensions.md).

mod rock;

use anyhow::Context;
use flint_player::{CaptureConfig, ExtensionCtx, PlayerApp, PlayerExtension};
use flint_procgen::GeneratorRegistry;
use winit::event_loop::{ControlFlow, EventLoop};

/// The game's extension: its generator, and its egui.
#[derive(Default)]
struct CaveGame {
    /// The stones the scene placed (entity, mesh), read from the world in
    /// `pre_update` for the survey panel `ui` draws.
    stones: Vec<(String, String)>,
}

impl PlayerExtension for CaveGame {
    fn register_generators(&mut self, registry: &mut GeneratorRegistry) {
        registry.register(Box::new(rock::RockGenerator));
    }

    fn pre_update(&mut self, ctx: &mut ExtensionCtx) {
        // The cave is one scene, placed once
        if !self.stones.is_empty() {
            return;
        }
        self.stones = ctx
            .world
            .all_entities()
            .into_iter()
            .filter_map(|e| {
                let asset = ctx.world.view(e.id, "model").opt_str("asset")?;
                asset
                    .starts_with("cave_")
                    .then(|| (e.name, asset.trim_start_matches("cave_").to_string()))
            })
            .collect();
    }

    /// `ui` is the root `Ui`: a panel shows on it, a window from it.
    fn ui(&mut self, ui: &mut egui::Ui) {
        egui::Panel::left("survey")
            .exact_size(210.0)
            .show(ui, |ui| {
                ui.heading("Survey");
                ui.label(format!("{} stones, one generator", self.stones.len()));
                ui.separator();
                egui::Grid::new("stones").striped(true).show(ui, |ui| {
                    for (name, mesh) in &self.stones {
                        ui.label(name);
                        ui.weak(mesh);
                        ui.end_row();
                    }
                });
            });
        egui::Window::new("Field note")
            .default_pos(egui::pos2(ui.max_rect().right() - 330.0, 24.0))
            .default_width(290.0)
            .resizable(false)
            .show(ui, |ui| {
                ui.label(
                    "Every stone in this cave is rock_v1, a generator the game \
                     brought with it. The engine ships nothing like it.",
                );
            });
    }
}

fn main() -> anyhow::Result<()> {
    // The player's log (#716): the load summary, the controls line and
    // what scripts print, on stderr, as `flint play` says them
    flint_player::logging::init(&flint_player::logging::LogArgs::default());

    // `--screenshot <png>`: one frame half a second in, then exit.
    let args: Vec<String> = std::env::args().skip(1).collect();
    let screenshot = args
        .iter()
        .position(|a| a == "--screenshot")
        .and_then(|i| args.get(i + 1))
        .map(std::path::PathBuf::from);

    let scene = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/examples/cave_game/cave.scene.toml"
    );
    let path = std::path::Path::new(scene);
    let (schemas, schema_dirs) = flint_schema::SchemaRegistry::resolve(Some(path), &[] as &[&str])
        .context("loading schemas")?;
    let (world, scene_file) =
        flint_scene::load_scene(scene, &schemas).context("loading the cave")?;

    let mut player = PlayerApp::new(
        world,
        scene.into(),
        false,
        None,
        scene_file.scene.input_config.clone(),
    );
    player.apply_scene_settings(&scene_file);
    if screenshot.is_some() {
        player.capture = CaptureConfig::new(screenshot, 0.5, None, Some(1.0));
    }
    // Before the event loop: the first scene's specs resolve as it loads.
    player.add_extension(Box::new(CaveGame::default()));
    player.set_schemas(schemas, schema_dirs);

    let event_loop = EventLoop::new()?;
    event_loop.set_control_flow(ControlFlow::Poll);
    event_loop.run_app(&mut player)?;
    if let Some(failure) = player.run_failure() {
        anyhow::bail!(failure);
    }
    Ok(())
}
