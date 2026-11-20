//! The render modes, as a deprecated shim (#746): `--render-mode N
//! --mode-mix M --mode-params x,y,z,w` and Rhai's `set_render_mode` /
//! `set_render_mode_params` for one release, mapped onto what replaced
//! them. Remove with them.
//!
//! | mode | what it is now |
//! |---|---|
//! | 1 Matrix | the `matrix` style, `blend_source = "tear"`, `rate` = z |
//! | 2 blood | the `blood` style following the tear, and the ocean's tint ([`OceanTint::blood`]) |
//! | 3 drunk | the `drunk` style at `blend` = mix |
//! | 4 Tron | the `tron` style following the tear |
//! | 5 underwater | the `underwater` style (`depth`, `churn`, `daylight`, `glow` = x, y, z, w; `blend` = mix) and the ocean's [`OceanTint::underside`] |
//! | 6 tear | the tear alone |
//!
//! Modes 1-4 and 6 set `[post_process] tear` to the mix, `tear_scale` to x
//! and `tear_iris` to y >= 0.5, as their mask read them.
//!
//! A mode was transient: shown only the frames a script asks for it. So is
//! this: [`RenderModeShim::stamp`] saves what it overwrites the first frame
//! a mode is asked for and puts it back the first frame none is.

use crate::ocean_pipeline::OceanTint;
use crate::postprocess::PostProcessConfig;
use crate::stylize::StyleSettings;

/// The styles a mode switches on, which the shim saves and puts back.
pub const STYLES: [&str; 5] = ["matrix", "blood", "drunk", "tron", "underwater"];

/// A mode the shim draws: 1-6 (0 and anything above 6 draw nothing).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RenderMode {
    pub mode: u32,
    pub mix: f32,
    pub params: [f32; 4],
}

impl RenderMode {
    /// Whether it draws anything.
    pub fn draws(&self) -> bool {
        (1..=6).contains(&self.mode) && self.mix > 0.0
    }

    /// Write it into `config`: the tear and the mode's style. The config
    /// must hold none of the shim's earlier writes (`RenderModeShim` puts
    /// them back first).
    pub fn apply(&self, config: &mut PostProcessConfig) {
        let [x, y, z, w] = self.params;
        let mix = self.mix.clamp(0.0, 1.0);
        if matches!(self.mode, 1..=4 | 6) {
            config.tear = mix;
            config.tear_scale = x;
            config.tear_iris = y >= 0.5;
        }
        let styles = &mut config.styles;
        let tear_look = |styles: &mut StyleSettings, name: &str| {
            styles.set_enabled(name, true);
            styles.set_follows_tear(name, true);
            styles.set_blend(name, None);
        };
        match self.mode {
            1 => {
                tear_look(styles, "matrix");
                styles.set_param("matrix", "rate", if z < 0.01 { 6.0 } else { z });
            }
            2 => tear_look(styles, "blood"),
            3 => {
                styles.set_enabled("drunk", true);
                styles.set_blend("drunk", Some(mix));
            }
            4 => tear_look(styles, "tron"),
            5 => {
                styles.set_enabled("underwater", true);
                styles.set_blend("underwater", Some(mix));
                styles.set_param("underwater", "depth", x);
                styles.set_param("underwater", "churn", y.clamp(0.0, 1.0));
                styles.set_param("underwater", "daylight", z.clamp(0.0, 1.0));
                styles.set_param("underwater", "glow", w.clamp(0.0, 1.0));
            }
            _ => {}
        }
    }

    /// The sea's tint the mode drew (blood's pools, the underside from a
    /// buried eye), for `FrameInputs::ocean_tint`.
    pub fn ocean_tint(&self) -> Option<OceanTint> {
        let mix = self.mix.clamp(0.0, 1.0);
        match self.mode {
            2 => Some(OceanTint::blood(mix)),
            5 => Some(OceanTint::underside(mix)),
            _ => None,
        }
    }
}

/// What the shim overwrote, to put back.
#[derive(Debug, Clone, PartialEq)]
struct Saved {
    tear: (f32, f32, bool),
    styles: StyleSettings,
}

/// The shim's memory between frames: what a mode overwrote in a config
/// that lives on (the player's and the headless game's are stamped, not
/// rebuilt, each frame).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct RenderModeShim {
    saved: Option<Saved>,
    /// The mode drawn last, for its ocean tint.
    shown: Option<RenderMode>,
}

impl RenderModeShim {
    /// Stamp this frame's mode (`None` when no script asked for one) over
    /// `config`: the first frame of a mode saves what it overwrites, each
    /// frame starts from that, and the first frame with none puts it back.
    /// Returns whether it changed `config`.
    pub fn stamp(&mut self, config: &mut PostProcessConfig, mode: Option<RenderMode>) -> bool {
        let mode = mode.filter(RenderMode::draws);
        if let Some(saved) = &self.saved {
            restore(config, saved);
        }
        self.shown = mode;
        match mode {
            Some(mode) => {
                if self.saved.is_none() {
                    self.saved = Some(save(config));
                }
                mode.apply(config);
                true
            }
            None => self.saved.take().is_some(),
        }
    }

    /// Whether a mode is on (its writes are in the config).
    pub fn active(&self) -> bool {
        self.saved.is_some()
    }

    /// The ocean tint of the mode stamped last, if it has one.
    pub fn ocean_tint(&self) -> Option<OceanTint> {
        self.shown.and_then(|m| m.ocean_tint())
    }

    /// Forget the mode without putting anything back: a scene load, whose
    /// config is the new scene's.
    pub fn forget(&mut self) {
        *self = Self::default();
    }
}

fn save(config: &PostProcessConfig) -> Saved {
    let mut styles = StyleSettings::default();
    for name in STYLES {
        styles.restore(name, &config.styles);
    }
    Saved {
        tear: (config.tear, config.tear_scale, config.tear_iris),
        styles,
    }
}

fn restore(config: &mut PostProcessConfig, saved: &Saved) {
    (config.tear, config.tear_scale, config.tear_iris) = saved.tear;
    for name in STYLES {
        config.styles.restore(name, &saved.styles);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mode(mode: u32, mix: f32, params: [f32; 4]) -> Option<RenderMode> {
        Some(RenderMode { mode, mix, params })
    }

    #[test]
    fn each_mode_is_its_style_and_the_tear() {
        let mut config = PostProcessConfig::default();
        mode(1, 0.5, [3.0, 0.0, 0.0, 0.0])
            .unwrap()
            .apply(&mut config);
        assert_eq!(
            (config.tear, config.tear_scale, config.tear_iris),
            (0.5, 3.0, false)
        );
        assert!(config.styles.is_enabled("matrix") && config.styles.follows_tear("matrix"));
        assert_eq!(
            config.styles.param("matrix", "rate"),
            Some(6.0),
            "0 reads as 6"
        );

        let mut config = PostProcessConfig::default();
        mode(5, 0.7, [0.4, 0.3, 0.9, 0.2])
            .unwrap()
            .apply(&mut config);
        assert_eq!(config.tear, 0.0, "underwater is no tear");
        assert_eq!(config.styles.blend("underwater"), 0.7);
        assert_eq!(config.styles.param("underwater", "depth"), Some(0.4));
        assert_eq!(config.styles.param("underwater", "glow"), Some(0.2));
        assert!(!config.styles.follows_tear("underwater"));

        let mut config = PostProcessConfig::default();
        mode(6, 0.5, [0.0, 1.0, 0.0, 0.0])
            .unwrap()
            .apply(&mut config);
        assert!(config.tear_iris);
        assert!(
            STYLES.iter().all(|s| !config.styles.is_enabled(s)),
            "the tear alone"
        );
        assert_eq!(
            mode(2, 0.4, [0.0; 4]).unwrap().ocean_tint(),
            Some(OceanTint::blood(0.4))
        );
        assert_eq!(
            mode(5, 1.0, [0.0; 4]).unwrap().ocean_tint(),
            Some(OceanTint::underside(1.0))
        );
        assert_eq!(mode(1, 1.0, [0.0; 4]).unwrap().ocean_tint(), None);
    }

    /// Transient, as the mode was: the scene's own settings come back the
    /// frame no script asks, a switch from one mode to another leaves
    /// nothing of the first, and a frame with none changes nothing.
    #[test]
    fn the_shim_puts_back_what_it_overwrote() {
        let mut config = PostProcessConfig {
            tear: 0.2,
            ..Default::default()
        };
        config.styles.set_enabled("tron", true);
        config.styles.set_param("tron", "spacing", 4.0);
        let authored = config.clone();
        let same = |a: &PostProcessConfig, b: &PostProcessConfig| {
            (a.tear, a.tear_scale, a.tear_iris) == (b.tear, b.tear_scale, b.tear_iris)
                && a.styles == b.styles
        };
        let mut shim = RenderModeShim::default();

        assert!(!shim.stamp(&mut config, None), "nothing to do");
        assert!(shim.stamp(&mut config, mode(4, 0.8, [0.0; 4])));
        assert_eq!(config.tear, 0.8);
        assert!(config.styles.follows_tear("tron"));
        assert!(shim.stamp(&mut config, mode(1, 0.5, [0.0; 4])));
        assert!(
            !config.styles.follows_tear("tron"),
            "the first mode's writes are gone"
        );
        assert!(config.styles.is_enabled("matrix"));
        assert!(
            shim.stamp(&mut config, mode(1, 0.0, [0.0; 4])),
            "a mix of 0 heals"
        );
        assert!(same(&config, &authored));
        assert!(!shim.active());
        assert!(!shim.stamp(&mut config, None));
        assert!(same(&config, &authored));
    }
}
