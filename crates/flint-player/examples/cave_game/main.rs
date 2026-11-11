//! A game binary with a procgen generator of its own (#195, ADR 0119).
//!
//! The cave game's rocks come from `rock_v1` (rock.rs), which the engine
//! doesn't ship. The game embeds `PlayerApp`, and its extension hands the
//! generator to the player in `register_generators`; from then on a scene's
//! `*.procgen.toml` naming `generator = "rock_v1"` resolves like a tree.
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
use flint_player::{CaptureConfig, PlayerApp, PlayerExtension};
use flint_procgen::GeneratorRegistry;
use winit::event_loop::{ControlFlow, EventLoop};

/// The game's extension: here, only its generators.
struct CaveGame;

impl PlayerExtension for CaveGame {
    fn register_generators(&mut self, registry: &mut GeneratorRegistry) {
        registry.register(Box::new(rock::RockGenerator));
    }
}

fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::new("warn"))
        .init();

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
    player.add_extension(Box::new(CaveGame));
    player.set_schemas(schemas, schema_dirs);

    let event_loop = EventLoop::new()?;
    event_loop.set_control_flow(ControlFlow::Poll);
    event_loop.run_app(&mut player)?;
    if let Some(failure) = player.run_failure() {
        anyhow::bail!(failure);
    }
    Ok(())
}
