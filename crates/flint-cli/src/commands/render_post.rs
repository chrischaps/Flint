//! What `flint render`'s post-processing flags write (#708): `--set`.
//!
//! `--set` takes what `--sweep` takes, with one value instead of a list:
//! `post.<key>=v` is a `[post_process]` key (the effects' descriptors, #741,
//! are its list and its kinds), and `<style>.<key>=v` a key of a style's
//! table, as the scene's `[post_process.styles.<style>]` writes it, without
//! turning the style on (`--style` does that). The per-effect flags it
//! replaced were hidden aliases for a release and are gone (#1144);
//! [`super::render_retired`] names each one's spelling when it is given.
//!
//! A render's post-processing is built in this order: the scene's block, the
//! look and a sweep column's `post.` value (as a `[post_process]` table); then
//! `--no-postprocess`, the deprecated `--render-mode`, `--set` in the order
//! given, and `--style` last, so an explicit word wins.

use super::render::RenderArgs;
use crate::commands::common_args::parse_vec4;
use anyhow::Result;
use flint_render::stylize::{StyleRegistry, StyleSpec};
use flint_render::PostProcessConfig;
use flint_schema::post_effects::{self, PostParam};

/// One `--set`: a key and the one value it takes.
#[derive(Debug, Clone, PartialEq)]
pub struct Set {
    pub key: SetKey,
    /// As written, after the `=`.
    pub value: String,
}

#[derive(Debug, Clone, PartialEq)]
pub enum SetKey {
    /// `post.<key>`: a built-in effect's `[post_process]` key.
    Post(&'static str),
    /// `<style>.<key>`: a key of a style's table. Checked against the
    /// registry once the scene's user styles are in ([`Set::check`]), as
    /// `--style` is.
    Style { style: String, key: String },
}

impl Set {
    /// The key as written: `post.exposure`, `ascii.cell`. A `--sweep` of
    /// the same key has the same name.
    pub fn name(&self) -> String {
        match &self.key {
            SetKey::Post(key) => format!("post.{key}"),
            SetKey::Style { style, key } => format!("{style}.{key}"),
        }
    }

    /// A style key as the `--style` word that would also turn the style on:
    /// what [`StyleSpec`] checks and parses it with.
    fn style_word(&self) -> Option<String> {
        match &self.key {
            SetKey::Style { style, key } => Some(format!("{style}:{key}={}", self.value)),
            SetKey::Post(_) => None,
        }
    }

    fn at(&self) -> String {
        format!("--set {}={}", self.name(), self.value)
    }

    /// Check a style key against the registry, user styles included: call
    /// it once the scene's folder has registered them.
    pub fn check(&self, styles: &StyleRegistry) -> Result<(), String> {
        match self.style_word() {
            Some(word) => StyleSpec::parse(&word, styles)
                .map(drop)
                .map_err(|e| format!("{}: {e}", self.at())),
            None => Ok(()),
        }
    }

    /// Write the value into `config`. A post value past what its effect is
    /// tuned for still draws, and says so, as the scene's would.
    pub fn apply(&self, config: &mut PostProcessConfig, styles: &StyleRegistry) -> Result<()> {
        match &self.key {
            SetKey::Post(key) => {
                let value = super::sweep::parse_value(&self.value);
                if let Some(range) = post_effects::param(key).and_then(|p| p.range_error(&value)) {
                    eprintln!("warning: {}: {range} (it still draws)", self.at());
                }
                config
                    .param_mut(key)
                    .expect("every descriptor key is a config field")
                    .set(&value)
                    .map_err(|e| anyhow::anyhow!("{}: {key} {e}", self.at()))?;
            }
            SetKey::Style { .. } => {
                let word = self.style_word().expect("a style key");
                StyleSpec::parse(&word, styles)
                    .map_err(|e| anyhow::anyhow!("{}: {e}", self.at()))?
                    .apply_settings(config, styles);
            }
        }
        Ok(())
    }
}

/// `--set`'s value parser: `post.<key>=v` or `<style>.<key>=v`. A post key
/// and its value's kind are checked here; a style key once the scene's user
/// styles are registered ([`Set::check`]).
pub fn parse_set(text: &str) -> Result<Set, String> {
    let (name, value) = text
        .split_once('=')
        .map(|(k, v)| (k.trim(), v.trim()))
        .filter(|(k, v)| !k.is_empty() && !v.is_empty())
        .ok_or_else(|| {
            format!(
                "'{text}' is not key=value: post.<key>=v (`flint render --list-effects`) or \
                 <style>.<key>=v (`--list-styles`)"
            )
        })?;
    let (head, key) = name.split_once('.').ok_or_else(|| {
        format!("'{name}' is not post.<key> or <style>.<key> (`--list-effects`, `--list-styles`)")
    })?;
    let key = if head == "post" {
        let param = post_param(key)?;
        // The kind now, so a typo'd colour fails before any GPU work
        PostProcessConfig::default()
            .param_mut(param.key)
            .expect("every descriptor key is a config field")
            .set(&super::sweep::parse_value(value))
            .map_err(|e| format!("--set {name}={value}: {} {e}", param.key))?;
        SetKey::Post(param.key)
    } else {
        if matches!(key, "enabled" | "on" | "off") {
            return Err(format!(
                "--set {name}: --set tunes a style and leaves it on or off as it was; \
                 `--style {head}` turns it on and `--style {head}:off` off"
            ));
        }
        SetKey::Style {
            style: head.to_string(),
            key: key.to_string(),
        }
    };
    Ok(Set {
        key,
        value: value.to_string(),
    })
}

/// The descriptor of `post.<key>`, or the key it might have been.
fn post_param(key: &str) -> Result<&'static PostParam, String> {
    post_effects::param(key).ok_or_else(|| {
        let near = flint_core::diagnostic::closest(key, post_effects::params().map(|p| p.key));
        format!(
            "post.{key} is no [post_process] key{} (`flint render --list-effects` lists them)",
            near.map(|n| format!("; did you mean post.{n}?"))
                .unwrap_or_default()
        )
    })
}

/// The deprecated render mode (#746), hidden: what `--set` and `--style`
/// spell is in [`super::render_retired`]. These three go with the Rhai shim
/// they share, `flint_render::render_mode` (#1036), once Float is ported.
#[derive(clap::Args, Debug, Clone, Default)]
pub struct RenderModeArgs {
    /// Deprecated since #746: a render mode is a style. 1 = `--style
    /// matrix:blend_source=tear --set post.tear=M`, 2 = blood, 3 = `--style
    /// drunk:blend=M`, 4 = tron, 5 = `--style underwater:blend=M,depth=..`, 6 =
    /// `--set post.tear=M` alone
    #[arg(long, hide = true)]
    pub render_mode: Option<u32>,
    /// With --render-mode: its mix 0..1 (default 1)
    #[arg(long, hide = true)]
    pub mode_mix: Option<f32>,
    /// With --render-mode: X,Y,Z,W
    #[arg(long, hide = true, value_parser = parse_vec4)]
    pub mode_params: Option<[f32; 4]>,
}

/// Check every `--set` style key (after the scene's user styles are
/// registered), and refuse a `--set` of a key a `--sweep` turns: one of the
/// two would be lost.
pub fn check(args: &RenderArgs, styles: &StyleRegistry) -> Result<()> {
    for s in &args.set {
        s.check(styles).map_err(anyhow::Error::msg)?;
        if let Some(sweep) = args.sweeps.sweep.iter().find(|w| w.name() == s.name()) {
            anyhow::bail!(
                "--set {} and --sweep {}: the sweep turns that key, so drop the --set",
                s.name(),
                sweep.name()
            );
        }
    }
    Ok(())
}

/// The deprecated `--render-mode` (#746), with its mix (1 when not given)
/// and params, as the shim maps it onto the styles that replaced it; warned
/// about once a process.
pub fn legacy_render_mode(args: &RenderArgs) -> Option<flint_render::render_mode::RenderMode> {
    let l = &args.render_mode;
    let mode = l.render_mode?;
    static WARNED: std::sync::Once = std::sync::Once::new();
    WARNED.call_once(|| {
        tracing::warn!(
            "--render-mode is deprecated (#746) and goes in the next release: \
             the modes are styles now (`flint render --list-styles`: matrix, blood, drunk, \
             tron, underwater), torn in by --set post.tear and `blend_source=tear`"
        );
    });
    Some(flint_render::render_mode::RenderMode {
        mode,
        // A mode with no explicit mix defaults to fully torn through
        mix: l.mode_mix.unwrap_or(1.0),
        params: l.mode_params.unwrap_or_default(),
    })
}

/// The command line's post words over `config`: the deprecated render mode,
/// then `--set`. The stage path lays them again over a sweep column's
/// preset, whose `[boil]` they win over.
pub fn apply_flags(
    args: &RenderArgs,
    config: &mut PostProcessConfig,
    styles: &StyleRegistry,
) -> Result<()> {
    if let Some(mode) = legacy_render_mode(args) {
        mode.apply(config);
    }
    for s in &args.set {
        s.apply(config, styles)?;
    }
    Ok(())
}

/// The post-processing a render draws with: the scene's block (under the
/// look and a sweep column, already laid into `def`), then the command line.
pub fn post_process_config(
    args: &RenderArgs,
    def: Option<&flint_scene::PostProcessDef>,
    styles: &StyleRegistry,
) -> Result<PostProcessConfig> {
    let mut config = match def {
        Some(def) => flint_player::post_process_config_from_def(def, styles),
        None => PostProcessConfig::default(),
    };
    if args.no_postprocess {
        config.enabled = false;
    }
    apply_flags(args, &mut config, styles)?;
    // `--style name[:k=v,...]` (#271) last: an explicit word wins
    args.style.apply(&mut config, styles);
    Ok(config)
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;

    #[derive(Parser)]
    struct Cli {
        #[command(flatten)]
        args: RenderArgs,
    }

    fn args(line: &str) -> RenderArgs {
        let words = ["render", "s.scene.toml"]
            .into_iter()
            .chain(line.split_whitespace());
        match Cli::try_parse_from(words) {
            Ok(cli) => cli.args,
            Err(e) => panic!("{line}: {e}"),
        }
    }

    fn config(line: &str) -> String {
        format!(
            "{:?}",
            post_process_config(
                &args(line),
                None,
                &flint_render::stylize::StyleRegistry::new()
            )
            .unwrap()
        )
    }

    #[test]
    fn set_checks_the_key_and_the_kind() {
        let s = parse_set("post.exposure=2").unwrap();
        assert_eq!(s.key, SetKey::Post("exposure"));
        let e = parse_set("post.exposur=2").unwrap_err();
        assert!(e.contains("did you mean post.exposure"), "{e}");
        let e = parse_set("post.fog_color=1,2").unwrap_err();
        assert!(e.contains("three numbers"), "{e}");
        let e = parse_set("post.ssao_samples=1.5").unwrap_err();
        assert!(e.contains("whole number"), "{e}");
        assert!(parse_set("exposure=2").is_err());
        assert!(parse_set("post.exposure").is_err());
        let e = parse_set("ascii.enabled=true").unwrap_err();
        assert!(e.contains("--style ascii"), "{e}");
        // A style key waits for the registry: a user style may be named
        let s = parse_set("my_style.glow=2").unwrap();
        assert!(
            s.check(&flint_render::stylize::StyleRegistry::new())
                .is_err(),
            "an unknown style fails its check"
        );
        assert!(parse_set("ascii.cel=3")
            .unwrap()
            .check(&flint_render::stylize::StyleRegistry::new())
            .is_err());
        assert!(parse_set("ascii.cell=12")
            .unwrap()
            .check(&flint_render::stylize::StyleRegistry::new())
            .is_ok());
    }

    #[test]
    fn a_style_key_tunes_without_turning_the_style_on() {
        let c = post_process_config(
            &args("--set ascii.cell=12"),
            None,
            &flint_render::stylize::StyleRegistry::new(),
        )
        .unwrap();
        assert!(!c.styles.is_enabled("ascii"));
        let on = post_process_config(
            &args("--set ascii.cell=12 --style ascii"),
            None,
            &flint_render::stylize::StyleRegistry::new(),
        )
        .unwrap();
        assert!(on.styles.is_enabled("ascii"));
        assert_eq!(
            format!("{on:?}"),
            config("--style ascii:cell=12"),
            "the key is kept when --style turns it on"
        );
    }

    #[test]
    fn set_applies_in_order_and_style_last() {
        assert_eq!(
            config("--set post.exposure=1 --set post.exposure=3"),
            config("--set post.exposure=3")
        );
        assert_eq!(
            config("--style ascii:cell=8 --set ascii.cell=12"),
            config("--style ascii:cell=8"),
            "--style applies after --set"
        );
    }

    #[test]
    fn a_set_of_a_swept_key_is_refused() {
        let e = check(
            &args("--set post.exposure=2 --sweep post.exposure=1,2"),
            &flint_render::stylize::StyleRegistry::new(),
        )
        .unwrap_err();
        assert!(e.to_string().contains("drop the --set"), "{e}");
        assert!(check(
            &args("--set post.exposure=2 --sweep post.desaturate=0,1"),
            &flint_render::stylize::StyleRegistry::new()
        )
        .is_ok());
    }
}
