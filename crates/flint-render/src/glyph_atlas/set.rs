//! Glyph sets: an atlas plus the ramps and edge glyphs a style names, read
//! from a `*.glyphs.toml`.
//!
//! ```toml
//! default_ramp = "classic"    # the ramp a style gets when it names none
//!                             # (top level, so before the first [table])
//! [font]
//! file = "Hack-Regular.ttf"   # beside this file; omit it for the built-in 8x8 set
//! size = 16                   # TTF: line height in pixels (default 16)
//! # A bitmap grid instead of a TTF:
//! # file = "terminal.png"
//! # cell = [8, 12]            # the grid's cell, in pixels
//! # first_char = 32           # cell 0 holds this character...
//! # chars = " !\"#..."        # ...or name every cell's character in order
//! # invert = false            # true for dark ink on light paper
//! # Or cells drawn by hand in builtin_8x8.txt's format (`== c`, then a row
//! # of `#` ink and `.` paper per pixel row):
//! # file = "halftone.txt"
//! # cell = [8, 12]
//!
//! [ramps.classic]             # no ink to most ink
//! glyphs = " .:-=+*#%@"
//! [ramps.detailed]
//! glyphs = " .'`^\",:;Il!i><~+_-?][}{1)(|\\/tfjrx..."
//! sort = "coverage"           # re-order by each glyph's measured ink
//!
//! [edges]                     # each defaults to the one shown
//! horizontal = "-"
//! vertical = "|"
//! diagonal = "/"
//! antidiagonal = "\\"
//! ```
//!
//! A set whose file names no `[ramps]` gets the built-in ones (those its font
//! can draw). A TTF atlas holds printable ASCII plus every glyph the set's
//! ramps and edges name; a missing one is an error naming it.

use super::{printable_ascii, GlyphAtlas, GlyphError};
use crate::file_watch::FileStamps;
use flint_core::vfs;
use serde::Deserialize;
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};

/// The name a style uses for the built-in set.
pub const BUILTIN_SET: &str = "builtin";
/// The file suffix of a glyph set.
pub const GLYPHS_SUFFIX: &str = ".glyphs.toml";
/// TTF line height when a file gives none.
pub const DEFAULT_FONT_SIZE: f32 = 16.0;

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct GlyphsFile {
    #[serde(default)]
    font: FontDef,
    default_ramp: Option<String>,
    #[serde(default)]
    ramps: BTreeMap<String, RampDef>,
    #[serde(default)]
    edges: EdgesDef,
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct FontDef {
    file: Option<String>,
    size: Option<f32>,
    cell: Option<[u32; 2]>,
    first_char: Option<u32>,
    chars: Option<String>,
    #[serde(default)]
    invert: bool,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RampDef {
    glyphs: String,
    #[serde(default)]
    sort: RampOrder,
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct EdgesDef {
    horizontal: Option<char>,
    vertical: Option<char>,
    diagonal: Option<char>,
    antidiagonal: Option<char>,
}

/// How a ramp's glyphs are ordered.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RampOrder {
    /// The order the file wrote them in.
    #[default]
    AsWritten,
    /// Least ink to most, measured in the atlas (ties keep the written order).
    Coverage,
}

/// Glyphs ordered from no ink to the most, that a style maps luminance onto.
#[derive(Debug, Clone, PartialEq)]
pub struct GlyphRamp {
    name: String,
    glyphs: Vec<char>,
    indices: Vec<u32>,
}

impl GlyphRamp {
    /// A ramp of `glyphs` from `atlas`, which must hold every one of them.
    pub fn new(
        name: impl Into<String>,
        glyphs: &str,
        order: RampOrder,
        atlas: &GlyphAtlas,
    ) -> Result<Self, GlyphError> {
        let mut glyphs: Vec<char> = glyphs.chars().collect();
        let missing: BTreeSet<char> = glyphs
            .iter()
            .copied()
            .filter(|c| !atlas.contains(*c))
            .collect();
        if !missing.is_empty() {
            return Err(GlyphError::Missing(missing.into_iter().collect()));
        }
        let name = name.into();
        if glyphs.is_empty() {
            return Err(GlyphError::Parse(format!("ramp `{name}` has no glyphs")));
        }
        if order == RampOrder::Coverage {
            glyphs.sort_by(|a, b| {
                atlas
                    .coverage(*a)
                    .unwrap_or(0.0)
                    .total_cmp(&atlas.coverage(*b).unwrap_or(0.0))
            });
        }
        let indices = glyphs
            .iter()
            .map(|c| atlas.index_of(*c).expect("checked above"))
            .collect();
        Ok(Self {
            name,
            glyphs,
            indices,
        })
    }

    /// The ramp's name in its set.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// The glyphs, least ink first.
    pub fn glyphs(&self) -> &[char] {
        &self.glyphs
    }

    /// Each glyph's atlas index, in ramp order: what a shader's ramp buffer holds.
    pub fn atlas_indices(&self) -> &[u32] {
        &self.indices
    }

    /// Number of steps in the ramp.
    pub fn len(&self) -> usize {
        self.glyphs.len()
    }

    /// Always false: a ramp has at least one glyph.
    pub fn is_empty(&self) -> bool {
        self.glyphs.is_empty()
    }

    /// The step for `luminance` (0 dark .. 1 bright, clamped): equal bands,
    /// the last glyph only at full brightness's band.
    pub fn step(&self, luminance: f32) -> usize {
        let n = self.glyphs.len();
        let l = if luminance.is_nan() {
            0.0
        } else {
            luminance.clamp(0.0, 1.0)
        };
        ((l * n as f32) as usize).min(n - 1)
    }

    /// The glyph for `luminance` (0 dark .. 1 bright).
    pub fn pick(&self, luminance: f32) -> char {
        self.glyphs[self.step(luminance)]
    }
}

/// The glyphs drawn along silhouette edges, by the edge's direction on screen.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EdgeGlyphs {
    /// An edge running left to right: `-`.
    pub horizontal: char,
    /// An edge running top to bottom: `|`.
    pub vertical: char,
    /// Rising to the right: `/`.
    pub diagonal: char,
    /// Falling to the right: `\`.
    pub antidiagonal: char,
}

impl Default for EdgeGlyphs {
    fn default() -> Self {
        Self {
            horizontal: '-',
            vertical: '|',
            diagonal: '/',
            antidiagonal: '\\',
        }
    }
}

impl EdgeGlyphs {
    /// The four glyphs, horizontal, vertical, diagonal, antidiagonal.
    pub fn all(&self) -> [char; 4] {
        [
            self.horizontal,
            self.vertical,
            self.diagonal,
            self.antidiagonal,
        ]
    }

    /// The glyph for an edge whose direction on screen is `angle` radians
    /// from the +x axis (y up); direction, not orientation, so `a` and
    /// `a + PI` pick the same glyph.
    pub fn for_angle(&self, angle: f32) -> char {
        let a = angle.rem_euclid(std::f32::consts::PI);
        let eighth = std::f32::consts::FRAC_PI_8;
        if !(eighth..7.0 * eighth).contains(&a) {
            self.horizontal
        } else if a < 3.0 * eighth {
            self.diagonal
        } else if a < 5.0 * eighth {
            self.vertical
        } else {
            self.antidiagonal
        }
    }
}

/// An atlas with the ramps and edge glyphs a style names.
#[derive(Debug, Clone)]
pub struct GlyphSet {
    name: String,
    atlas: Arc<GlyphAtlas>,
    ramps: BTreeMap<String, GlyphRamp>,
    default_ramp: String,
    edges: EdgeGlyphs,
}

impl GlyphSet {
    /// The built-in set: the hand-drawn 8x8 atlas and its ramps (`classic`,
    /// `blocks`, `minimal`, `detailed`). Built once per process.
    pub fn builtin() -> Arc<GlyphSet> {
        static BUILTIN: OnceLock<Arc<GlyphSet>> = OnceLock::new();
        BUILTIN
            .get_or_init(|| {
                Arc::new(
                    Self::from_toml(
                        BUILTIN_SET,
                        include_str!("builtin.glyphs.toml"),
                        Path::new(""),
                    )
                    .expect("the built-in glyph set builds"),
                )
            })
            .clone()
    }

    /// Load a `*.glyphs.toml` through the vfs. The set is named for the file
    /// (`fonts/terminal.glyphs.toml` is `terminal`); its font file resolves
    /// beside it.
    pub fn load(path: &Path) -> Result<Self, GlyphError> {
        let text = vfs::read_to_string(path)
            .map_err(|e| GlyphError::Read(format!("{}: {e}", path.display())))?;
        let name = set_name(path);
        let dir = path.parent().unwrap_or_else(|| Path::new(""));
        Self::from_toml(&name, &text, dir).map_err(|e| match e {
            GlyphError::Parse(m) => GlyphError::Parse(format!("{}: {m}", path.display())),
            other => other,
        })
    }

    /// Build a set from `*.glyphs.toml` text; a font file resolves against
    /// `dir` and is read through the vfs.
    pub fn from_toml(name: &str, text: &str, dir: &Path) -> Result<Self, GlyphError> {
        let file: GlyphsFile = toml::from_str(text)
            .map_err(|e| GlyphError::Parse(e.to_string().trim_end().to_string()))?;

        let edges = EdgeGlyphs {
            horizontal: file.edges.horizontal.unwrap_or('-'),
            vertical: file.edges.vertical.unwrap_or('|'),
            diagonal: file.edges.diagonal.unwrap_or('/'),
            antidiagonal: file.edges.antidiagonal.unwrap_or('\\'),
        };
        let atlas = build_atlas(&file, &edges, dir)?;

        let missing: BTreeSet<char> = edges
            .all()
            .into_iter()
            .filter(|c| !atlas.contains(*c))
            .collect();
        if !missing.is_empty() {
            return Err(GlyphError::Missing(missing.into_iter().collect()));
        }

        let mut ramps = BTreeMap::new();
        for (ramp_name, def) in &file.ramps {
            let ramp = GlyphRamp::new(ramp_name, &def.glyphs, def.sort, &atlas)
                .map_err(|e| in_ramp(ramp_name, e))?;
            ramps.insert(ramp_name.clone(), ramp);
        }
        if ramps.is_empty() && name != BUILTIN_SET {
            // No ramps of its own: the built-in ones its font can draw.
            for (ramp_name, ramp) in &Self::builtin().ramps {
                let glyphs: String = ramp.glyphs.iter().collect();
                if let Ok(r) = GlyphRamp::new(ramp_name, &glyphs, RampOrder::Coverage, &atlas) {
                    ramps.insert(ramp_name.clone(), r);
                }
            }
        }
        if ramps.is_empty() {
            return Err(GlyphError::Parse("the set has no ramps".into()));
        }
        let default_ramp = match file.default_ramp {
            Some(r) if ramps.contains_key(&r) => r,
            Some(r) => {
                return Err(GlyphError::Parse(format!(
                    "default_ramp `{r}` is not one of its ramps ({})",
                    ramps.keys().cloned().collect::<Vec<_>>().join(", ")
                )))
            }
            None if ramps.contains_key("classic") => "classic".to_string(),
            None => ramps.keys().next().cloned().expect("not empty"),
        };

        Ok(Self {
            name: name.to_string(),
            atlas: Arc::new(atlas),
            ramps,
            default_ramp,
            edges,
        })
    }

    /// The set's name: its file's stem, or `builtin`.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// The glyph atlas.
    pub fn atlas(&self) -> &Arc<GlyphAtlas> {
        &self.atlas
    }

    /// A ramp by name.
    pub fn ramp(&self, name: &str) -> Option<&GlyphRamp> {
        self.ramps.get(name)
    }

    /// The ramp a style gets when it names none.
    pub fn default_ramp(&self) -> &GlyphRamp {
        &self.ramps[&self.default_ramp]
    }

    /// Every ramp, by name.
    pub fn ramps(&self) -> impl Iterator<Item = &GlyphRamp> {
        self.ramps.values()
    }

    /// The edge glyphs.
    pub fn edges(&self) -> EdgeGlyphs {
        self.edges
    }
}

fn in_ramp(ramp: &str, e: GlyphError) -> GlyphError {
    match e {
        GlyphError::Parse(m) => GlyphError::Parse(format!("ramp `{ramp}`: {m}")),
        other => other,
    }
}

/// `fonts/terminal.glyphs.toml` -> `terminal`.
fn set_name(path: &Path) -> String {
    let file = path
        .file_name()
        .map(|f| f.to_string_lossy().into_owned())
        .unwrap_or_default();
    file.strip_suffix(GLYPHS_SUFFIX)
        .or_else(|| file.strip_suffix(".toml"))
        .unwrap_or(&file)
        .to_string()
}

fn build_atlas(
    file: &GlyphsFile,
    edges: &EdgeGlyphs,
    dir: &Path,
) -> Result<GlyphAtlas, GlyphError> {
    let font = &file.font;
    let Some(font_file) = &font.file else {
        // Built-in cells.
        return Ok((*GlyphAtlas::builtin()).clone());
    };
    let path = dir.join(font_file);
    let ext = path
        .extension()
        .map(|e| e.to_string_lossy().to_ascii_lowercase())
        .unwrap_or_default();
    match ext.as_str() {
        "ttf" | "otf" => {
            let bytes = vfs::read(&path)
                .map_err(|e| GlyphError::Read(format!("{}: {e}", path.display())))?;
            let mut chars: BTreeSet<char> = printable_ascii().collect();
            for ramp in file.ramps.values() {
                chars.extend(ramp.glyphs.chars());
            }
            chars.extend(edges.all());
            if file.ramps.is_empty() {
                // The built-in ramps it may inherit, where the font has them.
                let font_ref = ab_glyph::FontRef::try_from_slice(&bytes)
                    .map_err(|e| GlyphError::Font(e.to_string()))?;
                for ramp in GlyphSet::builtin().ramps() {
                    chars.extend(
                        ramp.glyphs()
                            .iter()
                            .filter(|c| ab_glyph::Font::glyph_id(&font_ref, **c).0 != 0),
                    );
                }
            }
            GlyphAtlas::from_ttf(&bytes, font.size.unwrap_or(DEFAULT_FONT_SIZE), chars)
        }
        "png" | "jpg" | "jpeg" => {
            let cell = font.cell.ok_or_else(|| {
                GlyphError::Parse(format!(
                    "{font_file} is a grid image: give its cell = [w, h]"
                ))
            })?;
            let image = crate::open_image(&path)
                .map_err(|e| GlyphError::Read(format!("{}: {e}", path.display())))?;
            let chars: Vec<char> = match &font.chars {
                Some(chars) => chars.chars().collect(),
                None => {
                    let first = font.first_char.unwrap_or(0x20);
                    let cells = (image.width() / cell[0]) * (image.height() / cell[1]);
                    (first..first + cells).filter_map(char::from_u32).collect()
                }
            };
            GlyphAtlas::from_grid_image(&image, cell, chars, font.invert)
        }
        "txt" => {
            // Hand-drawn cells in the built-in set's format (#335)
            let cell = font.cell.ok_or_else(|| {
                GlyphError::Parse(format!(
                    "{font_file} is drawn pixel text: give its cell = [w, h]"
                ))
            })?;
            let text = vfs::read_to_string(&path)
                .map_err(|e| GlyphError::Read(format!("{}: {e}", path.display())))?;
            GlyphAtlas::from_pixel_text(&text, cell)
        }
        _ => Err(GlyphError::Parse(format!(
            "font file {font_file}: expected .ttf, .otf, .png, .jpg or .txt"
        ))),
    }
}

/// Glyph sets by the name a style gives, loaded once each.
///
/// A name is `builtin`, a path ending `.glyphs.toml` (against each search
/// directory, then as given), or a bare set name looked up as
/// `<dir>/fonts/<name>.glyphs.toml` in each search directory, the scene's
/// folder then its parent, as the player finds `fonts/`. Failures are kept
/// too, so a bad name is reported once rather than every frame.
/// [`Self::poll`] forgets them all when a file one depended on (the
/// `.glyphs.toml`, its font file, a candidate searched ahead of it) changes
/// on disk (#323).
#[derive(Default)]
pub struct GlyphSetCache {
    search: Vec<PathBuf>,
    sets: HashMap<String, Result<Arc<GlyphSet>, GlyphError>>,
    stamps: FileStamps,
}

impl GlyphSetCache {
    /// A cache that resolves names against `search`, in order.
    pub fn new(search: Vec<PathBuf>) -> Self {
        Self {
            search,
            sets: HashMap::new(),
            stamps: FileStamps::default(),
        }
    }

    /// Search the scene's folder, then its parent.
    pub fn for_scene(scene_path: &Path) -> Self {
        let dir = scene_path.parent().unwrap_or_else(|| Path::new("."));
        let mut search = vec![dir.to_path_buf()];
        if let Some(parent) = dir.parent() {
            search.push(parent.to_path_buf());
        }
        Self::new(search)
    }

    /// The set `name` names, loading it the first time.
    pub fn get(&mut self, name: &str) -> Result<Arc<GlyphSet>, GlyphError> {
        if let Some(found) = self.sets.get(name) {
            return found.clone();
        }
        let loaded = self.resolve(name);
        if let Err(e) = &loaded {
            tracing::warn!("glyph set `{name}`: {e}");
        }
        self.sets.insert(name.to_string(), loaded.clone());
        loaded
    }

    /// Forget every set, so the next [`Self::get`] reads the files again.
    pub fn clear(&mut self) {
        self.sets.clear();
        self.stamps.clear();
    }

    /// Whether a file a set was read from (or looked for at) has changed on
    /// disk since; if so the cache is cleared, so the next [`Self::get`]
    /// reads it again and hands out a new `Arc`. Native only (a bundle
    /// never changes): on the web this is always `false`. A few `stat`s;
    /// hosts call it a few times a second, not every frame.
    pub fn poll(&mut self) -> bool {
        let changed = self.stamps.changed();
        if changed {
            self.clear();
        }
        changed
    }

    fn resolve(&mut self, name: &str) -> Result<Arc<GlyphSet>, GlyphError> {
        if name.is_empty() || name == BUILTIN_SET {
            return Ok(GlyphSet::builtin());
        }
        let candidates = self.candidates(name);
        // Watch the candidates up to the one that loads, and its font file:
        // an edit to either, or a file appearing ahead of it (or at all,
        // after a failure), reloads
        let found = candidates.iter().position(vfs::is_file);
        for path in &candidates[..found.map_or(candidates.len(), |i| i + 1)] {
            self.stamps.watch(path);
        }
        #[cfg(not(target_arch = "wasm32"))]
        if let Some(font) = found.and_then(|i| font_file_of(&candidates[i])) {
            self.stamps.watch(&font);
        }
        match found.map(|i| &candidates[i]) {
            Some(path) => GlyphSet::load(path).map(Arc::new),
            None => Err(GlyphError::Read(format!(
                "no glyph set `{name}` (looked for {})",
                candidates
                    .iter()
                    .map(|p| p.display().to_string())
                    .collect::<Vec<_>>()
                    .join(", ")
            ))),
        }
    }

    fn candidates(&self, name: &str) -> Vec<PathBuf> {
        if name.ends_with(GLYPHS_SUFFIX) {
            let mut c: Vec<PathBuf> = self.search.iter().map(|d| d.join(name)).collect();
            c.push(PathBuf::from(name));
            c
        } else {
            self.search
                .iter()
                .map(|d| d.join("fonts").join(format!("{name}{GLYPHS_SUFFIX}")))
                .collect()
        }
    }
}

/// The font file a `*.glyphs.toml` names (`[font] file`), resolved beside
/// it, read leniently so a set that fails to build still watches its font.
#[cfg(not(target_arch = "wasm32"))]
fn font_file_of(path: &Path) -> Option<PathBuf> {
    let text = vfs::read_to_string(path).ok()?;
    let value: toml::Value = toml::from_str(&text).ok()?;
    let file = value.get("font")?.get("file")?.as_str()?;
    Some(path.parent().unwrap_or_else(|| Path::new("")).join(file))
}
