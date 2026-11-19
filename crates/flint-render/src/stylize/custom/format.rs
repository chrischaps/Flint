//! The `.shader.toml` format: what a description may say, and its checks
//! (moved from `custom.rs`, #742).

use super::*;

/// The file suffix of a style description.
pub const SUFFIX: &str = ".shader.toml";

/// The only `version` this loader reads.
pub const VERSION: i64 = 1;

/// One `[params.<name>]` table.
#[derive(Debug, Clone, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ParamDef {
    pub default: f32,
    pub min: f32,
    pub max: f32,
    #[serde(default)]
    pub label: Option<String>,
    #[serde(default)]
    pub help: Option<String>,
    #[serde(default)]
    pub integer: bool,
}

/// A parsed `*.shader.toml`.
#[derive(Debug, Clone, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ShaderStyleDef {
    pub version: i64,
    pub name: String,
    #[serde(default)]
    pub label: Option<String>,
    #[serde(default = "default_stage")]
    pub stage: String,
    /// The one pass's WGSL, for a style of one pass; a style of several
    /// lists [`passes`](Self::passes) instead.
    #[serde(default)]
    pub shader: Option<String>,
    #[serde(default)]
    pub inputs: Vec<String>,
    #[serde(default)]
    pub params: BTreeMap<String, ParamDef>,
    /// String options (#413): a choice the shader reads as a `u32` index,
    /// or an image the scene picks by file name.
    #[serde(default)]
    pub options: BTreeMap<String, OptionDef>,
    /// Images of the style's own (#413): a LUT, a paper, a brush sheet.
    #[serde(default)]
    pub images: BTreeMap<String, ImageDef>,
    /// Bloom reads this style's result (#300); hdr styles only.
    #[serde(default)]
    pub feeds_bloom: bool,
    /// How far past its entities, in points, a per-entity style changes a
    /// pixel (#538): further out, `style` returns `p.color` as it is. With
    /// it the framework draws the style's last pass only inside its
    /// entities' screen bounds grown by this, and its passthrough around
    /// them. Mask-reading styles only.
    #[serde(default)]
    pub reach: Option<f32>,
    /// `[[passes]]` (#412): each a WGSL `fn style` of its own, run in
    /// order. Every pass but the last writes a target of the style's own;
    /// the last writes the stage's output.
    #[serde(default)]
    pub passes: Vec<PassDef>,
}

/// One `[[passes]]` entry (#412).
#[derive(Debug, Clone, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PassDef {
    /// For errors, F4 and logs; `pass1`, `pass2`... when absent.
    #[serde(default)]
    pub name: Option<String>,
    pub shader: String,
    /// The target this pass writes; none on the last pass.
    #[serde(default)]
    pub target: Option<String>,
    /// `rgba16f` (the default), `rgba8` or `r32f`.
    #[serde(default)]
    pub format: Option<String>,
    /// The target's size as a fraction of the stage's (1/16..1, default 1).
    #[serde(default)]
    pub scale: Option<f32>,
    /// Keep the target from frame to frame (ping-ponged): a pass that reads
    /// it at or before the one writing it reads last frame's.
    #[serde(default)]
    pub history: bool,
    /// The targets this pass reads, by name.
    #[serde(default)]
    pub reads: Vec<String>,
}

/// The formats a target can be (#412).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TargetFormat {
    /// Four half floats: HDR colour, signed values. Filtered.
    Rgba16f,
    /// Four bytes, 0..1, stored as written (not sRGB). Filtered.
    Rgba8,
    /// One float, full precision. Read unfiltered (nearest).
    R32f,
}

impl TargetFormat {
    pub(super) fn parse(s: &str) -> Option<Self> {
        match s {
            "rgba16f" => Some(Self::Rgba16f),
            "rgba8" => Some(Self::Rgba8),
            "r32f" => Some(Self::R32f),
            _ => None,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Self::Rgba16f => "rgba16f",
            Self::Rgba8 => "rgba8",
            Self::R32f => "r32f",
        }
    }

    pub fn wgpu(self) -> wgpu::TextureFormat {
        match self {
            Self::Rgba16f => wgpu::TextureFormat::Rgba16Float,
            Self::Rgba8 => wgpu::TextureFormat::Rgba8Unorm,
            Self::R32f => wgpu::TextureFormat::R32Float,
        }
    }

    pub fn bytes(self) -> u32 {
        match self {
            Self::Rgba16f => 8,
            Self::Rgba8 | Self::R32f => 4,
        }
    }

    /// Whether the linear sampler may read it (`R32Float` isn't filterable
    /// without a feature most web GPUs lack).
    pub fn filterable(self) -> bool {
        self != Self::R32f
    }
}

/// A target of the style's own, checked.
#[derive(Debug, Clone, PartialEq)]
pub struct TargetSource {
    pub name: String,
    pub format: TargetFormat,
    pub scale: f32,
    pub history: bool,
    /// The pass that writes it.
    pub writer: usize,
}

impl TargetSource {
    /// Its size at a stage of `width` x `height`.
    pub fn size(&self, width: u32, height: u32) -> [u32; 2] {
        let s = |v: u32| ((v as f32 * self.scale).round() as u32).max(1);
        [s(width), s(height)]
    }

    /// Bytes for each pixel of the stage: its texel times its area, twice
    /// with history.
    pub fn bytes_per_stage_pixel(&self) -> f32 {
        self.format.bytes() as f32 * self.scale * self.scale * if self.history { 2.0 } else { 1.0 }
    }
}

/// A target a pass reads.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ReadSource {
    /// Index into [`StyleSource::targets`].
    pub target: usize,
    /// It reads last frame's: a history target read at or before the pass
    /// that writes it.
    pub previous: bool,
}

/// A pass, checked.
#[derive(Debug, Clone, PartialEq)]
pub struct PassSource {
    pub name: String,
    pub wgsl_path: PathBuf,
    /// Index into [`StyleSource::targets`]; `None` writes the output.
    pub target: Option<usize>,
    pub reads: Vec<ReadSource>,
}

/// How many passes a style may have.
pub const MAX_PASSES: usize = 8;

/// How many targets a style may have: each costs memory at every
/// resolution. With [`MAX_IMAGES`], the input and the three optional
/// buffers, a pass binds at most 16 sampled textures, every backend's limit.
pub const MAX_TARGETS: usize = 4;

/// What a style's targets may cost together, in bytes for each pixel of
/// its stage: four full-size `rgba16f` targets (66 MB at 1080p).
pub const TARGET_BUDGET: f32 = 32.0;

/// One `[images.<name>]` table (#413): a file beside the `.shader.toml`,
/// read in the shader with `flint_image_<name>(uv)`.
#[derive(Debug, Clone, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ImageDef {
    /// A PNG or JPEG beside the description (subfolders allowed, `..` not).
    pub file: String,
    /// Decode the file's sRGB to linear light as it's read (a colour
    /// picture for an hdr style). Off: the shader gets the values as
    /// stored, which at the display stage are display values like
    /// `p.color`, and which a LUT or a height map wants untouched.
    #[serde(default)]
    pub srgb: bool,
    /// Upload a mip chain (#558), so a read smaller than its texels (a
    /// paper tiled small) averages them instead of aliasing; the clamped
    /// and tiled helpers pick the level from the screen. Off for a LUT,
    /// which must be read texel for texel.
    #[serde(default)]
    pub mips: bool,
}

/// One `[options.<name>]` table (#413).
#[derive(Debug, Clone, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OptionDef {
    /// `"choice"` (the default): one of `choices`, read as
    /// `params.<name>`, a `u32` index into them. `"image"`: a file name,
    /// read as `flint_image_<name>(uv)` (and its index in `params`).
    #[serde(default)]
    pub kind: Option<String>,
    pub default: String,
    /// A choice option's values. An image option without them offers the
    /// images in its default's folder.
    #[serde(default)]
    pub choices: Vec<String>,
    #[serde(default)]
    pub label: Option<String>,
    #[serde(default)]
    pub help: Option<String>,
    /// An image option's files decode sRGB to linear (see [`ImageDef`]).
    #[serde(default)]
    pub srgb: bool,
    /// An image option's files get a mip chain (see [`ImageDef`], #558).
    #[serde(default)]
    pub mips: bool,
}

/// What an option holds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OptionKind {
    Choice,
    Image,
}

/// An option, checked: its choices in index order.
#[derive(Debug, Clone)]
pub struct OptionSource {
    pub kind: OptionKind,
    pub default: String,
    pub choices: Vec<String>,
    pub label: Option<String>,
    pub help: Option<String>,
    /// An image option with no `choices`: they are the images found in its
    /// default's folder, which the reload poll lists again (#558).
    pub discovered: bool,
}

impl OptionSource {
    /// The index `value` stands for: its place in `choices`, or the
    /// default's for a value outside them (warned at scene load, #318).
    pub fn index(&self, value: &str) -> u32 {
        let at = |v: &str| self.choices.iter().position(|c| c == v);
        at(value).or_else(|| at(&self.default)).unwrap_or(0) as u32
    }
}

/// One image a style binds, in binding order.
#[derive(Debug, Clone, PartialEq)]
pub struct ImageSource {
    pub name: String,
    /// `Some(file)` for an `[images]` table; `None` for an image option,
    /// whose file is the scene's choice.
    pub file: Option<String>,
    pub srgb: bool,
    /// Uploaded with a mip chain (#558).
    pub mips: bool,
}

/// How many images a style may bind: with the input and the three
/// optional buffers it stays inside every backend's 16 sampled textures.
pub const MAX_IMAGES: usize = 8;

/// The image files a style may read (what [`crate::open_image`] decodes
/// into 8-bit colour).
pub(super) const IMAGE_EXTENSIONS: &[&str] = &["png", "jpg", "jpeg"];

pub(super) fn default_stage() -> String {
    "display".into()
}

/// `file` is a relative path to an image inside the style's folder.
pub(super) fn check_image_file(
    at: &dyn std::fmt::Display,
    key: &str,
    file: &str,
) -> Result<(), String> {
    let path = Path::new(file);
    if file.is_empty() || path.is_absolute() || path.components().any(|c| c.as_os_str() == "..") {
        return Err(format!(
            "{at}: {key} = \"{file}\" must be a file beside the .shader.toml (subfolders, no ..)"
        ));
    }
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_ascii_lowercase())
        .unwrap_or_default();
    if !IMAGE_EXTENSIONS.contains(&ext.as_str()) {
        return Err(format!(
            "{at}: {key} = \"{file}\" is not an image this engine reads ({})",
            IMAGE_EXTENSIONS.join(", ")
        ));
    }
    Ok(())
}

/// The images in `default`'s folder (relative to `dir`), each as a path
/// relative to `dir`, sorted, `default` among them: an image option's
/// choices when it lists none. Read through the vfs, so a bundle lists the
/// files it packed.
pub(super) fn images_beside(dir: &Path, default: &str) -> Vec<String> {
    let names = flint_core::vfs::read_dir(dir.join(choice_folder(default)))
        .unwrap_or_default()
        .into_iter()
        .filter_map(|p| Some(p.file_name()?.to_str()?.to_string()));
    image_choices(default, names)
}

/// The folder, relative to the style's, that an image option with no
/// `choices` finds them in: its default's.
pub(super) fn choice_folder(default: &str) -> &Path {
    Path::new(default).parent().unwrap_or(Path::new(""))
}

/// An image option's choices from the file names in its default's folder
/// ([`choice_folder`]): the images among them as paths from the style's
/// folder, with `default`, sorted. What [`images_beside`] lists at load and
/// the reload poll lists again (#558), so the two agree on a folder.
pub(super) fn image_choices(default: &str, names: impl Iterator<Item = String>) -> Vec<String> {
    let sub = choice_folder(default);
    let mut found: Vec<String> = names
        .filter_map(|file| {
            let ext = Path::new(&file).extension()?.to_str()?.to_ascii_lowercase();
            IMAGE_EXTENSIONS.contains(&ext.as_str()).then(|| {
                if sub.as_os_str().is_empty() {
                    file
                } else {
                    format!("{}/{file}", sub.to_string_lossy().replace('\\', "/"))
                }
            })
        })
        .collect();
    if !found.iter().any(|f| f == default) {
        found.push(default.to_string());
    }
    found.sort();
    found.dedup();
    found
}

/// Which optional buffers a style reads.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Inputs {
    pub depth: bool,
    pub mask: bool,
    /// The shared geometry edges (#301): `flint_edges`, `flint_edge`.
    pub edges: bool,
}

impl Inputs {
    pub(super) fn any(self) -> bool {
        self.depth || self.mask || self.edges
    }
}

/// A style description, checked: what [`compose`] and the pass need.
#[derive(Debug, Clone)]
pub struct StyleSource {
    pub name: String,
    pub label: String,
    pub stage: StyleStage,
    pub inputs: Inputs,
    /// Bloom reads the style's result by default (#300).
    pub feeds_bloom: bool,
    /// The style's `reach` in points (#538): set, its last pass is drawn
    /// only within that of its entities (`StyleFrame::styled_rect`).
    pub reach: Option<f32>,
    /// The component an entity takes the style with (#414): `<name>_style`
    /// for a style that reads the mask, its fields the parameters. It
    /// makes the style per-entity, as `ascii_style` does ASCII: a
    /// `style_mask` naming it (or the component) switches it on with no
    /// table, and each entity's settings reach its own pixels as `params`.
    pub component: Option<String>,
    /// Parameters in uniform order (by name).
    pub params: Vec<(String, ParamDef)>,
    /// Options in uniform order (by name), after the parameters (#413).
    pub options: Vec<(String, OptionSource)>,
    /// Images in binding order (by name; group 3, #413).
    pub images: Vec<ImageSource>,
    /// The `.shader.toml`.
    pub toml_path: PathBuf,
    /// The WGSL file it names, beside it: the last pass's, the one that
    /// writes the output (a style of one pass has only that).
    pub wgsl_path: PathBuf,
    /// Its passes in the order they run (#412; one for a style with a
    /// top-level `shader`); the last writes the stage's output.
    pub passes: Vec<PassSource>,
    /// The targets the passes write, in pass order.
    pub targets: Vec<TargetSource>,
    /// What the check found that doesn't stop the style (a target nothing
    /// reads), one line each.
    pub notes: Vec<String>,
}

impl StyleSource {
    /// The WGSL files, one a pass.
    pub fn wgsl_paths(&self) -> impl Iterator<Item = &Path> {
        self.passes.iter().map(|p| p.wgsl_path.as_path())
    }

    /// Whether any target keeps history.
    pub fn has_history(&self) -> bool {
        self.targets.iter().any(|t| t.history)
    }

    /// The index of the last pass, which writes the output.
    pub fn last_pass(&self) -> usize {
        self.passes.len().saturating_sub(1)
    }

    /// The folder the description is in: its shader and images are here.
    pub fn dir(&self) -> &Path {
        self.toml_path.parent().unwrap_or(Path::new("."))
    }

    /// How many 4-byte fields its `Params` uniform has before the pad.
    pub fn uniform_fields(&self) -> usize {
        self.params.len() + self.options.len()
    }
}

/// Check the `[[passes]]` of a description: names, targets, reads, the
/// order they're written in and what they cost (#412).
pub(super) fn check_passes(
    at: &dyn std::fmt::Display,
    dir: &Path,
    shader: Option<String>,
    defs: Vec<PassDef>,
) -> Result<(Vec<PassSource>, Vec<TargetSource>, Vec<String>), String> {
    let beside = |file: &str| -> Result<PathBuf, String> {
        let p = Path::new(file);
        if p.is_absolute() || p.components().any(|c| c.as_os_str() == "..") {
            return Err(format!(
                "{at}: shader = \"{file}\" must be a file beside it"
            ));
        }
        Ok(dir.join(p))
    };
    let defs = match (shader, defs.is_empty()) {
        (Some(_), false) => {
            return Err(format!(
                "{at}: a style has a top-level shader or [[passes]], not both"
            ))
        }
        (None, true) => {
            return Err(format!(
                "{at}: no shader: name one (shader = \"x.wgsl\") or list [[passes]]"
            ))
        }
        (Some(shader), true) => {
            return Ok((
                vec![PassSource {
                    name: "style".into(),
                    wgsl_path: beside(&shader)?,
                    target: None,
                    reads: Vec::new(),
                }],
                Vec::new(),
                Vec::new(),
            ))
        }
        (None, false) => defs,
    };
    if defs.len() > MAX_PASSES {
        return Err(format!("{at}: at most {MAX_PASSES} passes"));
    }
    let last = defs.len() - 1;
    let names: Vec<String> = defs
        .iter()
        .enumerate()
        .map(|(i, d)| d.name.clone().unwrap_or_else(|| format!("pass{}", i + 1)))
        .collect();
    for (i, name) in names.iter().enumerate() {
        if !is_identifier(name) {
            return Err(format!(
                "{at}: pass \"{name}\": a name is lowercase letters, digits and _"
            ));
        }
        if names[..i].contains(name) {
            return Err(format!("{at}: two passes are named \"{name}\""));
        }
    }
    // Targets, each written by one pass
    let mut targets: Vec<TargetSource> = Vec::new();
    let mut writes: Vec<Option<usize>> = Vec::new();
    for (i, d) in defs.iter().enumerate() {
        let pass = &names[i];
        let Some(target) = &d.target else {
            if i != last {
                return Err(format!(
                    "{at}: pass \"{pass}\" writes nothing: every pass but the last names a target"
                ));
            }
            if d.format.is_some() || d.scale.is_some() || d.history {
                return Err(format!(
                    "{at}: pass \"{pass}\" has format, scale or history but no target"
                ));
            }
            writes.push(None);
            continue;
        };
        if i == last {
            return Err(format!(
                "{at}: pass \"{pass}\" is the last: it writes the stage's output, not target \"{target}\""
            ));
        }
        if !is_identifier(target) {
            return Err(format!(
                "{at}: target \"{target}\": a name is lowercase letters, digits and _"
            ));
        }
        if let Some(other) = targets.iter().find(|t| &t.name == target) {
            return Err(format!(
                "{at}: passes \"{}\" and \"{pass}\" both write target \"{target}\"; give each its own",
                names[other.writer]
            ));
        }
        let format = match d.format.as_deref() {
            None => TargetFormat::Rgba16f,
            Some(f) => TargetFormat::parse(f).ok_or_else(|| {
                format!("{at}: pass \"{pass}\": format = \"{f}\"; it is rgba16f, rgba8 or r32f")
            })?,
        };
        let scale = d.scale.unwrap_or(1.0);
        if !(1.0 / 16.0..=1.0).contains(&scale) {
            return Err(format!(
                "{at}: pass \"{pass}\": scale = {scale}; it is 0.0625..1 of the stage's size"
            ));
        }
        writes.push(Some(targets.len()));
        targets.push(TargetSource {
            name: target.clone(),
            format,
            scale,
            history: d.history,
            writer: i,
        });
    }
    if targets.len() > MAX_TARGETS {
        return Err(format!(
            "{at}: {} targets; a style has at most {MAX_TARGETS}",
            targets.len()
        ));
    }
    let cost: f32 = targets
        .iter()
        .map(TargetSource::bytes_per_stage_pixel)
        .sum();
    if cost > TARGET_BUDGET {
        let each: Vec<String> = targets
            .iter()
            .map(|t| format!("{} {}", t.name, t.bytes_per_stage_pixel()))
            .collect();
        return Err(format!(
            "{at}: the targets cost {cost} bytes a pixel ({}); the budget is {TARGET_BUDGET} \
             (four full-size rgba16f): lower a scale, use rgba8 or r32f, or drop a history",
            each.join(", ")
        ));
    }
    // Reads: every one names a target; a target is read only after it is
    // written, unless it keeps history (then a read at or before its
    // writer is last frame's)
    let mut reads: Vec<Vec<ReadSource>> = Vec::new();
    for (i, d) in defs.iter().enumerate() {
        let pass = &names[i];
        let mut list = Vec::new();
        for (k, name) in d.reads.iter().enumerate() {
            if d.reads[..k].contains(name) {
                return Err(format!("{at}: pass \"{pass}\" reads \"{name}\" twice"));
            }
            let Some(t) = targets.iter().position(|t| &t.name == name) else {
                let known: Vec<&str> = targets.iter().map(|t| t.name.as_str()).collect();
                return Err(format!(
                    "{at}: pass \"{pass}\" reads \"{name}\", which no pass writes (targets: {})",
                    if known.is_empty() {
                        "none".to_string()
                    } else {
                        known.join(", ")
                    }
                ));
            };
            let writer = targets[t].writer;
            let previous = writer >= i;
            if previous && !targets[t].history {
                return Err(read_before_write(at, &names, &targets, &defs, i, t));
            }
            list.push(ReadSource {
                target: t,
                previous,
            });
        }
        reads.push(list);
    }
    let mut notes = Vec::new();
    for t in &targets {
        let read = reads
            .iter()
            .flatten()
            .any(|r| targets[r.target].name == t.name);
        if !read {
            notes.push(format!(
                "{at}: warning: pass \"{}\" writes target \"{}\" and no pass reads it",
                names[t.writer], t.name
            ));
        }
    }
    let passes = defs
        .iter()
        .zip(names)
        .zip(writes.into_iter().zip(reads))
        .map(|((d, name), (target, reads))| {
            Ok(PassSource {
                name,
                wgsl_path: beside(&d.shader)?,
                target,
                reads,
            })
        })
        .collect::<Result<Vec<_>, String>>()?;
    Ok((passes, targets, notes))
}

/// The message for pass `reader` reading target `t` before it is written:
/// a cycle when the writer depends, through what it reads, on the reader.
pub(super) fn read_before_write(
    at: &dyn std::fmt::Display,
    names: &[String],
    targets: &[TargetSource],
    defs: &[PassDef],
    reader: usize,
    t: usize,
) -> String {
    let writer = targets[t].writer;
    let target = &targets[t].name;
    if writer == reader {
        return format!(
            "{at}: pass \"{}\" reads target \"{target}\", which it writes: a cycle \
             (add history = true to read last frame's)",
            names[reader]
        );
    }
    // Walk back from the writer through the passes whose targets it reads:
    // reaching the reader is a cycle
    let depends = |p: usize| -> Vec<usize> {
        defs[p]
            .reads
            .iter()
            .filter_map(|n| targets.iter().find(|t| &t.name == n))
            .filter(|t| !t.history || t.writer < p)
            .map(|t| t.writer)
            .collect()
    };
    let mut path = vec![writer];
    let mut seen = vec![false; defs.len()];
    fn walk(
        p: usize,
        goal: usize,
        depends: &dyn Fn(usize) -> Vec<usize>,
        seen: &mut [bool],
        path: &mut Vec<usize>,
    ) -> bool {
        if p == goal {
            return true;
        }
        if std::mem::replace(&mut seen[p], true) {
            return false;
        }
        for q in depends(p) {
            path.push(q);
            if walk(q, goal, depends, seen, path) {
                return true;
            }
            path.pop();
        }
        false
    }
    if walk(writer, reader, &depends, &mut seen, &mut path) {
        let mut chain: Vec<&str> = vec![names[reader].as_str()];
        chain.extend(path.iter().map(|&p| names[p].as_str()));
        return format!(
            "{at}: a cycle: passes {} each read what the next writes (\"{}\" reads target \"{target}\")",
            chain
                .iter()
                .map(|n| format!("\"{n}\""))
                .collect::<Vec<_>>()
                .join(" -> "),
            names[reader]
        );
    }
    format!(
        "{at}: pass \"{}\" reads target \"{target}\" before pass \"{}\" writes it; \
         move \"{}\" after \"{}\" (or add history = true to read last frame's)",
        names[reader], names[writer], names[reader], names[writer]
    )
}

impl ShaderStyleDef {
    /// Check the description and resolve its shader beside `toml_path`.
    pub fn check(self, toml_path: &Path) -> Result<StyleSource, String> {
        let at = shown(toml_path);
        if self.version != VERSION {
            return Err(format!(
                "{at}: version = {} is not one this engine reads (it reads {VERSION})",
                self.version
            ));
        }
        let stem = toml_path
            .file_name()
            .and_then(|f| f.to_str())
            .and_then(|f| f.strip_suffix(SUFFIX))
            .unwrap_or("");
        if self.name != stem {
            return Err(format!(
                "{at}: name = \"{}\" must match the file name (\"{stem}\")",
                self.name
            ));
        }
        if !is_identifier(&self.name) {
            return Err(format!(
                "{at}: name \"{}\" must be lowercase letters, digits and _",
                self.name
            ));
        }
        if super::super::is_builtin(&self.name) {
            return Err(format!(
                "{at}: \"{}\" is a built-in style; pick another name",
                self.name
            ));
        }
        let stage = match self.stage.as_str() {
            "display" => StyleStage::Display,
            "hdr" => StyleStage::Hdr,
            other => {
                return Err(format!(
                    "{at}: stage = \"{other}\"; it is \"display\" or \"hdr\""
                ))
            }
        };
        if self.feeds_bloom && stage != StyleStage::Hdr {
            return Err(format!(
                "{at}: feeds_bloom = true needs stage = \"hdr\"; a display style draws after bloom"
            ));
        }
        let mut inputs = Inputs::default();
        for input in &self.inputs {
            match input.as_str() {
                "color" => {}
                "depth" => inputs.depth = true,
                "mask" => inputs.mask = true,
                "edges" => inputs.edges = true,
                other => {
                    return Err(format!(
                        "{at}: unknown input \"{other}\"; inputs are color, depth, mask, edges"
                    ))
                }
            }
        }
        if let Some(reach) = self.reach {
            if !inputs.mask {
                return Err(format!(
                    "{at}: reach needs inputs = [\"mask\"]: it says how far past its \
                     entities a per-entity style draws, and without the mask it has none"
                ));
            }
            if !(reach.is_finite() && (0.0..=MAX_REACH).contains(&reach)) {
                return Err(format!(
                    "{at}: reach = {reach}; it is 0..{MAX_REACH} points"
                ));
            }
        }
        for (name, p) in &self.params {
            if !is_identifier(name) || name.starts_with("flint") || name.starts_with('_') {
                return Err(format!(
                    "{at}: parameter \"{name}\" must be lowercase letters, digits and _, \
                     and not start with flint or _"
                ));
            }
            if super::super::RESERVED_KEYS.contains(&name.as_str()) {
                return Err(format!(
                    "{at}: parameter \"{name}\" is taken: every style's table reads {}",
                    super::super::RESERVED_KEYS.join(", ")
                ));
            }
            if !(p.min <= p.default && p.default <= p.max) {
                return Err(format!(
                    "{at}: params.{name}: default {} is outside {}..{}",
                    p.default, p.min, p.max
                ));
            }
        }
        if self.params.len() + self.options.len() > MAX_PARAMS {
            return Err(format!(
                "{at}: at most {MAX_PARAMS} parameters and options together"
            ));
        }
        let dir = toml_path.parent().unwrap_or(Path::new("."));
        let (passes, targets, notes) = check_passes(&at, dir, self.shader, self.passes)?;

        // Options and images share the parameters' names: one table key,
        // one `params` field or one `flint_image_` helper each
        let check_name = |what: &str, name: &str| -> Result<(), String> {
            if !is_identifier(name) || name.starts_with("flint") {
                return Err(format!(
                    "{at}: {what} \"{name}\" must be lowercase letters, digits and _, \
                     and not start with flint"
                ));
            }
            if super::super::RESERVED_KEYS.contains(&name) {
                return Err(format!(
                    "{at}: {what} \"{name}\" is taken: every style's table reads {}",
                    super::super::RESERVED_KEYS.join(", ")
                ));
            }
            if self.params.contains_key(name) {
                return Err(format!(
                    "{at}: {what} \"{name}\" has a parameter's name; each name is one setting"
                ));
            }
            Ok(())
        };
        let mut options = Vec::new();
        let mut images = Vec::new();
        for (name, o) in self.options {
            check_name("option", &name)?;
            let kind = match o.kind.as_deref().unwrap_or("choice") {
                "choice" => OptionKind::Choice,
                "image" => OptionKind::Image,
                other => {
                    return Err(format!(
                        "{at}: options.{name}: kind = \"{other}\"; it is \"choice\" or \"image\""
                    ))
                }
            };
            let choices = match kind {
                OptionKind::Choice => {
                    if o.srgb || o.mips {
                        let key = if o.srgb { "srgb" } else { "mips" };
                        return Err(format!(
                            "{at}: options.{name}: {key} is for an image option (kind = \"image\")"
                        ));
                    }
                    if o.choices.is_empty() {
                        return Err(format!(
                            "{at}: options.{name}: a choice option lists its choices"
                        ));
                    }
                    o.choices.clone()
                }
                OptionKind::Image => {
                    check_image_file(&at, &format!("options.{name}.default"), &o.default)?;
                    for c in &o.choices {
                        check_image_file(&at, &format!("options.{name}.choices"), c)?;
                    }
                    if o.choices.is_empty() {
                        images_beside(dir, &o.default)
                    } else {
                        o.choices.clone()
                    }
                }
            };
            if !choices.contains(&o.default) {
                return Err(format!(
                    "{at}: options.{name}: default \"{}\" is not one of its choices ({})",
                    o.default,
                    choices.join(", ")
                ));
            }
            if let Some(dup) = choices
                .iter()
                .enumerate()
                .find(|(i, c)| choices[..*i].contains(c))
            {
                return Err(format!(
                    "{at}: options.{name}: \"{}\" is listed twice",
                    dup.1
                ));
            }
            if kind == OptionKind::Image {
                images.push(ImageSource {
                    name: name.clone(),
                    file: None,
                    srgb: o.srgb,
                    mips: o.mips,
                });
            }
            options.push((
                name,
                OptionSource {
                    kind,
                    default: o.default,
                    discovered: kind == OptionKind::Image && o.choices.is_empty(),
                    choices,
                    label: o.label,
                    help: o.help,
                },
            ));
        }
        for (name, image) in self.images {
            check_name("image", &name)?;
            if options.iter().any(|(o, _)| *o == name) {
                return Err(format!(
                    "{at}: image \"{name}\" has an option's name; an image option is \
                     already an image"
                ));
            }
            check_image_file(&at, &format!("images.{name}.file"), &image.file)?;
            images.push(ImageSource {
                name,
                file: Some(image.file),
                srgb: image.srgb,
                mips: image.mips,
            });
        }
        if images.len() > MAX_IMAGES {
            return Err(format!(
                "{at}: at most {MAX_IMAGES} images (image options among them)"
            ));
        }
        images.sort_by(|a, b| a.name.cmp(&b.name));
        Ok(StyleSource {
            component: inputs.mask.then(|| component_name(&self.name)),
            label: self
                .label
                .unwrap_or_else(|| super::super::humanize(&self.name)),
            name: self.name,
            stage,
            inputs,
            feeds_bloom: self.feeds_bloom,
            reach: self.reach,
            params: self.params.into_iter().collect(),
            options,
            images,
            toml_path: toml_path.to_path_buf(),
            wgsl_path: passes
                .last()
                .expect("check_passes gives a pass")
                .wgsl_path
                .clone(),
            passes,
            targets,
            notes,
        })
    }
}

/// The largest `reach` a style may declare, in points (#538): past a few
/// hundred the rect is the frame anyway.
pub const MAX_REACH: f32 = 512.0;

/// How many parameters a style may declare: 64 floats is a 272-byte
/// uniform, well inside every backend's limit.
pub const MAX_PARAMS: usize = 64;

/// The component a mask-reading user style `name` is set on entities with.
pub fn component_name(name: &str) -> String {
    format!("{name}_style")
}

pub(in crate::stylize) fn is_identifier(name: &str) -> bool {
    let mut chars = name.chars();
    chars.next().is_some_and(|c| c.is_ascii_lowercase())
        && chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
}
