//! Play command — launches the game player for a scene
//!
//! `flint play` is `flint-player` without `--bundle` (#705): both flatten
//! `flint_player::PlayerArgs` and launch through `flint_player::launch`,
//! so a flag, a scene setting or a capture switch reaches the two alike.

use anyhow::Result;

#[derive(clap::Args)]
#[command(after_help = flint_core::env::help_section(&flint_core::env::PLAYER_GROUPS))]
pub struct PlayArgs {
    /// Every flag `flint-player` takes but `--bundle`
    #[command(flatten)]
    pub player: flint_player::PlayerArgs,
}

pub fn run(mut args: PlayArgs) -> Result<()> {
    // `--list-keys --json` reads flint's global `--json` (#1184): the
    // player's own is `flint-player`'s, kept off the flattened args
    args.player.json = crate::output::json();
    flint_player::launch(args.player)
}

#[cfg(test)]
mod tests {
    use super::PlayArgs;
    use clap::{CommandFactory, Parser};

    /// `flint play` as its own command line, for parsing and its help
    #[derive(clap::Parser)]
    #[command(name = "flint-play")]
    struct Play {
        #[command(flatten)]
        args: PlayArgs,
    }

    /// Each argument's id, long name and help, in the order `--help` lists
    /// them
    fn arguments(cmd: &clap::Command) -> Vec<(String, Option<String>, String)> {
        cmd.get_arguments()
            .map(|a| {
                (
                    a.get_id().to_string(),
                    a.get_long().map(str::to_string),
                    a.get_help().map(|h| h.to_string()).unwrap_or_default(),
                )
            })
            .collect()
    }

    /// The issue's second box: the two help texts differ only by
    /// `--bundle` and `--json` (the global `--project` and `--json` of
    /// every `flint` command are flint's own, not the play command's)
    #[test]
    fn flint_play_takes_every_flint_player_flag_but_bundle() {
        let play = arguments(&Play::command());
        let player: Vec<_> = arguments(&flint_player::PlayerCli::command())
            .into_iter()
            .filter(|(id, ..)| id != "bundle" && id != "json")
            .collect();
        assert_eq!(play, player);
        for flag in [
            "screenshot",
            "exit-after",
            "frame-step",
            "msaa",
            "quality",
            "look",
        ] {
            assert!(
                play.iter()
                    .any(|(_, long, _)| long.as_deref() == Some(flag)),
                "--{flag}"
            );
        }
    }

    /// #1184: `flint play --json` is flint's global flag, which a `--json`
    /// of the player's flattened in would take from it (and then ask for
    /// `--list-keys`), so `--list-keys --json` reads `output::json()` and
    /// a plain `flint play --json` reaches the window refusal
    #[test]
    fn flint_play_leaves_json_to_the_global_flag() {
        for words in [
            &["flint", "play", "--json"][..],
            &["flint", "play", "--list-keys", "--json"],
            &["flint", "--json", "play", "--list-keys"],
        ] {
            let cli = crate::Cli::try_parse_from(words)
                .unwrap_or_else(|e| panic!("{words:?}: {}", e.render()));
            assert!(cli.json, "{words:?}");
            let crate::Commands::Play(args) = &cli.command else {
                panic!("{words:?} is not play");
            };
            assert!(!args.player.json, "{words:?}: the player's own is unset");
            let list_keys = words.contains(&"--list-keys");
            assert_eq!(cli.command.interactive().is_none(), list_keys, "{words:?}");
        }
    }

    /// `flint-player` keeps its own `--json`, still only with `--list-keys`
    #[test]
    fn flint_player_takes_its_own_json_with_list_keys() {
        let cli =
            flint_player::PlayerCli::try_parse_from(["flint-player", "--list-keys", "--json"])
                .unwrap();
        assert!(cli.json && cli.args.list_keys);
        let alone = flint_player::PlayerCli::try_parse_from(["flint-player", "--json"])
            .expect_err("--json alone is refused");
        assert!(alone.to_string().contains("--list-keys"), "{alone}");
    }

    /// The issue's third box: a scene with `[environment] shadows = false`,
    /// no default lights and a user style in its `[post_process]`, through
    /// both entry points, builds the same player
    #[test]
    fn both_entry_points_play_a_scene_the_same() {
        let dir = std::env::temp_dir().join(format!("flint-play-705-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("styles")).unwrap();
        std::fs::create_dir_all(dir.join("schemas/components")).unwrap();
        std::fs::write(
            dir.join("styles/sepia705.shader.toml"),
            "version = 1
name = \"sepia705\"
stage = \"display\"
shader = \"sepia705.wgsl\"

             [params.amount]
default = 0.5
min = 0.0
max = 1.0
",
        )
        .unwrap();
        std::fs::write(
            dir.join("styles/sepia705.wgsl"),
            "fn style(p: StylePixel) -> vec4<f32> {
                 let g = vec3<f32>(flint_luma(p.color.rgb));
                 return vec4<f32>(mix(p.color.rgb, g, params.amount), p.color.a);
}
",
        )
        .unwrap();
        let scene = dir.join("dim.scene.toml");
        std::fs::write(
            &scene,
            r#"[scene]
name = "dim"

[environment]
shadows = false
default_lights = false
ambient_sky = [0.1, 0.2, 0.3]
diffuse_wrap = 0.4

[post_process.styles.sepia705]
amount = 0.8

[entities.box]
"#,
        )
        .unwrap();
        let scene = scene.to_string_lossy().to_string();
        let schemas = dir.join("schemas").to_string_lossy().to_string();
        let words = [scene.as_str(), "--schemas", schemas.as_str(), "--msaa", "4"];

        let play = Play::try_parse_from(std::iter::once("flint-play").chain(words))
            .unwrap()
            .args
            .player
            .prepare(None)
            .unwrap()
            .app;
        let player =
            flint_player::PlayerCli::try_parse_from(std::iter::once("flint-player").chain(words))
                .unwrap()
                .args
                .prepare(None)
                .unwrap()
                .app;

        for app in [&play, &player] {
            assert!(!app.scene.shadows, "shadows = false");
            assert!(!app.scene.default_lights, "default_lights = false");
            assert_eq!(app.launch.msaa_sample_count, 4);
            assert!(
                app.scene_post_config().styles.is_enabled("sepia705"),
                "the scene's user style survives into its post config"
            );
        }
        assert_eq!(play.scene.ambient, player.scene.ambient);
        assert_eq!(play.scene.diffuse_wrap, player.scene.diffuse_wrap);
        assert_eq!(
            format!("{:?}", play.scene_post_config()),
            format!("{:?}", player.scene_post_config())
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}
