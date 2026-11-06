//! Colour palettes (#276): a short list of ink colours and a background,
//! read from a `*.palette.toml`, for any style that wants a fixed set of
//! colours (ASCII's `palette` colour mode first).
//!
//! ```toml
//! # palettes/phosphor.palette.toml
//! match = "luminance"          # nearest (default) | luminance
//! background = "#020a04"       # the ground behind the inks (default black)
//! colors = ["#1f6e33", "#33b04d", "#4dff6e", "#b4ffc4"]
//! ```
//!
//! `nearest` gives each cell the ink closest to its own colour (a CGA
//! screen); `luminance` lists the inks darkest first and picks one by the
//! cell's brightness alone (a one-colour phosphor tube). Colours are
//! display (sRGB) values, `#rrggbb` or `#rgb`, at most [`MAX_COLORS`].
//!
//! A style names a palette as `builtin` names ([`BUILTIN_PALETTES`]), a
//! bare name looked up as `<dir>/palettes/<name>.palette.toml` in the
//! scene's folder then its parent, or a `.palette.toml` path. Every read
//! goes through the vfs.

use flint_core::vfs;
use serde::Deserialize;
use std::collections::HashMap;
use std::fmt;
use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};

/// The most inks a palette holds (what the styles' uniforms make room for).
pub const MAX_COLORS: usize = 32;

/// File suffix of a palette.
pub const PALETTE_SUFFIX: &str = ".palette.toml";

/// The palettes built into the engine, by name.
pub const BUILTIN_PALETTES: &[&str] = &["cga", "cga-warm", "cga16", "phosphor", "amber", "mono"];

/// How a palette's ink is chosen for a colour.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PaletteMatch {
    /// The ink nearest the colour.
    #[default]
    Nearest,
    /// The inks are a ladder, darkest first; brightness picks the rung.
    Luminance,
}

impl PaletteMatch {
    /// The shader's number for it.
    pub fn index(self) -> u32 {
        match self {
            PaletteMatch::Nearest => 0,
            PaletteMatch::Luminance => 1,
        }
    }
}

/// A palette did not parse or could not be found.
#[derive(Clone, Debug, PartialEq)]
pub enum PaletteError {
    Read(String),
    Parse(String),
}

impl fmt::Display for PaletteError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            PaletteError::Read(e) => write!(f, "{e}"),
            PaletteError::Parse(e) => write!(f, "{e}"),
        }
    }
}

impl std::error::Error for PaletteError {}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct PaletteFile {
    #[serde(default, rename = "match")]
    matching: PaletteMatch,
    #[serde(default)]
    background: Option<String>,
    colors: Vec<String>,
}

/// A named set of inks and the ground behind them, in display values 0-1.
#[derive(Clone, Debug, PartialEq)]
pub struct Palette {
    name: String,
    colors: Vec<[f32; 3]>,
    background: [f32; 3],
    matching: PaletteMatch,
}

impl Palette {
    /// A palette from display-space colours. Errors on none or too many.
    pub fn new(
        name: impl Into<String>,
        colors: Vec<[f32; 3]>,
        background: [f32; 3],
        matching: PaletteMatch,
    ) -> Result<Self, PaletteError> {
        let name = name.into();
        if colors.is_empty() {
            return Err(PaletteError::Parse(format!(
                "palette `{name}` has no colors"
            )));
        }
        if colors.len() > MAX_COLORS {
            return Err(PaletteError::Parse(format!(
                "palette `{name}` has {} colors; at most {MAX_COLORS}",
                colors.len()
            )));
        }
        Ok(Self {
            name,
            colors,
            background,
            matching,
        })
    }

    /// Parse a `*.palette.toml`.
    pub fn from_toml(name: &str, text: &str) -> Result<Self, PaletteError> {
        let file: PaletteFile = toml::from_str(text)
            .map_err(|e| PaletteError::Parse(format!("palette `{name}`: {e}")))?;
        let colors = file
            .colors
            .iter()
            .map(|c| {
                parse_hex(c).map_err(|e| PaletteError::Parse(format!("palette `{name}`: {e}")))
            })
            .collect::<Result<Vec<_>, _>>()?;
        let background = match &file.background {
            Some(c) => {
                parse_hex(c).map_err(|e| PaletteError::Parse(format!("palette `{name}`: {e}")))?
            }
            None => [0.0; 3],
        };
        Self::new(name, colors, background, file.matching)
    }

    /// Read a `*.palette.toml` through the vfs.
    pub fn load(path: &Path) -> Result<Self, PaletteError> {
        let text = vfs::read_to_string(path)
            .map_err(|e| PaletteError::Read(format!("{}: {e}", path.display())))?;
        let name = path
            .file_name()
            .and_then(|n| n.to_str())
            .map(|n| n.trim_end_matches(PALETTE_SUFFIX).to_string())
            .unwrap_or_default();
        Self::from_toml(&name, &text)
    }

    /// A built-in palette by name.
    pub fn builtin(name: &str) -> Option<Arc<Palette>> {
        static BUILTINS: OnceLock<HashMap<&'static str, Arc<Palette>>> = OnceLock::new();
        BUILTINS
            .get_or_init(|| {
                BUILTIN_PALETTES
                    .iter()
                    .map(|n| {
                        let text = builtin_text(n).expect("every built-in has a file");
                        let p = Palette::from_toml(n, text).expect("built-in palettes parse");
                        (*n, Arc::new(p))
                    })
                    .collect()
            })
            .get(name)
            .cloned()
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    /// The inks, in file order.
    pub fn colors(&self) -> &[[f32; 3]] {
        &self.colors
    }

    pub fn background(&self) -> [f32; 3] {
        self.background
    }

    pub fn matching(&self) -> PaletteMatch {
        self.matching
    }

    /// The ink nearest `color` (display values; plain RGB distance).
    pub fn nearest(&self, color: [f32; 3]) -> [f32; 3] {
        let d = |c: &[f32; 3]| {
            (0..3)
                .map(|i| (c[i] - color[i]) * (c[i] - color[i]))
                .sum::<f32>()
        };
        *self
            .colors
            .iter()
            .min_by(|a, b| d(a).total_cmp(&d(b)))
            .expect("a palette has a colour")
    }

    /// The rung of a luminance ladder for tone `t` (0-1).
    pub fn rung(&self, t: f32) -> [f32; 3] {
        let n = self.colors.len();
        let i = ((t.clamp(0.0, 1.0) * n as f32) as usize).min(n - 1);
        self.colors[i]
    }

    /// The ink for a cell of full-value `hue` and tone `t`, as the shader
    /// picks it.
    pub fn ink(&self, hue: [f32; 3], t: f32) -> [f32; 3] {
        match self.matching {
            PaletteMatch::Nearest => self.nearest(hue),
            PaletteMatch::Luminance => self.rung(t),
        }
    }

    /// The inks as uniform slots (rgb + 0), padded to [`MAX_COLORS`].
    pub fn uniform_colors(&self) -> [[f32; 4]; MAX_COLORS] {
        let mut out = [[0.0; 4]; MAX_COLORS];
        for (slot, c) in out.iter_mut().zip(&self.colors) {
            *slot = [c[0], c[1], c[2], 0.0];
        }
        out
    }
}

/// `#rrggbb`, `rrggbb`, `#rgb` or `rgb` as display values 0-1.
pub fn parse_hex(text: &str) -> Result<[f32; 3], String> {
    let hex = text.trim().trim_start_matches('#');
    let digits: Vec<u32> = hex
        .chars()
        .map(|c| c.to_digit(16))
        .collect::<Option<_>>()
        .ok_or_else(|| format!("`{text}` is not a hex colour"))?;
    let bytes = match digits.len() {
        6 => [
            digits[0] * 16 + digits[1],
            digits[2] * 16 + digits[3],
            digits[4] * 16 + digits[5],
        ],
        3 => [digits[0] * 17, digits[1] * 17, digits[2] * 17],
        _ => return Err(format!("`{text}` is not #rrggbb or #rgb")),
    };
    Ok(bytes.map(|b| b as f32 / 255.0))
}

fn builtin_text(name: &str) -> Option<&'static str> {
    Some(match name {
        // CGA mode 4, palette 1 at high intensity: the cyan-magenta screen
        "cga" => {
            r##"colors = ["#55ffff", "#ff55ff", "#ffffff"]
background = "#000000""##
        }
        // CGA mode 4, palette 0 at high intensity: green, red, yellow
        "cga-warm" => {
            r##"colors = ["#55ff55", "#ff5555", "#ffff55"]
background = "#000000""##
        }
        // The sixteen text-mode colours, black as the ground
        "cga16" => {
            r##"colors = ["#0000aa", "#00aa00", "#00aaaa", "#aa0000", "#aa00aa", "#aa5500", "#aaaaaa",
          "#555555", "#5555ff", "#55ff55", "#55ffff", "#ff5555", "#ff55ff", "#ffff55", "#ffffff"]
background = "#000000""##
        }
        // A green phosphor tube: one hue, four brightnesses
        "phosphor" => {
            r##"match = "luminance"
colors = ["#1f7a38", "#33b850", "#5cff7a", "#c8ffd2"]
background = "#03100a""##
        }
        // An amber monitor
        "amber" => {
            r##"match = "luminance"
colors = ["#8a4a00", "#cc7a00", "#ffb000", "#ffe0a0"]
background = "#120800""##
        }
        // Greys on black
        "mono" => {
            r##"match = "luminance"
colors = ["#5a5a5a", "#999999", "#d0d0d0", "#ffffff"]
background = "#000000""##
        }
        _ => return None,
    })
}

/// Palettes by the name a style gives, loaded once each. Failures are
/// kept, so a bad name warns once rather than every frame.
#[derive(Default)]
pub struct PaletteCache {
    search: Vec<PathBuf>,
    palettes: HashMap<String, Result<Arc<Palette>, PaletteError>>,
}

impl PaletteCache {
    /// A cache that resolves names against `search`, in order.
    pub fn new(search: Vec<PathBuf>) -> Self {
        Self {
            search,
            palettes: HashMap::new(),
        }
    }

    /// The directories names resolve against.
    pub fn search(&self) -> &[PathBuf] {
        &self.search
    }

    /// The palette `name` names, loading it the first time.
    pub fn get(&mut self, name: &str) -> Result<Arc<Palette>, PaletteError> {
        if let Some(found) = self.palettes.get(name) {
            return found.clone();
        }
        let loaded = self.resolve(name);
        if let Err(e) = &loaded {
            tracing::warn!("palette `{name}`: {e}");
        }
        self.palettes.insert(name.to_string(), loaded.clone());
        loaded
    }

    /// Forget every palette, so the next [`Self::get`] reads files again.
    pub fn clear(&mut self) {
        self.palettes.clear();
    }

    fn resolve(&self, name: &str) -> Result<Arc<Palette>, PaletteError> {
        let candidates: Vec<PathBuf> = if name.ends_with(PALETTE_SUFFIX) {
            let mut c: Vec<PathBuf> = self.search.iter().map(|d| d.join(name)).collect();
            c.push(PathBuf::from(name));
            c
        } else {
            self.search
                .iter()
                .map(|d| d.join("palettes").join(format!("{name}{PALETTE_SUFFIX}")))
                .collect()
        };
        // A file beside the scene wins over a built-in of the same name
        if let Some(path) = candidates.iter().find(|p| vfs::is_file(p)) {
            return Palette::load(path).map(Arc::new);
        }
        if let Some(p) = Palette::builtin(name) {
            return Ok(p);
        }
        Err(PaletteError::Read(format!(
            "no palette `{name}` (built-in: {}; looked for {})",
            BUILTIN_PALETTES.join(", "),
            candidates
                .iter()
                .map(|p| p.display().to_string())
                .collect::<Vec<_>>()
                .join(", ")
        )))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_builtin_parses() {
        for name in BUILTIN_PALETTES {
            let p = Palette::builtin(name).unwrap();
            assert_eq!(p.name(), *name);
            assert!(!p.colors().is_empty());
        }
        assert!(Palette::builtin("plaid").is_none());
    }

    #[test]
    fn hex_parses_long_and_short() {
        assert_eq!(parse_hex("#ffffff").unwrap(), [1.0, 1.0, 1.0]);
        assert_eq!(parse_hex("000").unwrap(), [0.0, 0.0, 0.0]);
        assert_eq!(parse_hex("#f80").unwrap(), [1.0, 136.0 / 255.0, 0.0]);
        assert!(parse_hex("#ff").is_err());
        assert!(parse_hex("#gggggg").is_err());
    }

    #[test]
    fn a_file_parses_with_its_defaults() {
        let p = Palette::from_toml("two", r##"colors = ["#ff0000", "#0000ff"]"##).unwrap();
        assert_eq!(p.matching(), PaletteMatch::Nearest);
        assert_eq!(p.background(), [0.0; 3]);
        assert_eq!(p.colors().len(), 2);
        assert!(Palette::from_toml("none", "colors = []").is_err());
        assert!(Palette::from_toml("typo", r##"colours = ["#fff"]"##).is_err());
        let many = format!("colors = [{}]", vec!["\"#fff\""; MAX_COLORS + 1].join(","));
        assert!(Palette::from_toml("many", &many).is_err());
    }

    #[test]
    fn cga_sends_each_hue_to_its_nearest_ink() {
        let cga = Palette::builtin("cga").unwrap();
        let cyan = [0x55 as f32 / 255.0, 1.0, 1.0];
        let magenta = [1.0, 0x55 as f32 / 255.0, 1.0];
        assert_eq!(cga.ink([0.0, 1.0, 1.0], 0.5), cyan);
        assert_eq!(cga.ink([0.0, 0.8, 1.0], 0.5), cyan);
        assert_eq!(cga.ink([1.0, 0.0, 0.9], 0.5), magenta);
        assert_eq!(cga.ink([1.0, 1.0, 1.0], 0.5), [1.0; 3]);
        // A full-value red lands on magenta, never on the black ground
        assert_eq!(cga.ink([1.0, 0.0, 0.0], 0.1), magenta);
    }

    #[test]
    fn a_luminance_ladder_climbs_with_tone() {
        let p = Palette::builtin("phosphor").unwrap();
        let c = p.colors();
        assert_eq!(p.ink([1.0, 0.0, 0.0], 0.0), c[0]);
        assert_eq!(p.ink([1.0, 0.0, 0.0], 0.3), c[1]);
        assert_eq!(p.ink([0.0, 0.0, 1.0], 0.99), c[3]);
        assert_eq!(p.ink([0.0, 0.0, 1.0], 1.0), c[3]);
    }

    #[test]
    fn the_cache_prefers_a_file_then_a_builtin() {
        let dir = std::env::temp_dir().join(format!("flint-palette-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("palettes")).unwrap();
        std::fs::write(
            dir.join("palettes/cga.palette.toml"),
            r##"colors = ["#123456"]"##,
        )
        .unwrap();
        let mut cache = PaletteCache::new(vec![dir.clone()]);
        assert_eq!(cache.get("cga").unwrap().colors().len(), 1);
        assert_eq!(
            cache.get("amber").unwrap().matching(),
            PaletteMatch::Luminance
        );
        assert!(cache.get("nope").is_err());
        std::fs::remove_dir_all(&dir).ok();
    }
}
