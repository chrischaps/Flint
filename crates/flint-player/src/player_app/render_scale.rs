//! The scene's render scale (#122): how many of the surface's pixels the 3D
//! scene is drawn at (`SceneRenderer::set_render_scale`). The HUD is always
//! drawn at the surface's own size.
//!
//! The scale is a ceiling and, optionally, a step under it:
//! - The ceiling is the fixed scale (`--render-scale`, `?scale=`), capped
//!   further where the device pixel ratio is above `dpr_cap`. A DPR 2
//!   laptop with the web's cap of 1.5 draws the scene at 0.75 of each side,
//!   56% of the pixels.
//! - With `dynamic`, [`DynamicScale`] steps below the ceiling when the frame
//!   time stays over budget for seconds, and tries a step back up after a
//!   calm stretch that grows each time a try fails, so a scene on the edge
//!   doesn't pump.

/// Where the scale comes from; see the module docs.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RenderScaleSetting {
    /// The most the scene is drawn at, 1 = the surface.
    pub scale: f32,
    /// The highest pixel ratio the scene is drawn at, if any.
    pub dpr_cap: Option<f64>,
    /// Step below the ceiling to hold the frame rate.
    pub dynamic: bool,
}

impl Default for RenderScaleSetting {
    /// The desktop's: the scene at the surface's size.
    fn default() -> Self {
        Self {
            scale: 1.0,
            dpr_cap: None,
            dynamic: false,
        }
    }
}

/// The web page's pixel ratio cap: past it, a high-DPI screen's extra
/// pixels go to the HUD's text, not the scene.
pub const WEB_DPR_CAP: f64 = 1.5;

impl RenderScaleSetting {
    /// A page's: the scene at no more than [`WEB_DPR_CAP`].
    pub fn web() -> Self {
        Self {
            dpr_cap: Some(WEB_DPR_CAP),
            ..Self::default()
        }
    }

    /// Read `--render-scale` / `?scale=`: a number from 0.25 to 1 draws
    /// at that scale, whatever the pixel ratio; `auto` keeps the cap and
    /// steps under it to hold the frame rate. Anything else is `None`.
    pub fn parse(self, value: &str) -> Option<Self> {
        let value = value.trim();
        if value.eq_ignore_ascii_case("auto") {
            return Some(Self {
                dynamic: true,
                ..self
            });
        }
        let scale = value
            .parse::<f32>()
            .ok()
            .filter(|s| s.is_finite() && *s > 0.0)?;
        Some(Self {
            scale: flint_render::clamp_render_scale(scale),
            dpr_cap: None,
            dynamic: false,
        })
    }

    /// The ceiling at pixel ratio `dpr`.
    pub fn ceiling(&self, dpr: f64) -> f32 {
        let cap = match self.dpr_cap {
            Some(cap) if dpr > cap => (cap / dpr) as f32,
            _ => 1.0,
        };
        flint_render::clamp_render_scale(self.scale.min(cap))
    }
}

/// The steps under the ceiling, as fractions of it.
const STEPS: [f32; 5] = [1.0, 0.875, 0.75, 0.625, 0.5];
/// A window of frames judged together, seconds.
const WINDOW: f64 = 2.0;
/// Windows over budget in a row before a step down.
const OVER_WINDOWS: u32 = 2;
/// The first calm stretch before a step up, seconds; it doubles each time
/// a step up has to be taken back, up to `MAX_CALM`.
const FIRST_CALM: f64 = 10.0;
const MAX_CALM: f64 = 160.0;
/// A frame over budget: its refresh period (or 60 Hz's) and a fifth.
const OVER: f64 = 1.2;
/// A frame longer than this is a stall (a tab switch, a load), not load.
const STALL: f64 = 0.25;

/// Steps under the ceiling to hold the frame rate; see the module docs.
#[derive(Debug, Clone)]
pub struct DynamicScale {
    step: usize,
    /// The window under way: its seconds and frame times.
    window_secs: f64,
    frames: Vec<f64>,
    over_windows: u32,
    calm_secs: f64,
    calm_needed: f64,
    /// Seconds since the last step up, while its window is still judged.
    since_raise: Option<f64>,
}

impl Default for DynamicScale {
    fn default() -> Self {
        Self {
            step: 0,
            window_secs: 0.0,
            frames: Vec::new(),
            over_windows: 0,
            calm_secs: 0.0,
            calm_needed: FIRST_CALM,
            since_raise: None,
        }
    }
}

impl DynamicScale {
    /// The fraction of the ceiling to draw at.
    pub fn factor(&self) -> f32 {
        STEPS[self.step]
    }

    /// Feed one frame's wall time, seconds, and the display's refresh
    /// period if known. Returns true when the factor changed.
    pub fn frame(&mut self, dt: f64, refresh: Option<f64>) -> bool {
        if !(dt.is_finite() && dt > 0.0 && dt <= STALL) {
            return false;
        }
        self.window_secs += dt;
        self.frames.push(dt);
        if let Some(t) = &mut self.since_raise {
            *t += dt;
        }
        // (Less a hair, so sixty 60 Hz frames make a second, summed or not.)
        if self.window_secs < WINDOW - 1e-6 {
            return false;
        }
        let budget = refresh.unwrap_or(1.0 / 60.0).max(1.0 / 60.0) * OVER;
        self.frames.sort_by(f64::total_cmp);
        let median = self.frames[self.frames.len() / 2];
        let secs = self.window_secs;
        self.window_secs = 0.0;
        self.frames.clear();

        if median > budget {
            self.calm_secs = 0.0;
            self.over_windows += 1;
            // A step up that didn't hold: back down at once, and wait
            // longer before the next try.
            let failed_raise = self.since_raise.is_some_and(|t| t <= WINDOW * 2.0 + 1e-9);
            if failed_raise {
                self.calm_needed = (self.calm_needed * 2.0).min(MAX_CALM);
            }
            if (failed_raise || self.over_windows >= OVER_WINDOWS) && self.step + 1 < STEPS.len() {
                self.step += 1;
                self.over_windows = 0;
                self.since_raise = None;
                return true;
            }
            return false;
        }
        self.over_windows = 0;
        if self.since_raise.is_some_and(|t| t > WINDOW * 2.0) {
            self.since_raise = None;
        }
        self.calm_secs += secs;
        if self.step > 0 && self.calm_secs >= self.calm_needed - 1e-6 {
            self.step -= 1;
            self.calm_secs = 0.0;
            self.since_raise = Some(0.0);
            return true;
        }
        false
    }
}

impl super::PlayerApp {
    /// Set the scene's render scale for this frame: the ceiling at the
    /// window's pixel ratio, stepped down by the dynamic scale if it's on.
    /// The renderer only rebuilds its targets when the size changes.
    pub(super) fn update_render_scale(&mut self) {
        let setting = self.render_scale;
        if setting.dynamic && self.clock.frame_step.is_none() {
            // A paced page's budget is its period (#140), not the refresh.
            let refresh = self.paced_period().or(self.clock.vsync_period());
            let dt = self.clock.raw_delta_time;
            if self.dynamic_scale.frame(dt, refresh) {
                tracing::info!(
                    "[render-scale] {} of the ceiling",
                    self.dynamic_scale.factor()
                );
            }
        }
        let dpr = self.window.as_ref().map_or(1.0, |w| w.scale_factor());
        let scale = setting.ceiling(dpr) * self.dynamic_scale.factor();
        if let (Some(context), Some(renderer)) = (&self.render_context, &mut self.scene_renderer) {
            renderer.set_render_scale(&context.device, scale);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const HZ60: f64 = 1.0 / 60.0;

    #[test]
    fn the_desktop_draws_at_the_surface() {
        let s = RenderScaleSetting::default();
        assert_eq!(s.ceiling(1.0), 1.0);
        assert_eq!(s.ceiling(2.0), 1.0);
    }

    #[test]
    fn the_web_caps_the_pixel_ratio() {
        let s = RenderScaleSetting::web();
        assert_eq!(s.ceiling(1.0), 1.0);
        assert_eq!(s.ceiling(1.5), 1.0);
        assert_eq!(s.ceiling(2.0), 0.75);
        assert_eq!(s.ceiling(3.0), 0.5);
    }

    #[test]
    fn a_number_overrides_the_cap() {
        let s = RenderScaleSetting::web().parse("1").unwrap();
        assert_eq!(s.ceiling(2.0), 1.0);
        let s = RenderScaleSetting::web().parse("0.5").unwrap();
        assert_eq!(s.ceiling(1.0), 0.5);
        assert_eq!(s.ceiling(2.0), 0.5);
        let s = RenderScaleSetting::web().parse("auto").unwrap();
        assert!(s.dynamic);
        assert_eq!(s.ceiling(2.0), 0.75);
        assert_eq!(RenderScaleSetting::web().parse("big"), None);
        assert_eq!(RenderScaleSetting::web().parse("0"), None);
    }

    fn run(d: &mut DynamicScale, secs: f64, dt: f64) -> usize {
        let mut changes = 0;
        for _ in 0..(secs / dt).round() as usize {
            changes += d.frame(dt, Some(HZ60)) as usize;
        }
        changes
    }

    #[test]
    fn a_frame_rate_held_stays_put() {
        let mut d = DynamicScale::default();
        assert_eq!(run(&mut d, 60.0, HZ60), 0);
        assert_eq!(d.factor(), 1.0);
    }

    #[test]
    fn steps_down_only_after_seconds_over_budget() {
        let mut d = DynamicScale::default();
        // One bad second is not enough
        run(&mut d, 1.0, 2.0 * HZ60);
        run(&mut d, 3.0, HZ60);
        assert_eq!(d.factor(), 1.0);
        // Four seconds at 30 fps steps down once
        assert_eq!(run(&mut d, 4.0, 2.0 * HZ60), 1);
        assert_eq!(d.factor(), 0.875);
    }

    #[test]
    fn stalls_are_not_load() {
        let mut d = DynamicScale::default();
        for _ in 0..40 {
            d.frame(0.5, Some(HZ60));
        }
        assert_eq!(d.factor(), 1.0);
    }

    #[test]
    fn a_failed_step_up_waits_longer_before_the_next() {
        let mut d = DynamicScale::default();
        run(&mut d, 4.0, 2.0 * HZ60);
        assert_eq!(d.factor(), 0.875);
        // Calm at the lower scale: a try after FIRST_CALM
        assert_eq!(run(&mut d, FIRST_CALM, HZ60), 1);
        assert_eq!(d.factor(), 1.0);
        // It doesn't hold: straight back down after one window
        assert_eq!(run(&mut d, WINDOW, 2.0 * HZ60), 1);
        assert_eq!(d.factor(), 0.875);
        // The next try waits twice as long
        assert_eq!(run(&mut d, FIRST_CALM * 1.5, HZ60), 0);
        assert_eq!(run(&mut d, FIRST_CALM * 0.5, HZ60), 1);
        assert_eq!(d.factor(), 1.0);
    }

    #[test]
    fn a_slow_display_is_its_own_budget() {
        // A 30 Hz display presents every 33 ms; that's not over budget.
        let mut d = DynamicScale::default();
        for _ in 0..600 {
            d.frame(2.0 * HZ60, Some(2.0 * HZ60));
        }
        assert_eq!(d.factor(), 1.0);
    }

    #[test]
    fn never_below_the_last_step() {
        let mut d = DynamicScale::default();
        run(&mut d, 120.0, 4.0 * HZ60);
        assert_eq!(d.factor(), 0.5);
    }
}
