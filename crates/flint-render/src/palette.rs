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
//! screen), by distance in OKLab (#320), where distance follows how
//! different two colours look; a style can push the colour's chroma before
//! matching and dither between the two nearest inks ([`Palette::pick`], and
//! `palette.wgsl` for the GPU's). `luminance` lists the inks darkest first
//! and picks one by the cell's brightness alone (a one-colour phosphor
//! tube). Colours are
//! display (sRGB) values, `#rrggbb` or `#rgb`, at most [`MAX_COLORS`].
//!
//! A style names a palette as `builtin` names ([`BUILTIN_PALETTES`]), a
//! bare name looked up as `<dir>/palettes/<name>.palette.toml` in the
//! scene's folder then its parent, or a `.palette.toml` path. Every read
//! goes through the vfs.

use crate::file_watch::FileStamps;
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
pub const BUILTIN_PALETTES: &[&str] = &[
    "cga", "cga-warm", "cga16", "phosphor", "amber", "mono", "paper",
];

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

    /// The inks in OKLab, in file order (what the shader matches against).
    pub fn lab_colors(&self) -> Vec<[f32; 3]> {
        self.colors.iter().map(|c| oklab(*c)).collect()
    }

    /// The ink nearest `color` (display values), by OKLab distance (#320).
    pub fn nearest(&self, color: [f32; 3]) -> [f32; 3] {
        self.pick(color, 1.0, 0.0, [0, 0])
    }

    /// The two inks nearest `color` once its chroma is scaled by `chroma`:
    /// the indices of the nearest and the next, and their squared OKLab
    /// distances. The first of equals wins, as in the shader.
    pub fn nearest_two(&self, color: [f32; 3], chroma: f32) -> ([usize; 2], [f32; 2]) {
        nearest_two_in(&self.colors, color, chroma)
    }

    /// The ink for `color` with its chroma scaled by `chroma`, and
    /// `dither` (0..1) of the way to mixing the two nearest inks by a 4x4
    /// Bayer pattern on the cell grid: `cell` is the cell's index. The CPU
    /// reference for `pal_pick` in `palette.wgsl`.
    pub fn pick(&self, color: [f32; 3], chroma: f32, dither: f32, cell: [i32; 2]) -> [f32; 3] {
        pick_in(&self.colors, color, chroma, dither, cell)
    }

    /// The table the palette style (#324) quantizes to: the ground is a
    /// colour like any other there, so a dark pixel can fall to CGA's black
    /// or a light one to paper. Nearest puts the ground first; a ladder
    /// takes it in at its own lightness, so paper's ground is the top rung
    /// and phosphor's the bottom one. At most [`MAX_COLORS`] + 1 colours.
    pub fn table(&self) -> Vec<[f32; 3]> {
        let mut table = self.colors.clone();
        match self.matching {
            PaletteMatch::Nearest => table.insert(0, self.background),
            PaletteMatch::Luminance => {
                let at = table
                    .iter()
                    .position(|c| luma(*c) > luma(self.background))
                    .unwrap_or(table.len());
                table.insert(at, self.background);
            }
        }
        table
    }

    /// The colour the palette style draws for a block of colour `color`
    /// (display values) in block `cell`. Nearest: `pal_pick` over
    /// [`table`](Self::table) with `chroma` and `dither`. A ladder: the
    /// rung for the colour's luminance, moved up to half a rung either way
    /// by `dither` and the cell's Bayer threshold (as `palette.wgsl` in
    /// `stylize/` draws it).
    pub fn quantize(&self, color: [f32; 3], chroma: f32, dither: f32, cell: [i32; 2]) -> [f32; 3] {
        let table = self.table();
        match self.matching {
            PaletteMatch::Nearest => pick_in(&table, color, chroma, dither, cell),
            PaletteMatch::Luminance => {
                let n = table.len();
                let t = luma(color).clamp(0.0, 1.0);
                let td = (t + (bayer4(cell) - 0.5) * dither / n as f32).clamp(0.0, 1.0);
                table[((td * n as f32) as usize).min(n - 1)]
            }
        }
    }

    /// The rung of a luminance ladder for tone `t` (0-1).
    pub fn rung(&self, t: f32) -> [f32; 3] {
        let n = self.colors.len();
        let i = ((t.clamp(0.0, 1.0) * n as f32) as usize).min(n - 1);
        self.colors[i]
    }

    /// The ink for a cell of full-value `hue` and tone `t`, as the shader
    /// picks it with chroma 1 and no dither.
    pub fn ink(&self, hue: [f32; 3], t: f32) -> [f32; 3] {
        self.ink_in(hue, t, 1.0, 0.0, [0, 0])
    }

    /// [`Self::ink`] with the palette look's `chroma` and `dither` for the
    /// cell at `cell` (a ladder ignores both).
    pub fn ink_in(
        &self,
        hue: [f32; 3],
        t: f32,
        chroma: f32,
        dither: f32,
        cell: [i32; 2],
    ) -> [f32; 3] {
        match self.matching {
            PaletteMatch::Nearest => self.pick(hue, chroma, dither, cell),
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

/// The palette matcher in WGSL (#320): `pal_oklab`, `pal_bayer` and
/// `pal_pick`. The includer defines `palette_ink(i)` and
/// `palette_ink_lab(i)`.
pub const PALETTE_WGSL: &str = include_str!("palette.wgsl");

/// Display (sRGB-encoded) 0..1 to OKLab (Björn Ottosson, 2020): L 0..1,
/// a and b about ±0.3. As `pal_oklab` in `palette.wgsl`.
pub fn oklab(display: [f32; 3]) -> [f32; 3] {
    let lin = |c: f32| {
        let c = c.clamp(0.0, 1.0);
        if c <= 0.04045 {
            c / 12.92
        } else {
            ((c + 0.055) / 1.055).powf(2.4)
        }
    };
    let [r, g, b] = display.map(lin);
    let l = 0.412_221_46 * r + 0.536_332_55 * g + 0.051_445_995 * b;
    let m = 0.211_903_5 * r + 0.680_699_5 * g + 0.107_396_96 * b;
    let s = 0.088_302_46 * r + 0.281_718_85 * g + 0.629_978_7 * b;
    let [l, m, s] = [l, m, s].map(|x| x.max(0.0).powf(1.0 / 3.0));
    [
        0.210_454_26 * l + 0.793_617_8 * m - 0.004_072_047 * s,
        1.977_998_5 * l - 2.428_592_2 * m + 0.450_593_7 * s,
        0.025_904_037 * l + 0.782_771_77 * m - 0.808_675_77 * s,
    ]
}

/// Rec. 709 luminance of a display colour, as the shaders weigh it.
pub fn luma(c: [f32; 3]) -> f32 {
    0.2126 * c[0] + 0.7152 * c[1] + 0.0722 * c[2]
}

/// [`Palette::nearest_two`] over any list of inks.
pub fn nearest_two_in(inks: &[[f32; 3]], color: [f32; 3], chroma: f32) -> ([usize; 2], [f32; 2]) {
    let mut lab = oklab(color);
    lab[1] *= chroma;
    lab[2] *= chroma;
    let (mut best, mut second) = (0, 0);
    let (mut best_d, mut second_d) = (1e9_f32, 1e9_f32);
    for (i, ink) in inks.iter().enumerate() {
        let e = oklab(*ink);
        let d = (0..3)
            .map(|k| (e[k] - lab[k]) * (e[k] - lab[k]))
            .sum::<f32>();
        if d < best_d {
            (second, second_d) = (best, best_d);
            (best, best_d) = (i, d);
        } else if d < second_d {
            (second, second_d) = (i, d);
        }
    }
    ([best, second], [best_d, second_d])
}

/// [`Palette::pick`] over any list of inks: `pal_pick` on the CPU.
pub fn pick_in(
    inks: &[[f32; 3]],
    color: [f32; 3],
    chroma: f32,
    dither: f32,
    cell: [i32; 2],
) -> [f32; 3] {
    let ([best, second], [best_d, second_d]) = nearest_two_in(inks, color, chroma);
    if dither > 0.0 && inks.len() > 1 {
        let d1 = best_d.sqrt();
        let share = d1 / (d1 + second_d.sqrt()).max(1e-6);
        if bayer4(cell) < share * dither {
            return inks[second];
        }
    }
    inks[best]
}

/// A 4x4 Bayer threshold in (0, 1) for the cell at `cell`, as `pal_bayer`.
pub fn bayer4(cell: [i32; 2]) -> f32 {
    const M: [u32; 16] = [0, 8, 2, 10, 12, 4, 14, 6, 3, 11, 1, 9, 15, 7, 13, 5];
    let (x, y) = ((cell[0] & 3) as usize, (cell[1] & 3) as usize);
    (M[y * 4 + x] as f32 + 0.5) / 16.0
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
        // Dark inks on a light ground (#324): ink, wash and pencil on
        // cream paper. The palette style takes the paper in as the top
        // rung; ASCII draws it with `ground = "inverse"` (see the book)
        "paper" => {
            r##"match = "luminance"
colors = ["#1b1a22", "#4b4654", "#8c8479", "#c9bfae"]
background = "#f3ecdc""##
        }
        _ => return None,
    })
}

/// Palettes by the name a style gives, loaded once each. Failures are
/// kept, so a bad name warns once rather than every frame. [`Self::poll`]
/// forgets them all when a file one depended on changes on disk (#323).
#[derive(Default)]
pub struct PaletteCache {
    search: Vec<PathBuf>,
    palettes: HashMap<String, Result<Arc<Palette>, PaletteError>>,
    stamps: FileStamps,
}

impl PaletteCache {
    /// A cache that resolves names against `search`, in order.
    pub fn new(search: Vec<PathBuf>) -> Self {
        Self {
            search,
            palettes: HashMap::new(),
            stamps: FileStamps::default(),
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
        let candidates = self.candidates(name);
        // Watch the candidates up to the one that loads: an edit to it, or a
        // file appearing ahead of it (or at all, after a failure), reloads
        let found = candidates.iter().position(vfs::is_file);
        for path in &candidates[..found.map_or(candidates.len(), |i| i + 1)] {
            self.stamps.watch(path);
        }
        let loaded = self.resolve(name, &candidates, found);
        if let Err(e) = &loaded {
            tracing::warn!("palette `{name}`: {e}");
        }
        self.palettes.insert(name.to_string(), loaded.clone());
        loaded
    }

    /// Forget every palette, so the next [`Self::get`] reads files again.
    pub fn clear(&mut self) {
        self.palettes.clear();
        self.stamps.clear();
    }

    /// Whether a file a palette was read from (or looked for at) has
    /// changed on disk since; if so the cache is cleared, so the next
    /// [`Self::get`] reads it again. Native only (a bundle never changes):
    /// on the web this is always `false`. A few `stat`s; hosts call it a
    /// few times a second, not every frame.
    pub fn poll(&mut self) -> bool {
        let changed = self.stamps.changed();
        if changed {
            self.clear();
        }
        changed
    }

    fn candidates(&self, name: &str) -> Vec<PathBuf> {
        if name.ends_with(PALETTE_SUFFIX) {
            let mut c: Vec<PathBuf> = self.search.iter().map(|d| d.join(name)).collect();
            c.push(PathBuf::from(name));
            c
        } else {
            self.search
                .iter()
                .map(|d| d.join("palettes").join(format!("{name}{PALETTE_SUFFIX}")))
                .collect()
        }
    }

    fn resolve(
        &self,
        name: &str,
        candidates: &[PathBuf],
        found: Option<usize>,
    ) -> Result<Arc<Palette>, PaletteError> {
        // A file beside the scene wins over a built-in of the same name
        if let Some(i) = found {
            return Palette::load(&candidates[i]).map(Arc::new);
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
    fn oklab_matches_the_reference_values() {
        // Ottosson's: white is L 1 with no chroma, sRGB red L 0.628
        let w = oklab([1.0; 3]);
        assert!(
            (w[0] - 1.0).abs() < 1e-3 && w[1].abs() < 1e-3 && w[2].abs() < 1e-3,
            "{w:?}"
        );
        let r = oklab([1.0, 0.0, 0.0]);
        for (got, want) in r.iter().zip([0.627_955, 0.224_863, 0.125_846]) {
            assert!((got - want).abs() < 1e-3, "{r:?}");
        }
        assert_eq!(oklab([0.0; 3]), [0.0; 3]);
    }

    #[test]
    fn cga16_sends_full_value_hues_to_the_bright_half() {
        // Plain RGB put (1,0,0) on #aa0000: dim ink on top of density
        let p = Palette::builtin("cga16").unwrap();
        let hex = |h: &str| parse_hex(h).unwrap();
        assert_eq!(p.nearest([1.0, 0.0, 0.0]), hex("#ff5555"));
        assert_eq!(p.nearest([0.0, 1.0, 0.0]), hex("#55ff55"));
        assert_eq!(p.nearest([0.0, 0.0, 1.0]), hex("#5555ff"));
        assert_eq!(p.nearest([1.0, 1.0, 1.0]), hex("#ffffff"));
    }

    #[test]
    fn chroma_pushes_a_muted_colour_off_the_neutral() {
        // A greyish blue sky at full value is nearest cga16's light grey;
        // doubled chroma reaches the cyan
        let p = Palette::builtin("cga16").unwrap();
        let sky = [0.55 / 0.9, 0.7 / 0.9, 1.0];
        assert_eq!(p.pick(sky, 1.0, 0.0, [0, 0]), parse_hex("#aaaaaa").unwrap());
        assert_eq!(p.pick(sky, 2.0, 0.0, [0, 0]), parse_hex("#55ffff").unwrap());
    }

    #[test]
    fn dither_mixes_the_two_nearest_by_share() {
        let p = Palette::new(
            "bw",
            vec![[0.0; 3], [1.0; 3]],
            [0.0; 3],
            PaletteMatch::Nearest,
        )
        .unwrap();
        let cells: Vec<[i32; 2]> = (0..4).flat_map(|y| (0..4).map(move |x| [x, y])).collect();
        let whites = |c: [f32; 3], dither: f32| {
            cells
                .iter()
                .filter(|&&cell| p.pick(c, 1.0, dither, cell) == [1.0; 3])
                .count()
        };
        // Off: the nearest everywhere; on, a grey between the inks takes a
        // share of the far ink that grows toward the middle, and the
        // Bayer cells are each threshold once
        assert_eq!(whites([0.3; 3], 0.0), 0);
        let some = whites([0.2; 3], 1.0);
        let more = whites([0.35; 3], 1.0);
        assert!(0 < some && some < more && more <= 8, "{some} {more}");
        assert_eq!(whites([0.0; 3], 1.0), 0, "an ink itself never dithers");
        let mut t: Vec<f32> = cells.iter().map(|c| bayer4(*c)).collect();
        t.sort_by(f32::total_cmp);
        assert!(t
            .windows(2)
            .all(|w| (w[1] - w[0] - 1.0 / 16.0).abs() < 1e-6));
        assert_eq!(bayer4([-1, -1]), bayer4([3, 3]));
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

    /// #323: an edited palette, a typo fixed on disk, and a file that
    /// appears ahead of a built-in each clear the cache at the next poll.
    #[test]
    fn the_cache_reloads_what_changes_on_disk() {
        let dir = std::env::temp_dir().join(format!("flint-palette-poll-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("palettes")).unwrap();
        let file = dir.join("palettes/tide.palette.toml");
        std::fs::write(&file, r##"colors = ["#ff0000"]"##).unwrap();
        let mut cache = PaletteCache::new(vec![dir.clone()]);
        let before = cache.get("tide").unwrap();
        assert!(!cache.poll(), "nothing changed yet");
        assert!(Arc::ptr_eq(&before, &cache.get("tide").unwrap()));

        // An edited ink
        std::fs::write(&file, r##"colors = ["#00ff00", "#0000ff"]"##).unwrap();
        assert!(cache.poll());
        let after = cache.get("tide").unwrap();
        assert_eq!(after.colors().len(), 2);
        assert_eq!(after.colors()[0], [0.0, 1.0, 0.0]);
        assert!(!cache.poll(), "re-read, so settled again");

        // A name that failed is looked for again once its file appears
        assert!(cache.get("reef").is_err());
        assert!(!cache.poll());
        std::fs::write(
            dir.join("palettes/reef.palette.toml"),
            r##"colors = ["#ffffff"]"##,
        )
        .unwrap();
        assert!(cache.poll());
        assert!(cache.get("reef").is_ok());

        // A file beside the scene shadows a built-in once it exists
        assert_eq!(
            cache.get("amber").unwrap().matching(),
            PaletteMatch::Luminance
        );
        std::fs::write(
            dir.join("palettes/amber.palette.toml"),
            r##"match = "nearest"
colors = ["#ffaa00"]"##,
        )
        .unwrap();
        assert!(cache.poll());
        assert_eq!(
            cache.get("amber").unwrap().matching(),
            PaletteMatch::Nearest
        );
        std::fs::remove_dir_all(&dir).ok();
    }
}
