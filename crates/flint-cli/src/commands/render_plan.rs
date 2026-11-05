//! What `flint render` was asked to capture (#216): which moments, under which
//! settings, into what kind of file. All of it is checked here, before a scene
//! loads or a GPU wakes, so a typo costs nothing.

use anyhow::{bail, Result};
use std::path::Path;

/// Each moment a wooze clock steps by when nothing says otherwise: the same 1/60 s
/// `--particle-time` uses, which is also a 60 Hz player's frame.
pub const DEFAULT_FRAME_STEP: f64 = 1.0 / 60.0;

/// The most frames a second a GIF can honestly hold. Its delays count hundredths
/// of a second, and browsers play anything under two of them at ten.
pub const GIF_MAX_FPS: f32 = 50.0;

/// `--time`: one moment or a span of them.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum TimeSpec {
    /// `--time 3.5`
    At(f64),
    /// `--time 0..4`: from the first, up to and not including the last, so that a
    /// span one cycle long loops without holding a frame twice.
    Range(f64, f64),
}

/// Parse `--time`: `3.5` or `0..4`.
pub fn parse_time(text: &str) -> Result<TimeSpec, String> {
    let seconds = |part: &str| -> Result<f64, String> {
        match part.trim().parse::<f64>() {
            Ok(value) if value.is_finite() && value >= 0.0 => Ok(value),
            _ => Err(format!(
                "'{part}' isn't a game time: it's seconds, 0 or more"
            )),
        }
    };
    match text.split_once("..") {
        None => seconds(text).map(TimeSpec::At),
        Some((from, to)) => {
            let (from, to) = (seconds(from)?, seconds(to)?);
            if to <= from {
                return Err(format!("the span {text} has to end after it starts"));
            }
            Ok(TimeSpec::Range(from, to))
        }
    }
}

/// The knobs `--sweep` can turn.
const SWEEP_KNOBS: [&str; 5] = ["amplitude", "step", "glide", "jitter", "preset"];

/// `--sweep wooze.glide=0,0.25,0.5,1`: one knob of the scene's global wooze
/// values and what to try it at, a column each.
#[derive(Debug, Clone, PartialEq)]
pub struct Sweep {
    /// The knob, as `WoozeField::set_global` calls it.
    pub knob: String,
    /// One per column, as written on the command line, which is also its label.
    pub values: Vec<String>,
}

impl Sweep {
    /// The `wooze.<knob>` it was written as.
    pub fn name(&self) -> String {
        format!("wooze.{}", self.knob)
    }

    /// What column `index` lays over the scene's `[wooze]` table.
    pub fn table(&self, index: usize) -> toml::Table {
        let text = &self.values[index];
        let value = match text.parse::<f64>() {
            Ok(number) => toml::Value::Float(number),
            Err(_) => toml::Value::String(text.clone()),
        };
        let mut table = toml::Table::new();
        table.insert(self.knob.clone(), value);
        table
    }
}

/// Parse `--sweep`. A knob's values are checked against its range by the field,
/// which says the same thing a bad scene table does; here is only what's needed
/// to know what was meant.
pub fn parse_sweep(text: &str) -> Result<Sweep, String> {
    let Some((name, values)) = text.split_once('=') else {
        return Err(format!(
            "'{text}' isn't a sweep: write it as wooze.glide=0,0.25,0.5,1"
        ));
    };
    let knob = name.trim().strip_prefix("wooze.").unwrap_or("");
    if !SWEEP_KNOBS.contains(&knob) {
        return Err(format!(
            "can't sweep '{}': it takes wooze.{}",
            name.trim(),
            SWEEP_KNOBS.join(", wooze.")
        ));
    }
    let values: Vec<String> = values.split(',').map(|v| v.trim().to_string()).collect();
    if let Some(blank) = values.iter().position(String::is_empty) {
        return Err(format!(
            "value {} of the sweep '{text}' is blank",
            blank + 1
        ));
    }
    if knob != "preset" {
        if let Some(wrong) = values.iter().find(|v| v.parse::<f32>().is_err()) {
            return Err(format!("wooze.{knob} takes numbers, and '{wrong}' isn't"));
        }
    }
    Ok(Sweep {
        knob: knob.to_string(),
        values,
    })
}

/// Whether `path` asks for an animation.
pub fn is_gif(path: &str) -> bool {
    Path::new(path)
        .extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| e.eq_ignore_ascii_case("gif"))
}

/// Parse `--fps`, for a span's frames or a strip's.
pub fn parse_gif_fps(text: &str) -> Result<f32, String> {
    match text.parse::<f32>() {
        Ok(fps) if fps > 0.0 && fps <= GIF_MAX_FPS => Ok(fps),
        _ => Err(format!(
            "'{text}' isn't a GIF's frame rate: it's more than 0 and at most {GIF_MAX_FPS}"
        )),
    }
}

/// The game time of every capture, in order.
pub fn capture_times(time: Option<TimeSpec>, fps: f32) -> Vec<f64> {
    match time {
        None => vec![0.0],
        Some(TimeSpec::At(t)) => vec![t],
        Some(TimeSpec::Range(from, to)) => {
            let fps = f64::from(fps);
            let frames = (((to - from) * fps).round() as u64).max(1);
            (0..frames).map(|i| from + i as f64 / fps).collect()
        }
    }
}

/// Refuse what can't be written: a span is motion, so it needs a file that moves,
/// and a GIF of one moment would be a slow way to write a still.
pub fn check_output(output: &str, time: Option<TimeSpec>) -> Result<()> {
    let span = matches!(time, Some(TimeSpec::Range(..)));
    if span && !is_gif(output) {
        bail!("a --time span makes an animation, so --output has to end in .gif, and {output} doesn't");
    }
    if !span && is_gif(output) {
        bail!("a GIF needs a span of game time to show: try --time 0..4, or a .png for one moment");
    }
    Ok(())
}

/// The size of one capture, unless `--width` and `--height` say: a still at the
/// usual size, and what's cheap enough to compare or to animate otherwise. A sweep's
/// columns are this size each, so the sheet is several of them across.
pub fn default_size(sweeping: bool, animated: bool) -> (u32, u32) {
    if sweeping {
        (640, 480)
    } else if animated {
        (960, 540)
    } else {
        (1920, 1080)
    }
}

/// How many steps of `frame_step` fit in `span`, and what's left over once they're
/// taken. Never overshoots: the leftover is the last step's, and landing on the
/// moment asked for matters more than the last step being a whole one.
pub fn steps_within(span: f64, frame_step: f64) -> (u64, f64) {
    if span <= 0.0 {
        return (0, 0.0);
    }
    let steps = (span / frame_step + 1e-9).floor();
    let rest = span - steps * frame_step;
    (steps as u64, if rest > 1e-9 { rest } else { 0.0 })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_time_is_a_moment_or_a_span() {
        assert_eq!(parse_time("3.5"), Ok(TimeSpec::At(3.5)));
        assert_eq!(parse_time("0..4"), Ok(TimeSpec::Range(0.0, 4.0)));
        assert!(parse_time("4..4").unwrap_err().contains("end after"));
        assert!(parse_time("-1").unwrap_err().contains("0 or more"));
        assert!(parse_time("soon").unwrap_err().contains("'soon'"));
        assert!(parse_time("NaN").is_err());
    }

    #[test]
    fn a_span_leaves_out_its_end_so_it_loops() {
        let times = capture_times(Some(TimeSpec::Range(1.0, 2.0)), 4.0);
        assert_eq!(times, [1.0, 1.25, 1.5, 1.75]);
        assert_eq!(capture_times(Some(TimeSpec::At(3.5)), 12.0), [3.5]);
        assert_eq!(capture_times(None, 12.0), [0.0]);
    }

    #[test]
    fn the_tickets_sweep_parses() {
        let sweep = parse_sweep("wooze.glide=0,0.25,0.5,1").unwrap();
        assert_eq!(sweep.name(), "wooze.glide");
        assert_eq!(sweep.values, ["0", "0.25", "0.5", "1"]);
        assert_eq!(sweep.table(1)["glide"].as_float(), Some(0.25));
    }

    #[test]
    fn a_preset_sweep_keeps_its_names() {
        let sweep = parse_sweep("wooze.preset=boil,morph").unwrap();
        assert_eq!(sweep.table(1)["preset"].as_str(), Some("morph"));
    }

    #[test]
    fn a_bad_sweep_says_what_was_wrong() {
        assert!(parse_sweep("wooze.glide").unwrap_err().contains("write it"));
        let unknown = parse_sweep("fog.density=1,2").unwrap_err();
        assert!(unknown.contains("fog.density") && unknown.contains("wooze.glide"));
        assert!(parse_sweep("glide=1,2").is_err());
        assert!(parse_sweep("wooze.glide=0,,1")
            .unwrap_err()
            .contains("value 2"));
        assert!(parse_sweep("wooze.glide=0,dreamy")
            .unwrap_err()
            .contains("'dreamy'"));
    }

    #[test]
    fn a_span_needs_a_gif_and_a_gif_needs_a_span() {
        let span = Some(TimeSpec::Range(0.0, 1.0));
        assert!(check_output("out.gif", span).is_ok());
        assert!(check_output("OUT.GIF", span).is_ok());
        assert!(check_output("out.png", span).is_err());
        assert!(check_output("out.gif", Some(TimeSpec::At(1.0))).is_err());
        assert!(check_output("out.gif", None).is_err());
        assert!(check_output("out.png", None).is_ok());
    }

    #[test]
    fn steps_land_on_the_moment_without_passing_it() {
        assert_eq!(steps_within(3.5, 0.0625), (56, 0.0));
        assert_eq!(steps_within(3.5, 1.0 / 60.0), (210, 0.0));
        let (steps, rest) = steps_within(0.1, 0.0625);
        assert_eq!(steps, 1);
        assert!((rest - 0.0375).abs() < 1e-12);
        assert_eq!(steps_within(0.0, 0.0625), (0, 0.0));
    }

    #[test]
    fn a_fps_a_gif_cant_hold_is_refused() {
        assert_eq!(parse_gif_fps("12"), Ok(12.0));
        assert!(parse_gif_fps("0").is_err());
        assert!(parse_gif_fps("60").unwrap_err().contains("at most 50"));
    }
}
