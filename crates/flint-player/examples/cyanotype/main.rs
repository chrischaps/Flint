//! A game binary with a Rust style of its own (#419).
//!
//! The cyanotype style (cyanotype.rs, two passes in cyanotype.wgsl) isn't
//! in the engine. The game embeds `PlayerApp`, and its extension hands the
//! style's descriptor to the player in `PlayerExtension::styles`; from then
//! on it is a style like a built-in: F4 lists it under Styles (tagged
//! `game`), the scene's `[post_process.styles.cyanotype]` table switches it
//! on, and `--style cyanotype:...` sets it from the command line.
//!
//! ```text
//! cargo run -p flint-player --example cyanotype
//! cargo run -p flint-player --example cyanotype -- --style cyanotype:process=print
//! cargo run -p flint-player --example cyanotype -- --screenshot print.png
//! cargo run -p flint-player --example cyanotype -- --scene examples/double_sided.scene.toml \
//!     --style cyanotype
//! ```
//!
//! `--scene` plays another scene under the game's styles; `--input-script`
//! plays timed keys (`tap F4` opens the Rendering & Effects menu), and
//! `--screenshot <png>` [`--at <secs>`] takes one frame and exits. A game in
//! its own repository is the same code in its own crate, with the engine
//! as a subtree (book: guides/player-extensions.md).

mod cyanotype;

use anyhow::Context;
use flint_player::{CaptureConfig, PlayerApp, PlayerExtension};
use flint_render::stylize::StyleDescriptor;
use winit::event_loop::{ControlFlow, EventLoop};

/// The game's extension: here, only its styles.
struct SunPrints;

impl PlayerExtension for SunPrints {
    fn styles(&self) -> Vec<&'static StyleDescriptor> {
        vec![&cyanotype::STYLE]
    }
}

fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::new("warn"))
        .init();

    let args: Vec<String> = std::env::args().skip(1).collect();
    let value = |flag: &str| {
        args.iter()
            .position(|a| a == flag)
            .and_then(|i| args.get(i + 1))
            .cloned()
    };
    let styles: Vec<String> = args
        .windows(2)
        .filter(|w| w[0] == "--style")
        .map(|w| w[1].clone())
        .collect();
    let screenshot = value("--screenshot").map(std::path::PathBuf::from);
    let at: f64 = value("--at")
        .map(|s| s.parse().context("--at: seconds"))
        .transpose()?
        .unwrap_or(0.5);

    let scene = value("--scene").unwrap_or_else(|| {
        concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/examples/cyanotype/atkins.scene.toml"
        )
        .to_string()
    });
    let path = std::path::Path::new(&scene);
    let (schemas, schema_dirs) = flint_schema::SchemaRegistry::resolve(Some(path), &[] as &[&str])
        .context("loading schemas")?;
    let (world, scene_file) =
        flint_scene::load_scene(&scene, &schemas).context("loading the scene")?;

    let mut player = PlayerApp::new(
        world,
        scene.clone(),
        false,
        None,
        scene_file.scene.input_config.clone(),
    );
    player.apply_scene_settings(&scene_file);
    // Before the event loop: the scene's `[post_process]` is read against
    // the registry when the renderer is made, and a `--style` word too.
    player.add_extension(Box::new(SunPrints));
    // So a typo in a word stops here, as it does in flint-player
    flint_render::stylize::check_style_specs(&styles).map_err(anyhow::Error::msg)?;
    player.launch.style_specs = styles;
    if screenshot.is_some() {
        player.capture = CaptureConfig::new(screenshot, at, None, Some(at + 0.5));
    }
    if let Some(script) = value("--input-script") {
        player.capture.input_script = Some(flint_player::InputScript::load(&script)?);
    }
    player.set_schemas(schemas, schema_dirs);

    let event_loop = EventLoop::new()?;
    event_loop.set_control_flow(ControlFlow::Poll);
    event_loop.run_app(&mut player)?;
    if let Some(failure) = player.run_failure() {
        anyhow::bail!(failure);
    }
    Ok(())
}
