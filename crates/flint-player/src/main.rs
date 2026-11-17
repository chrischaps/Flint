//! Flint Player - Standalone game player binary
//!
//! Launches a first-person game session from a scene file with physics.
//!
//! Usage:
//!   flint-player <scene.toml> [--schemas <path>] [--fullscreen]
//!                [--width <px>] [--height <px>]
//!                [--screenshot <out.png>] [--screenshot-at <secs>]
//!                [--screenshot-every <secs>] [--exit-after <secs>]
//!                [--frame-step <secs>] [--input-script <file>]
//!                [--expression-timeline <file>] [--expression-listen <ip:port>]
//!                [--hitch-log <file>] [--max-hitches <n>]
//!                [--capture-frame-at <secs>] [--capture-on-hitch[=<n>]]
//!                [--capture-on-hitch-startup]
//!   flint-player --bundle <game.flintpack> [scene.toml]
//!
//! Every flag but `--bundle` is `flint_player::PlayerArgs`, which `flint
//! play` flattens too, and both launch through `flint_player::launch`
//! (#705), so a scene plays the same under either.
//!
//! Screenshots capture the presented frame — the 3D scene *and* the egui
//! HUD — right before `present()`, so they show exactly what a player sees.
//! `flint render` remains the headless, script-free snapshot; this path is
//! for verifying script-driven UI and live gameplay state.

use anyhow::Result;
use clap::Parser;

fn main() -> Result<()> {
    let cli = flint_player::PlayerCli::parse();
    // Everything the player says, to stderr, as `--log-format` and
    // `--quiet` ask (#716)
    flint_player::logging::init(&cli.args.log);
    cli.run()
}
