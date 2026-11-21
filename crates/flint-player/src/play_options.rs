//! What a launch's options mean once read (#1142): one plain-data
//! [`PlayOptions`] that every host builds from its own source, and one
//! [`PlayOptions::apply`] that sets a `PlayerApp` from it.
//!
//! The desktop reads its flags (`PlayerArgs::options`, clap), the web page
//! its query string ([`PlayOptions::from_query`], `bin/flint_web.rs`) and
//! Android its dev profile (`flint-android`). Each source spells a value its
//! own way but reads it with the same reader here, so `--render-scale 0.5`,
//! `?scale=0.5` and a profile's `render_scale = 0.5` are one setting. A value
//! that doesn't read is a problem the reader names: the desktop refuses it
//! before a window opens, and the web and Android say it (the console,
//! logcat) and play without it, since neither has a command line to send
//! back to.
//!
//! A new option is a field here, a line in [`apply`](PlayOptions::apply),
//! and a line in each source that offers it.

use crate::{FrameRateSetting, PlayerApp, ProcGenMode, RenderScaleSetting};
use flint_runtime::TouchControlsMode;
use std::path::PathBuf;

/// What a launch's options mean once parsed: no clap, no query string, no
/// TOML. Every field's default is "as if the option weren't given".
#[derive(Debug, Clone, PartialEq)]
pub struct PlayOptions {
    /// Scene-pass MSAA samples, 1 or 4 (`--msaa`, `?msaa=`, `msaa`).
    pub msaa: u32,
    /// The scene's render scale (`--render-scale`, `?scale=`,
    /// `render_scale`). `None` keeps the player's default (the surface,
    /// natively); the web always sets one, since its default caps DPR 1.5.
    pub render_scale: Option<RenderScaleSetting>,
    /// `key=value,...` quality settings over msaa and the render scale
    /// (`--quality`, `?quality=`, `[quality]`).
    pub quality: String,
    /// Style words over every scene's post-processing (`--style`, `?style=`).
    pub styles: Vec<String>,
    /// A look every scene plays under (`--look`).
    pub look: Option<String>,
    /// The post time styles see, held (`--style-time`).
    pub style_time: Option<f32>,
    /// The whole frame on twos at this rate (`--motion-hold`).
    pub motion_hold: Option<f32>,
    /// The on-screen touch controls (`--touch-controls`, `?touch=`).
    pub touch_controls: TouchControlsMode,
    /// Stand-in safe-area insets in points (`--safe-area`).
    pub safe_area: Option<[f32; 4]>,
    /// Report a touch-first device to scripts (`--touch-device`).
    pub touch_device: bool,
    /// Pixels a point, over the window's scale factor (`--pixels-per-point`).
    pub pixels_per_point: Option<f32>,
    /// Fail the run on any script error (`--strict-scripts`).
    pub strict_scripts: bool,
    /// Make procgen assets behind placeholders (`--procgen-defer`).
    pub procgen_defer: bool,
    /// Keep saves in this folder (`--save-dir`).
    pub save_dir: Option<PathBuf>,
    /// Parts of the frame skipped or drawn N times (`--skip-pass`,
    /// `--repeat-pass`, `?skip_pass=`, `?repeat_pass=`).
    pub pass_knobs: Option<flint_render::PassKnobs>,
    /// Game seconds a frame, whatever the wall time (`--frame-step`,
    /// `?frame_step=`).
    pub frame_step: Option<f64>,
    /// Scripts' random seed (`--seed`, `?seed=`); a frame step seeds them too.
    pub seed: Option<u64>,
    /// Stop game time here (`?hold_at=`).
    pub hold_at: Option<f64>,
    /// Which refreshes are drawn (`?fps=`). `None` keeps the player's.
    pub frame_rate: Option<FrameRateSetting>,
    /// The perf readout: `Some(gpu)` shows it, timing the GPU when true
    /// (`--perf`, `?perf=1|cpu`, `perf = true`).
    pub perf: Option<bool>,
    /// The old mix, every sound on kira spatial tracks (`?mix=old`, #138).
    pub old_mix: bool,
}

impl Default for PlayOptions {
    fn default() -> Self {
        Self {
            msaa: 1,
            render_scale: None,
            quality: String::new(),
            styles: Vec::new(),
            look: None,
            style_time: None,
            motion_hold: None,
            touch_controls: TouchControlsMode::default(),
            safe_area: None,
            touch_device: false,
            pixels_per_point: None,
            strict_scripts: false,
            procgen_defer: false,
            save_dir: None,
            pass_knobs: None,
            frame_step: None,
            seed: None,
            hold_at: None,
            frame_rate: None,
            perf: None,
            old_mix: false,
        }
    }
}

impl PlayOptions {
    /// Set `app` from these options: every launch field they hold, the
    /// touch controls' mode, the frame step and seed, the frame rate, the
    /// perf readout and the mix. Call it once, before `run_app`, as every
    /// host does; it replaces what a field held.
    pub fn apply(self, app: &mut PlayerApp) {
        let launch = &mut app.launch;
        launch.msaa_sample_count = self.msaa;
        if let Some(scale) = self.render_scale {
            launch.render_scale = scale;
        }
        launch.quality_overrides = self.quality;
        launch.style_specs = self.styles;
        launch.look = self.look;
        launch.style_time = self.style_time;
        launch.motion_hold = self.motion_hold;
        launch.safe_area = self.safe_area;
        launch.touch_device = self.touch_device;
        launch.pixels_per_point = self.pixels_per_point.filter(|p| *p > 0.0);
        launch.strict_scripts = self.strict_scripts;
        launch.save_dir = self.save_dir;
        launch.pass_knobs = self.pass_knobs;
        launch.hold_at = self.hold_at;
        if self.procgen_defer {
            launch.procgen_mode = ProcGenMode::Deferred;
        }
        app.input.touch_controls_mut().set_mode(self.touch_controls);
        // The --frame-step and --seed rule (#705): a stepped run is seeded
        app.step_frames(self.frame_step, self.seed);
        if let Some(rate) = self.frame_rate {
            app.set_frame_rate(rate);
        }
        if let Some(gpu) = self.perf {
            app.enable_perf(gpu);
        }
        app.audio.set_old_mix(self.old_mix);
    }

    /// Read a render scale word over `base` (the host's default: the
    /// surface natively, the DPR cap on a page): a number from 0.25 to 1,
    /// or `auto`. The error says why.
    pub fn read_render_scale(
        &mut self,
        base: RenderScaleSetting,
        value: &str,
    ) -> Result<(), String> {
        let scale = base
            .parse(value)
            .ok_or_else(|| format!("{value}: a number or auto"))?;
        self.render_scale = Some(scale);
        Ok(())
    }

    /// Keep quality settings, `key=value,...`. A setting that doesn't read
    /// is the error (the first of them); the string is kept either way, so
    /// a host that plays on anyway applies the ones that do.
    pub fn read_quality(&mut self, value: &str) -> Result<(), String> {
        self.quality = value.to_string();
        let errors = flint_render::QualitySettings::default().apply_str(value);
        match errors.first() {
            Some(e) => Err(e.to_string()),
            None => Ok(()),
        }
    }

    /// Add a style word, its syntax checked now. An unknown name gets
    /// through: it may be a user style the scene's `styles/` registers
    /// (#274), checked once those are loaded. A malformed word is left out.
    pub fn read_style(&mut self, word: &str) -> Result<(), String> {
        check_style_word(word)?;
        self.styles.push(word.to_string());
        Ok(())
    }

    /// Read the touch controls' mode: `auto`, `always` or `never`.
    pub fn read_touch_controls(&mut self, value: &str) -> Result<(), String> {
        self.touch_controls =
            TouchControlsMode::parse(value).ok_or_else(|| "auto, always or never".to_string())?;
        Ok(())
    }

    /// The options a web page's query string asks for (#94's keys, listed
    /// in `bin/flint_web.rs`), and a line for each value that didn't read
    /// (`scale=huge: a number or auto`), which the page warns of and plays
    /// on without. Keys the page reads itself (`bundle`, `scene`, `width`,
    /// `loop`, ...) are not options and are passed over here.
    pub fn from_query(query: &Query) -> (Self, Vec<String>) {
        let mut options = Self {
            render_scale: Some(RenderScaleSetting::web()),
            ..Self::default()
        };
        let mut problems = Vec::new();
        let mut said = |key: &str, value: &str, why: String| {
            problems.push(format!("{key}={value}: {why}"));
        };
        let secs = |v: &str| v.parse::<f64>().ok().filter(|s| s.is_finite() && *s >= 0.0);

        if let Some(v) = query.get("msaa") {
            match v.parse::<u32>() {
                Ok(m @ (1 | 4)) => options.msaa = m,
                _ => said("msaa", v, "1 or 4".into()),
            }
        }
        if let Some(v) = query.get("scale") {
            if options
                .read_render_scale(RenderScaleSetting::web(), v)
                .is_err()
            {
                said("scale", v, "a number from 0.25 to 1, or auto".into());
            }
        }
        if let Some(v) = query.get("quality") {
            if let Err(e) = options.read_quality(v) {
                said("quality", v, e);
            }
        }
        for v in query.get_all("style").filter(|s| !s.is_empty()) {
            if let Err(e) = options.read_style(v) {
                said("style", v, e);
            }
        }
        if let Some(v) = query.get("touch") {
            if let Err(e) = options.read_touch_controls(v) {
                said("touch", v, e);
            }
        }
        if let Some(v) = query.get("frame_step") {
            match secs(v).filter(|s| *s > 0.0) {
                Some(s) => options.frame_step = Some(s),
                None => said("frame_step", v, "seconds, more than 0".into()),
            }
        }
        if let Some(v) = query.get("seed") {
            match v.parse::<u64>() {
                Ok(n) => options.seed = Some(n),
                Err(_) => said("seed", v, "a whole number, 0 or more".into()),
            }
        }
        if let Some(v) = query.get("hold_at") {
            match secs(v) {
                Some(s) => options.hold_at = Some(s),
                None => said("hold_at", v, "seconds, 0 or more".into()),
            }
        }
        if let Some(v) = query.get("fps") {
            match FrameRateSetting::parse(v) {
                Some(rate) => options.frame_rate = Some(rate),
                None => said("fps", v, "a rate from 10 to 240, auto or native".into()),
            }
        }
        options.perf = match query.get("perf") {
            None | Some("0") => None,
            Some("cpu") => Some(false),
            Some(_) => Some(true),
        };
        options.old_mix = query.get("mix") == Some("old");

        let mut knobs = flint_render::PassKnobs::new();
        for list in query.get_all("skip_pass") {
            knobs.skip_list(list);
        }
        for list in query.get_all("repeat_pass") {
            if let Err(e) = knobs.repeat_list(list) {
                said("repeat_pass", list, e);
            }
        }
        options.pass_knobs = (!knobs.is_empty()).then_some(knobs);
        (options, problems)
    }
}

/// A style word's syntax: `name[:key=value,...]`. An unknown name passes
/// (a user style registers later); anything else that doesn't parse is the
/// error. Also clap's check of `--style`.
pub fn check_style_word(word: &str) -> Result<(), String> {
    use flint_render::stylize::{StyleRegistry, StyleSpec};
    match StyleSpec::parse(word, &StyleRegistry::new()) {
        Ok(_) => Ok(()),
        Err(e) if e.starts_with("unknown style") => Ok(()),
        Err(e) => Err(e),
    }
}

/// A URL query string, decoded as a page's `URLSearchParams` decodes it:
/// `&`-separated `key=value` pairs, `+` a space and `%XX` a byte, in the
/// order written. The web player reads its page's with it, and a test can
/// read one with no browser.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Query {
    pairs: Vec<(String, String)>,
}

impl Query {
    /// Read `query`, with or without its leading `?`.
    pub fn parse(query: &str) -> Self {
        let query = query.strip_prefix('?').unwrap_or(query);
        let pairs = query
            .split('&')
            .filter(|pair| !pair.is_empty())
            .map(|pair| {
                let (key, value) = pair.split_once('=').unwrap_or((pair, ""));
                (decode(key), decode(value))
            })
            .collect();
        Self { pairs }
    }

    /// The first value of `key`, as `URLSearchParams.get` returns it.
    pub fn get(&self, key: &str) -> Option<&str> {
        self.pairs
            .iter()
            .find(|(k, _)| k == key)
            .map(|(_, v)| v.as_str())
    }

    /// Every value of `key`, in the order written.
    pub fn get_all<'a>(&'a self, key: &'a str) -> impl Iterator<Item = &'a str> + 'a {
        self.pairs
            .iter()
            .filter(move |(k, _)| k == key)
            .map(|(_, v)| v.as_str())
    }
}

/// `+` to a space, `%XX` to its byte, the bytes read as UTF-8 (a broken
/// sequence becomes U+FFFD, as the browser's decoder does).
fn decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        let escaped = (bytes[i] == b'%')
            .then(|| Some((hex(*bytes.get(i + 1)?)?, hex(*bytes.get(i + 2)?)?)))
            .flatten();
        match (bytes[i], escaped) {
            (_, Some((high, low))) => {
                out.push(high << 4 | low);
                i += 2;
            }
            (b'+', _) => out.push(b' '),
            (b, _) => out.push(b),
        }
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn hex(b: u8) -> Option<u8> {
    (b as char).to_digit(16).map(|d| d as u8)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::PlayerCli;
    use clap::Parser;

    fn desktop(flags: &[&str]) -> anyhow::Result<PlayOptions> {
        let mut argv = vec!["flint-player", "scene.toml"];
        argv.extend_from_slice(flags);
        PlayerCli::try_parse_from(argv)?.args.options()
    }

    fn web(query: &str) -> (PlayOptions, Vec<String>) {
        PlayOptions::from_query(&Query::parse(query))
    }

    /// What a launch sets on a headless `PlayerApp`: the whole
    /// `LaunchOptions`, and what `apply` sets beside it.
    fn launched(options: PlayOptions) -> String {
        let mut app = PlayerApp::new(
            flint_ecs::FlintWorld::new(),
            "play_options_test.scene.toml".into(),
            false,
            None,
            None,
        );
        options.apply(&mut app);
        format!(
            "{:?}\ntouch {:?}\nframe step {:?}\nscript timing {}\nold mix {}",
            app.launch,
            app.input.touch_controls().mode(),
            app.core.clock.frame_step,
            app.core.script.timing(),
            app.audio.engine.old_mix(),
        )
    }

    #[test]
    fn a_web_query_and_the_same_desktop_flags_launch_the_same() {
        // Percent-encoded as a browser's address bar leaves it
        let (from_page, problems) = web("?msaa=4&scale=0.5&quality=ssao%3D0%2Cshadow_cascades%3D1\
             &style=ascii%3Acell%3D12&style=invert&touch=never\
             &frame_step=0.0166667&seed=7&perf=1&skip_pass=Bloom&repeat_pass=Tonemap%3A3");
        assert!(problems.is_empty(), "{problems:?}");
        let from_flags = desktop(&[
            "--msaa",
            "4",
            "--render-scale",
            "0.5",
            "--quality",
            "ssao=0,shadow_cascades=1",
            "--style",
            "ascii:cell=12",
            "--style",
            "invert",
            "--touch-controls",
            "never",
            "--frame-step",
            "0.0166667",
            "--seed",
            "7",
            "--perf",
            "--skip-pass",
            "Bloom",
            "--repeat-pass",
            "Tonemap:3",
        ])
        .unwrap();
        assert_eq!(from_page, from_flags);

        let page = launched(from_page);
        assert_eq!(page, launched(from_flags));
        // And the launch did take them: not two untouched defaults
        assert_ne!(page, launched(PlayOptions::default()));
        for set in [
            "msaa_sample_count: 4",
            "scale: 0.5",
            "quality_overrides: \"ssao=0,shadow_cascades=1\"",
            "\"ascii:cell=12\", \"invert\"",
            "load_report: true",
            "touch Never",
            "frame step Some(0.0166667)",
            "script timing true",
        ] {
            assert!(page.contains(set), "{set} in\n{page}");
        }
    }

    #[test]
    fn a_value_that_doesnt_read_is_a_warning_on_the_page_and_refused_on_the_desktop() {
        let (from_page, problems) = web("scale=huge&quality=bloom=maybe&touch=sometimes&msaa=8");
        assert_eq!(
            problems,
            [
                "msaa=8: 1 or 4",
                "scale=huge: a number from 0.25 to 1, or auto",
                &format!(
                    "quality=bloom=maybe: {}",
                    flint_render::QualitySettings::default().apply_str("bloom=maybe")[0]
                ),
                "touch=sometimes: auto, always or never",
            ]
        );
        // The page plays on with its defaults
        assert_eq!(from_page.msaa, 1);
        assert_eq!(from_page.render_scale, Some(RenderScaleSetting::web()));
        assert_eq!(from_page.touch_controls, TouchControlsMode::Auto);

        for flags in [
            &["--render-scale", "huge"][..],
            &["--quality", "bloom=maybe"],
            &["--touch-controls", "sometimes"],
        ] {
            assert!(desktop(flags).is_err(), "{flags:?} refused");
        }
    }

    #[test]
    fn a_bare_page_keeps_its_own_defaults() {
        let (from_page, problems) = web("");
        assert!(problems.is_empty());
        assert_eq!(
            from_page,
            PlayOptions {
                render_scale: Some(RenderScaleSetting::web()),
                ..PlayOptions::default()
            }
        );
        // `auto` keeps each host's ceiling: the page's DPR cap, the
        // desktop's surface
        let (auto, _) = web("scale=auto");
        assert_eq!(auto.render_scale.unwrap().dpr_cap, Some(crate::WEB_DPR_CAP));
        let auto = desktop(&["--render-scale", "auto"]).unwrap();
        assert_eq!(auto.render_scale.unwrap().dpr_cap, None);
    }

    #[test]
    fn a_query_decodes_as_a_browser_does() {
        let q = Query::parse("?a=1&b=x+y&c=%E2%9C%93&a=2&flag&d=%zz&e=50%");
        assert_eq!(q.get("a"), Some("1"));
        assert_eq!(q.get_all("a").collect::<Vec<_>>(), ["1", "2"]);
        assert_eq!(q.get("b"), Some("x y"));
        assert_eq!(q.get("c"), Some("\u{2713}"));
        assert_eq!(q.get("flag"), Some(""));
        assert_eq!(q.get("d"), Some("%zz"));
        assert_eq!(q.get("e"), Some("50%"));
        assert_eq!(q.get("missing"), None);
    }
}
