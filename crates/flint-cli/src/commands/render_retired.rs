//! The per-effect flags `flint render` had until #1144, and what each is now.
//!
//! #708 replaced 67 hand-mapped flags (`--exposure`, `--bloom-intensity`,
//! `--fog-*`, `--kuwahara-*`, `--contour*`, `--ascii*`, `--tear*`, `--fxaa`...)
//! with `--set post.<key>=v`, `--set <style>.<key>=v` and `--style`, and kept
//! them hidden for a release. They are gone now: clap doesn't know them. This
//! table is what's left, so that the first person to type one learns its
//! spelling. [`explained`] turns clap's unknown-argument error for one into
//! an error naming every old flag on the line with its new words, for the
//! command line (`lib.rs`'s `parse_failed`), a `--variant`'s line and a
//! golden case's.
//!
//! The deprecated `--render-mode`, `--mode-mix` and `--mode-params` (#746)
//! are still flags (`render_post::RenderModeArgs`); #1036 retires them with
//! the Rhai shim they share.

use std::ffi::OsStr;

/// What one old flag is now. `v` is the value it was given.
#[derive(Debug, Clone, Copy)]
enum Now {
    /// `--set post.<key>=v`.
    Post(&'static str),
    /// `--set post.<key>=v --set post.<switch>=true`: a strength that also
    /// turned its effect on.
    PostOn(&'static str, &'static str),
    /// `--set post.<key>=v --set post.<switch>=<v > 0>`.
    PostAbove0(&'static str, &'static str),
    /// `--set post.<key>=v`, and `--set post.<switch>=false` when `v` is 0
    /// or less (SSAO's intensity).
    PostOffAt0(&'static str, &'static str),
    /// A switch: `--set post.<key>=true`.
    PostSwitch(&'static str),
    /// `--style <style>:<key>=v`, which turns the style on.
    Style(&'static str, &'static str),
    /// `--style <style>:<key>=r/g/b`: three numbers as a style takes them.
    StyleRgb(&'static str, &'static str),
    /// `--style ascii:<key>=v,color=<key>`: a palette or a tint picked the
    /// ASCII colour mode, unless `--ascii-color` was given too. `rgb` when
    /// the value is three numbers.
    AsciiColor(&'static str, bool),
    /// `--set <style>.<key>=v`, which leaves the style on or off.
    StyleKey(&'static str, &'static str),
    /// A switch: `--style <style>`.
    StyleOn(&'static str),
    /// A switch: these words.
    Words(&'static str),
}

use Now::*;

/// Every retired flag, without its `--`, and what it is now.
#[rustfmt::skip]
const RETIRED: &[(&str, Now)] = &[
    // The effects
    ("bloom-intensity", PostOn("bloom_intensity", "bloom_enabled")),
    ("bloom-threshold", Post("bloom_threshold")),
    ("exposure", Post("exposure")),
    ("ssao-radius", Post("ssao_radius")),
    ("ssao-intensity", PostOffAt0("ssao_intensity", "ssao_enabled")),
    ("ssao-samples", Post("ssao_samples")),
    ("fog-density", PostAbove0("fog_density", "fog_enabled")),
    ("fog-color", Post("fog_color")),
    ("fog-height-falloff", PostOn("fog_height_falloff", "fog_height_enabled")),
    ("dither-intensity", PostAbove0("dither_intensity", "dither_enabled")),
    ("desaturate", Post("desaturate")),
    ("dof", Post("dof_strength")),
    ("dof-focus", Post("dof_focus_distance")),
    ("dof-range", Post("dof_focus_range")),
    ("tear", Post("tear")),
    ("tear-scale", Post("tear_scale")),
    ("tear-iris", PostSwitch("tear_iris")),
    // Grade, edges, boil, grain
    ("volumetric-density", PostAbove0("volumetric_density", "volumetric_enabled")),
    ("volumetric-samples", Post("volumetric_samples")),
    ("edge-depth", Post("edge_depth")),
    ("edge-normal", Post("edge_normal")),
    ("boil-on", Post("boil_on")),
    ("motion-hold", Post("motion_hold")),
    ("film-grain", Post("film_grain")),
    ("grade-lift", Post("grade_lift")),
    ("grade-gamma", Post("grade_gamma")),
    ("grade-gain", Post("grade_gain")),
    ("fxaa", PostSwitch("fxaa")),
    // Kuwahara and contour
    ("kuwahara-radius", Style("kuwahara", "radius")),
    ("kuwahara-sharpness", Style("kuwahara", "sharpness")),
    ("kuwahara-hardness", Style("kuwahara", "hardness")),
    ("kuwahara-anisotropy", Style("kuwahara", "anisotropy")),
    ("kuwahara-boil", StyleKey("kuwahara", "boil")),
    ("contour", StyleOn("contour")),
    ("contour-width", Style("contour", "width")),
    ("contour-threshold", Style("contour", "threshold")),
    ("contour-strength", Style("contour", "strength")),
    ("contour-luma", Style("contour", "luma")),
    ("contour-fade", Style("contour", "fade")),
    ("contour-boil", Style("contour", "boil")),
    ("contour-color", StyleRgb("contour", "color")),
    // ASCII's look: each turned it on
    ("ascii", StyleOn("ascii")),
    ("ascii-font", Style("ascii", "font")),
    ("ascii-ramp", Style("ascii", "ramp")),
    ("ascii-scope", Style("ascii", "scope")),
    ("ascii-grid", Style("ascii", "grid")),
    ("ascii-color", Style("ascii", "color")),
    ("ascii-ground", Style("ascii", "ground")),
    ("ascii-palette", AsciiColor("palette", false)),
    ("ascii-tint", AsciiColor("tint", true)),
    // ASCII's tuning: these tuned it only when it was on
    ("ascii-cell", StyleKey("ascii", "cell")),
    ("ascii-levels", StyleKey("ascii", "levels")),
    ("ascii-footprint", StyleKey("ascii", "footprint")),
    ("ascii-edge-depth", StyleKey("ascii", "edge_depth")),
    ("ascii-edge-normal", StyleKey("ascii", "edge_normal")),
    ("ascii-no-edges", Words("--set ascii.edges=0")),
    // ...and these turned it on
    ("ascii-background", Style("ascii", "background")),
    ("ascii-brightness", Style("ascii", "brightness")),
    ("ascii-contrast", Style("ascii", "contrast")),
    ("ascii-ink", Style("ascii", "ink")),
    ("ascii-glow", Style("ascii", "glow")),
    ("ascii-edges", Style("ascii", "edges")),
    ("ascii-edge-threshold", Style("ascii", "edge_threshold")),
    ("ascii-edge-max", Style("ascii", "edge_max")),
    ("ascii-dither", Style("ascii", "dither")),
    ("ascii-exposure", Style("ascii", "exposure")),
    ("ascii-normalize", Style("ascii", "normalize")),
];

/// The retired flag `word` names (`--exposure`, `--exposure=1`), if any.
fn retired(word: &str) -> Option<(&'static str, Now)> {
    let name = word.strip_prefix("--")?;
    let name = name.split_once('=').map_or(name, |(n, _)| n);
    RETIRED.iter().find(|(n, _)| *n == name).copied()
}

impl Now {
    /// Whether the flag was a switch, taking no value.
    fn is_switch(self) -> bool {
        matches!(self, PostSwitch(_) | StyleOn(_) | Words(_))
    }

    /// The words the flag is now, given `v` (`<v>` when the line had none).
    /// `ascii_color`: the line also gave `--ascii-color`.
    fn words(self, v: &str, ascii_color: bool) -> String {
        let above0 = || v.parse::<f32>().map_or(true, |x| x > 0.0);
        let slashed = || v.replace(',', "/");
        match self {
            Post(key) => format!("--set post.{key}={v}"),
            PostOn(key, on) => format!("--set post.{key}={v} --set post.{on}=true"),
            PostAbove0(key, on) => format!("--set post.{key}={v} --set post.{on}={}", above0()),
            PostOffAt0(key, on) if !above0() => {
                format!("--set post.{key}={v} --set post.{on}=false")
            }
            PostOffAt0(key, _) => format!("--set post.{key}={v}"),
            PostSwitch(key) => format!("--set post.{key}=true"),
            Style(style, key) => format!("--style {style}:{key}={v}"),
            StyleRgb(style, key) => format!("--style {style}:{key}={}", slashed()),
            AsciiColor(key, rgb) => {
                let v = if rgb { slashed() } else { v.to_string() };
                match ascii_color {
                    true => format!("--style ascii:{key}={v}"),
                    false => format!("--style ascii:{key}={v},color={key}"),
                }
            }
            StyleKey(style, key) => format!("--set {style}.{key}={v}"),
            StyleOn(style) => format!("--style {style}"),
            Words(words) => words.to_string(),
        }
    }
}

/// Every retired flag in `words`, each as written (`--fog-density 0.05`)
/// beside its new words, in the order given.
pub fn spellings<S: AsRef<OsStr>>(words: &[S]) -> Vec<(String, String)> {
    let words: Vec<String> = words
        .iter()
        .map(|w| w.as_ref().to_string_lossy().into_owned())
        .collect();
    let ascii_color = words
        .iter()
        .any(|w| retired(w).is_some_and(|(n, _)| n == "ascii-color"));
    let mut found = Vec::new();
    for (i, word) in words.iter().enumerate() {
        let Some((name, now)) = retired(word) else {
            continue;
        };
        let given = if now.is_switch() {
            None
        } else if let Some((_, v)) = word.split_once('=') {
            Some(v.to_string())
        } else {
            // The next word, unless it is another flag (`-1` is a value)
            words
                .get(i + 1)
                .filter(|next| !next.starts_with("--"))
                .cloned()
        };
        let written = match &given {
            Some(v) if !word.contains('=') => format!("--{name} {v}"),
            _ => word.clone(),
        };
        let v = given.as_deref().unwrap_or("<v>");
        found.push((written, now.words(v, ascii_color)));
    }
    found
}

/// Clap's error for `words` (a `flint render` line, whole or after the
/// subcommand), with a retired flag's spelling: when clap stopped on one,
/// an error naming every old flag on the line and what to write, else `e`.
pub fn explained<S: AsRef<OsStr>>(e: clap::Error, words: &[S]) -> clap::Error {
    use clap::error::{ContextKind, ContextValue, ErrorKind};
    if e.kind() != ErrorKind::UnknownArgument {
        return e;
    }
    let Some(ContextValue::String(arg)) = e.get(ContextKind::InvalidArg) else {
        return e;
    };
    let Some((name, _)) = retired(arg) else {
        return e;
    };
    let found = spellings(words);
    if found.is_empty() {
        return e;
    }
    let wide = found.iter().map(|(w, _)| w.len()).max().unwrap_or(0);
    let mut text = format!(
        "unexpected argument '--{name}' found\n\n  \
         flint render's per-effect flags went in #1144; write:\n"
    );
    for (written, now) in &found {
        text.push_str(&format!("    {written:<wide$}  ->  {now}\n"));
    }
    text.push_str(
        "\n  `flint render --list-effects` lists every post key, \
         `--list-styles --json` every style's keys\n",
    );
    clap::Error::raw(ErrorKind::UnknownArgument, text)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::render::RenderArgs;
    use clap::Parser;

    #[derive(Parser)]
    struct Cli {
        #[command(flatten)]
        args: RenderArgs,
    }

    fn parse(line: &str) -> Result<RenderArgs, clap::Error> {
        let words: Vec<&str> = ["render", "s.scene.toml"]
            .into_iter()
            .chain(line.split_whitespace())
            .collect();
        Cli::try_parse_from(&words)
            .map(|cli| cli.args)
            .map_err(|e| explained(e, &words))
    }

    /// A value of the right kind for every old flag.
    fn sample(name: &str) -> &'static str {
        match name {
            "fog-color" | "grade-lift" | "grade-gamma" | "grade-gain" | "contour-color"
            | "ascii-tint" => "0.2,0.5,0.9",
            "ascii-font" => "builtin",
            "ascii-ramp" => "blocks",
            "ascii-scope" => "entities",
            "ascii-grid" => "object",
            "ascii-color" => "mono",
            "ascii-ground" => "inverse",
            "ascii-palette" => "phosphor",
            "ssao-samples" | "volumetric-samples" | "boil-on" | "ascii-levels" => "4",
            // Inside each style key's range, as `--style` refuses one past it
            "kuwahara-radius" => "6",
            "kuwahara-sharpness" => "4",
            "kuwahara-hardness" => "12",
            "ascii-cell" => "12",
            "ascii-footprint" => "2",
            _ => "0.5",
        }
    }

    fn line(name: &str, now: Now) -> String {
        match now.is_switch() {
            true => format!("--{name}"),
            false => format!("--{name} {}", sample(name)),
        }
    }

    #[test]
    fn exposure_names_its_set() {
        let e = parse("--exposure 1").err().expect("--exposure is no flag");
        assert_eq!(e.kind(), clap::error::ErrorKind::UnknownArgument);
        let text = e.to_string();
        assert!(
            text.contains("--exposure 1  ->  --set post.exposure=1"),
            "{text}"
        );
        assert!(text.contains("#1144"), "{text}");
    }

    #[test]
    fn every_old_flag_is_unknown_and_its_spelling_parses() {
        for &(name, now) in RETIRED {
            let old = line(name, now);
            let e = parse(&old)
                .err()
                .unwrap_or_else(|| panic!("{old} still parses"));
            let found = spellings(&old.split_whitespace().collect::<Vec<_>>());
            assert_eq!(found.len(), 1, "{old}");
            let (written, new) = &found[0];
            assert_eq!(written, &old);
            assert!(e.to_string().contains(new.as_str()), "{old}: {e}");
            let args = parse(new).unwrap_or_else(|e| panic!("{old} -> {new}: {e}"));
            let styles = flint_render::stylize::StyleRegistry::new();
            super::super::render_post::check(&args, &styles)
                .and_then(|()| super::super::render_post::post_process_config(&args, None, &styles))
                .unwrap_or_else(|e| panic!("{old} -> {new}: {e}"));
        }
    }

    #[test]
    fn the_table_holds_each_old_flag_once() {
        assert_eq!(
            RETIRED.len(),
            67,
            "#708 hid 67 post flags beside the render mode's 3"
        );
        let mut names: Vec<&str> = RETIRED.iter().map(|(n, _)| *n).collect();
        names.sort_unstable();
        names.dedup();
        assert_eq!(names.len(), RETIRED.len());
    }

    #[test]
    fn every_old_flag_on_the_line_is_named_in_order() {
        let e = parse("--fog-density 0.05 --no-grid --ascii-scope screen --ascii-palette phosphor --fxaa --exposure=-1")
            .err()
            .unwrap();
        let text = e.to_string();
        let at = |s: &str| text.find(s).unwrap_or_else(|| panic!("{s} in {text}"));
        assert!(
            at("--set post.fog_density=0.05 --set post.fog_enabled=true")
                < at("--style ascii:scope=screen")
        );
        assert!(at("--style ascii:palette=phosphor,color=palette") < at("--set post.fxaa=true"));
        assert!(at("--set post.exposure=-1") > at("--set post.fxaa=true"));
        let last = text
            .lines()
            .find(|l| l.contains("--set post.exposure=-1"))
            .unwrap();
        assert!(
            last.trim_start().starts_with("--exposure=-1 "),
            "as written: {last}"
        );
        assert!(!text.contains("--no-grid"), "{text}");
    }

    #[test]
    fn a_switched_strength_says_which_way() {
        let words = |line: &str| {
            spellings(&line.split_whitespace().collect::<Vec<_>>())[0]
                .1
                .clone()
        };
        assert_eq!(
            words("--fog-density 0"),
            "--set post.fog_density=0 --set post.fog_enabled=false"
        );
        assert_eq!(words("--ssao-intensity 1"), "--set post.ssao_intensity=1");
        assert_eq!(
            words("--ssao-intensity 0"),
            "--set post.ssao_intensity=0 --set post.ssao_enabled=false"
        );
        assert_eq!(
            words("--contour-color 0.2,0.05,0.02"),
            "--style contour:color=0.2/0.05/0.02"
        );
        assert_eq!(
            spellings(&["--ascii-color", "mono", "--ascii-tint", "1,0,0"])[1].1,
            "--style ascii:tint=1/0/0",
            "--ascii-color picks the mode"
        );
        assert_eq!(words("--exposure"), "--set post.exposure=<v>");
    }

    #[test]
    fn another_unknown_flag_is_clap_s_own_error() {
        let e = parse("--exposur 1").err().unwrap();
        assert!(!e.to_string().contains("#1144"), "{e}");
        // A retired flag later on the line is not what clap stopped on
        let e = parse("--nope --exposure 1").err().unwrap();
        assert!(!e.to_string().contains("#1144"), "{e}");
    }
}
