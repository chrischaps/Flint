//! User-authored styles (#274, ADR 0112): a game describes a style in
//! `styles/<name>.shader.toml` and writes one WGSL function beside it.
//!
//! ```toml
//! # styles/duotone.shader.toml
//! version = 1
//! name = "duotone"
//! label = "Duotone"
//! stage = "display"          # or "hdr"
//! shader = "duotone.wgsl"
//! inputs = ["depth"]         # colour is always there; also "mask", "edges"
//!
//! [params.mix]
//! default = 1.0
//! min = 0.0
//! max = 1.0
//! help = "How far to the two inks"
//! ```
//!
//! ```wgsl
//! // styles/duotone.wgsl
//! fn style(p: StylePixel) -> vec4<f32> {
//!     let l = flint_luma(p.color.rgb);
//!     return vec4<f32>(mix(p.color.rgb, vec3<f32>(l), params.mix), 1.0);
//! }
//! ```
//!
//! The framework writes everything else: a prelude with the bindings, the
//! `params` struct (one `f32` per declared parameter), helpers
//! (`flint_sample`, `flint_view_depth`, `flint_style_blend`, ...) and the
//! entry points. At the display stage `p.color` and the returned colour
//! are display values (what the eye sees) on any target format: the
//! prelude decodes and encodes an sRGB target, so a user never meets the
//! gotcha the built-ins work around by hand.
//!
//! A style that reads the mask (`inputs = ["mask"]`) is per-entity
//! (#414), as ASCII is: it gets the component `<name>_style`, a
//! `style_mask` naming it switches it on with no table, and `params` is
//! a private copy the entry point fills with the pixel's own row of the
//! settings table (row 0 the scene's, then one a variant, and a slot
//! table from mask id to row), so a shader written for the whole frame
//! reads each entity's settings unchanged.
//!
//! The composed module is parsed and validated with naga before wgpu sees
//! it (natively), and an error comes back as `file:line:col` in the user's
//! file. A style that fails draws its input unchanged (a passthrough) and
//! its error is kept for the host to show ([`errors`]). The WGSL is polled
//! for changes (natively, a few times a second) and rebuilt in place; an
//! edit that fails keeps the last good build drawing.
//!
//! The registry is append-only ([`super::register_runtime`]): a style keeps
//! its index for the run. An edited `.shader.toml` is read again by
//! [`reload_changed`] (#275), which hosts call a few times a second: the
//! new descriptor takes the old one's index ([`super::replace_runtime`]),
//! the renderer builds it again on the next frame, and the host re-reads
//! the scene's settings for it ([`refresh_settings`]).

use super::kit;
use super::{
    MaskUse, StyleDescriptor, StyleDevice, StyleFrame, StyleHint, StyleInput, StyleNeed,
    StyleOption, StyleParam, StylePass, StyleStage,
};
use bytemuck::{Pod, Zeroable};
use flint_core::project::{shown, AssetKind, AssetScope};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, RwLock};

mod template;
pub use template::{template, Template};

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
    fn parse(s: &str) -> Option<Self> {
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
}

/// How many images a style may bind: with the input and the three
/// optional buffers it stays inside every backend's 16 sampled textures.
pub const MAX_IMAGES: usize = 8;

/// The image files a style may read (what [`crate::open_image`] decodes
/// into 8-bit colour).
const IMAGE_EXTENSIONS: &[&str] = &["png", "jpg", "jpeg"];

fn default_stage() -> String {
    "display".into()
}

/// `file` is a relative path to an image inside the style's folder.
fn check_image_file(at: &dyn std::fmt::Display, key: &str, file: &str) -> Result<(), String> {
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
fn images_beside(dir: &Path, default: &str) -> Vec<String> {
    let sub = Path::new(default).parent().unwrap_or(Path::new(""));
    let mut found: Vec<String> = flint_core::vfs::read_dir(dir.join(sub))
        .unwrap_or_default()
        .into_iter()
        .filter_map(|p| {
            let file = p.file_name()?.to_str()?.to_string();
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
    fn any(self) -> bool {
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
fn check_passes(
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
fn read_before_write(
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
        let at = toml_path.display();
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
        if super::is_builtin(&self.name) {
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
        for (name, p) in &self.params {
            if !is_identifier(name) || name.starts_with("flint") || name.starts_with('_') {
                return Err(format!(
                    "{at}: parameter \"{name}\" must be lowercase letters, digits and _, \
                     and not start with flint or _"
                ));
            }
            if super::RESERVED_KEYS.contains(&name.as_str()) {
                return Err(format!(
                    "{at}: parameter \"{name}\" is taken: every style's table reads {}",
                    super::RESERVED_KEYS.join(", ")
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
            if super::RESERVED_KEYS.contains(&name) {
                return Err(format!(
                    "{at}: {what} \"{name}\" is taken: every style's table reads {}",
                    super::RESERVED_KEYS.join(", ")
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
                    if o.srgb {
                        return Err(format!(
                            "{at}: options.{name}: srgb is for an image option (kind = \"image\")"
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
                });
            }
            options.push((
                name,
                OptionSource {
                    kind,
                    default: o.default,
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
            label: self.label.unwrap_or_else(|| super::humanize(&self.name)),
            name: self.name,
            stage,
            inputs,
            feeds_bloom: self.feeds_bloom,
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

/// How many parameters a style may declare: 64 floats is a 272-byte
/// uniform, well inside every backend's limit.
pub const MAX_PARAMS: usize = 64;

/// Settings rows a mask-reading style's uniform holds (#414): row 0 the
/// scene's, then one a per-entity variant. Variants past it are drawn
/// with the scene's settings.
pub const MAX_VARIANTS: usize = 16;

/// Mask ids the slot table covers; an entity of the style with an id past
/// it is drawn like an unstyled pixel.
pub const MAX_IDS: usize = 256;

/// The component a mask-reading user style `name` is set on entities with.
pub fn component_name(name: &str) -> String {
    format!("{name}_style")
}

fn is_identifier(name: &str) -> bool {
    let mut chars = name.chars();
    chars.next().is_some_and(|c| c.is_ascii_lowercase())
        && chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
}

// ---------------------------------------------------------------------------
// The prelude and the composed module
// ---------------------------------------------------------------------------

/// The module wgpu compiles: the generated prelude, the user's source, the
/// entry points, and where the user's lines start.
#[derive(Debug, Clone)]
pub struct Composed {
    pub text: String,
    /// The number of lines before the user's first.
    pub prelude_lines: usize,
    pub user_lines: usize,
}

impl Composed {
    /// Where composed line `line` (1-based) is in the user's file, if it is
    /// one of theirs.
    pub fn user_line(&self, line: usize) -> Option<usize> {
        let first = self.prelude_lines + 1;
        (line >= first && line < first + self.user_lines).then(|| line - self.prelude_lines)
    }
}

const PRELUDE_FRAME: &str = r#"// ---- Flint style prelude (generated; ADR 0112) ----
struct FlintFrame {
    inv_view_proj: mat4x4<f32>,
    resolution: vec2<f32>,
    texel: vec2<f32>,
    depth_size: vec2<f32>,
    time: f32,
    frame: u32,
    near: f32,
    far: f32,
    srgb: f32,
    pixels_per_point: f32,
    camera_pos: vec3<f32>,
    mask_id: u32,
    // the size in pixels of what this pass draws: its target, or the output
    pass_size: vec2<f32>,
    // 1 when last frame's history targets are this picture's past; 0 on a
    // first frame, after a cut, a resize or a frame drawn without the style
    history: f32,
    // this pass's index, 0 first (#412)
    pass_index: u32,
};

struct StylePixel {
    // 0..1 across the picture, top left at 0,0
    uv: vec2<f32>,
    // pixel position (centres at .5)
    pos: vec2<f32>,
    // the input at uv: display values at the display stage, linear HDR at hdr
    color: vec4<f32>,
};

@group(0) @binding(0) var<uniform> flint: FlintFrame;
@group(1) @binding(0) var flint_input: texture_2d<f32>;
@group(1) @binding(1) var flint_sampler: sampler;

fn flint_to_display(c: vec3<f32>) -> vec3<f32> {
    let lo = c * 12.92;
    let hi = 1.055 * pow(max(c, vec3<f32>(0.0)), vec3<f32>(1.0 / 2.4)) - 0.055;
    return select(hi, lo, c <= vec3<f32>(0.0031308));
}

fn flint_to_linear(c: vec3<f32>) -> vec3<f32> {
    let lo = c / 12.92;
    let hi = pow((max(c, vec3<f32>(0.0)) + 0.055) / 1.055, vec3<f32>(2.4));
    return select(hi, lo, c <= vec3<f32>(0.04045));
}

fn flint_luma(c: vec3<f32>) -> f32 {
    return dot(c, vec3<f32>(0.2126, 0.7152, 0.0722));
}

// A cheap, stable hash of a cell: 0..1
fn flint_hash(p: vec2<f32>) -> f32 {
    let q = fract(p * vec2<f32>(123.34, 456.21));
    let r = q + dot(q, q + 45.32);
    return fract(r.x * r.y);
}
"#;

fn prelude_sample(stage: StyleStage) -> String {
    let display = stage == StyleStage::Display;
    format!(
        r#"
// The input at `uv`, in the stage's space (display values at the display stage)
fn flint_sample(uv: vec2<f32>) -> vec4<f32> {{
    let c = textureSampleLevel(flint_input, flint_sampler, uv, 0.0);
    {}
}}
"#,
        if display {
            "let rgb = clamp(c.rgb, vec3<f32>(0.0), vec3<f32>(1.0));\n    \
             return vec4<f32>(select(rgb, flint_to_display(rgb), flint.srgb > 0.5), c.a);"
        } else {
            "return c;"
        }
    )
}

const PRELUDE_DEPTH: &str = r#"
@group(2) @binding(0) var flint_depth_texture: texture_depth_2d;

fn flint_depth_texel(uv: vec2<f32>) -> vec2<i32> {
    let dims = vec2<i32>(flint.depth_size);
    return clamp(vec2<i32>(uv * flint.depth_size), vec2<i32>(0), dims - 1);
}

// The scene's depth buffer at uv, 0 (near) .. 1 (far)
fn flint_depth(uv: vec2<f32>) -> f32 {
    return textureLoad(flint_depth_texture, flint_depth_texel(uv), 0);
}

// Distance along the view axis at uv, in metres
fn flint_view_depth(uv: vec2<f32>) -> f32 {
    let d = flint_depth(uv);
    return flint.near * flint.far / (flint.far - d * (flint.far - flint.near));
}

// The world position the pixel at uv shows
fn flint_world_pos(uv: vec2<f32>) -> vec3<f32> {
    let ndc = vec4<f32>(uv.x * 2.0 - 1.0, 1.0 - uv.y * 2.0, flint_depth(uv), 1.0);
    let w = flint.inv_view_proj * ndc;
    return w.xyz / w.w;
}
"#;

const PRELUDE_MASK: &str = r#"
@group(2) @binding(1) var flint_mask_texture: texture_2d<u32>;

fn flint_mask_texel(uv: vec2<f32>) -> vec2<i32> {
    let dims = vec2<i32>(textureDimensions(flint_mask_texture));
    return clamp(vec2<i32>(uv * vec2<f32>(dims)), vec2<i32>(0), dims - 1);
}

// The style mask id at uv (0 = no style; #263)
fn flint_mask_id(uv: vec2<f32>) -> u32 {
    return textureLoad(flint_mask_texture, flint_mask_texel(uv), 0).r;
}

// Mask id -> this style's settings row + 1; 0 for a pixel that isn't one
// of this style's entities (or one opted out)
fn flint_slot(id: u32) -> u32 {
    if (id >= 256u) {
        return 0u;
    }
    return flint_params.slots[id / 4u][id % 4u];
}

// How much of this style the entity at uv asks for: 0 where it isn't this
// style's (a `style_mask` or `<name>_style` naming it), else its blend
// 0..1 (#278)
fn flint_style_blend(uv: vec2<f32>) -> f32 {
    let m = textureLoad(flint_mask_texture, flint_mask_texel(uv), 0);
    if (flint_slot(m.r) == 0u) {
        return 0.0;
    }
    return f32(m.g) / 65535.0;
}

// The settings of the entity at uv (#414): its `<name>_style` fields, else
// the scene's table, else the defaults. `params` is this for the pixel
// being drawn; this reads a neighbour's
fn flint_params_at(uv: vec2<f32>) -> Params {
    let s = flint_slot(flint_mask_id(uv));
    return flint_params.rows[max(s, 1u) - 1u];
}
"#;

const PRELUDE_EDGES: &str = r#"
@group(2) @binding(2) var flint_edges_texture: texture_2d<f32>;

// The shared geometry edges at uv (#301): r = silhouette strength, g =
// crease strength (1 = an edge at the scene's edge_depth / edge_normal),
// ba = the unit direction across the edge, screen space with y down
fn flint_edges(uv: vec2<f32>) -> vec4<f32> {
    let dims = vec2<i32>(textureDimensions(flint_edges_texture));
    let p = clamp(vec2<i32>(uv * vec2<f32>(dims)), vec2<i32>(0), dims - 1);
    return textureLoad(flint_edges_texture, p, 0);
}

// How much of an edge is at uv: the stronger of silhouette and crease
fn flint_edge(uv: vec2<f32>) -> f32 {
    let e = flint_edges(uv);
    return max(e.r, e.g);
}
"#;

/// The group 3 bindings and helpers of the targets pass `pass` reads
/// (#412), after the style's images: `flint_target_<name>(uv)` (filtered,
/// or nearest for `r32f`), `flint_target_<name>_load(texel)` and
/// `flint_target_<name>_size()`.
fn prelude_targets(source: &StyleSource, pass: usize) -> String {
    let mut text = String::new();
    let first = source.images.len();
    for (i, read) in source.passes[pass].reads.iter().enumerate() {
        let binding = first + i;
        let t = &source.targets[read.target];
        let n = &t.name;
        let when = if read.previous {
            "last frame's (it keeps history; mind flint.history)"
        } else {
            "as this frame's earlier pass wrote it"
        };
        let sample = if t.format.filterable() {
            format!("return textureSampleLevel(flint_target_{n}_texture, flint_sampler, uv, 0.0);")
        } else {
            format!("return flint_target_{n}_load(vec2<i32>(uv * flint_target_{n}_size()));")
        };
        text.push_str(&format!(
            r#"
// Target "{n}" ({format}, {scale} of the stage), {when}
@group(3) @binding({binding}) var flint_target_{n}_texture: texture_2d<f32>;

fn flint_target_{n}_size() -> vec2<f32> {{
    return vec2<f32>(textureDimensions(flint_target_{n}_texture));
}}

fn flint_target_{n}_load(texel: vec2<i32>) -> vec4<f32> {{
    let dims = vec2<i32>(textureDimensions(flint_target_{n}_texture));
    return textureLoad(flint_target_{n}_texture, clamp(texel, vec2<i32>(0), dims - 1), 0);
}}

fn flint_target_{n}(uv: vec2<f32>) -> vec4<f32> {{
    {sample}
}}
"#,
            format = t.format.name(),
            scale = t.scale,
        ));
    }
    text
}

fn epilogue(source: &StyleSource, last: bool) -> String {
    let stage = source.stage;
    let own = if source.inputs.mask {
        "params = flint_params_at(in.uv);"
    } else {
        "params = flint_params.rows[0];"
    };
    let out = if !last {
        // A target holds what the pass returns, unchanged (#412)
        "return style(p);"
    } else if stage == StyleStage::Display {
        "let c = style(p);\n    \
         let rgb = clamp(c.rgb, vec3<f32>(0.0), vec3<f32>(1.0));\n    \
         return vec4<f32>(select(rgb, flint_to_linear(rgb), flint.srgb > 0.5), c.a);"
    } else {
        "return style(p);"
    };
    format!(
        r#"
// ---- Flint style entry points (generated) ----
struct FlintVertexOut {{
    @builtin(position) position: vec4<f32>,
    @location(0) uv: vec2<f32>,
}};

@vertex
fn vs_main(@builtin(vertex_index) vi: u32) -> FlintVertexOut {{
    let uv = vec2<f32>(f32((vi << 1u) & 2u), f32(vi & 2u));
    var out: FlintVertexOut;
    out.position = vec4<f32>(uv * 2.0 - 1.0, 0.0, 1.0);
    out.uv = vec2<f32>(uv.x, 1.0 - uv.y);
    return out;
}}

@fragment
fn fs_main(in: FlintVertexOut) -> @location(0) vec4<f32> {{
    {own}
    var p: StylePixel;
    p.uv = in.uv;
    p.pos = in.position.xy;
    p.color = flint_sample(in.uv);
    {out}
}}
"#
    )
}

/// The prelude for `source`'s last pass (its only one, for a style of one
/// pass): everything before the user's first line.
pub fn prelude(source: &StyleSource) -> String {
    prelude_pass(source, source.last_pass())
}

/// The prelude for pass `pass` of `source` (#412).
pub fn prelude_pass(source: &StyleSource, pass: usize) -> String {
    let mut text = String::from(PRELUDE_FRAME);
    text.push_str("\nstruct Params {\n");
    for (name, _) in &source.params {
        text.push_str(&format!("    {name}: f32,\n"));
    }
    // Options (#413): an index into their choices
    for (name, _) in &source.options {
        text.push_str(&format!("    {name}: u32,\n"));
    }
    // Never empty, and the fields above round up to 16 bytes before it
    text.push_str("    _flint_pad: vec4<f32>,\n};\n");
    text.push_str(&option_constants(source));
    // The settings table (#414): row 0 the scene's, then a row for each
    // per-entity variant, and which row each mask id takes
    let mask = source.inputs.mask;
    text.push_str(&format!(
        "\nstruct FlintParamTable {{\n    rows: array<Params, {}>,\n{}}};\n\
         @group(0) @binding(1) var<uniform> flint_params: FlintParamTable;\n\
         // This pixel's settings: its entity's, else the scene's\n\
         var<private> params: Params;\n",
        table_rows(mask),
        if mask {
            format!("    slots: array<vec4<u32>, {}>,\n", MAX_IDS / 4)
        } else {
            String::new()
        }
    ));
    text.push_str(&prelude_sample(source.stage));
    if source.inputs.depth {
        text.push_str(PRELUDE_DEPTH);
    }
    if source.inputs.mask {
        text.push_str(PRELUDE_MASK);
    }
    if source.inputs.edges {
        text.push_str(PRELUDE_EDGES);
    }
    for (binding, image) in source.images.iter().enumerate() {
        text.push_str(&prelude_image(binding, image));
    }
    text.push_str(&prelude_targets(source, pass));
    text.push_str("\n// ---- user style ----\n");
    text
}

/// `const FLINT_<OPTION>_<CHOICE>: u32` for each choice that is a word,
/// so a shader says `params.finish == FLINT_FINISH_GLOSS`, not `== 1u`.
fn option_constants(source: &StyleSource) -> String {
    let mut text = String::new();
    for (name, option) in &source.options {
        if option.kind != OptionKind::Choice {
            continue;
        }
        for (i, choice) in option.choices.iter().enumerate() {
            let word = choice.to_ascii_uppercase().replace('-', "_");
            if word.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') && !word.is_empty() {
                text.push_str(&format!(
                    "const FLINT_{}_{word}: u32 = {i}u;\n",
                    name.to_ascii_uppercase()
                ));
            }
        }
    }
    text
}

/// One image's binding and helpers (#413). It is read with the input's
/// sampler (group 1): a style's images add no sampler to the DX12 heap.
fn prelude_image(binding: usize, image: &ImageSource) -> String {
    let n = &image.name;
    let what = image
        .file
        .as_deref()
        .map_or_else(|| "the scene's choice".to_string(), |f| f.to_string());
    format!(
        r#"
// Image `{n}` ({what})
@group(3) @binding({binding}) var flint_image_texture_{n}: texture_2d<f32>;

// Image `{n}` at uv (0..1 across it), bilinear, clamped at its edges
fn flint_image_{n}(uv: vec2<f32>) -> vec4<f32> {{
    return textureSampleLevel(flint_image_texture_{n}, flint_sampler, uv, 0.0);
}}

// Image `{n}`'s size in texels
fn flint_image_size_{n}() -> vec2<f32> {{
    return vec2<f32>(textureDimensions(flint_image_texture_{n}));
}}

// Image `{n}` repeated: uv 1.25 is uv 0.25, bilinear across the seam
fn flint_image_tiled_{n}(uv: vec2<f32>) -> vec4<f32> {{
    let dims = vec2<i32>(textureDimensions(flint_image_texture_{n}));
    let st = uv * vec2<f32>(dims) - 0.5;
    let base = floor(st);
    let f = st - base;
    let p0 = ((vec2<i32>(base) % dims) + dims) % dims;
    let p1 = (p0 + vec2<i32>(1)) % dims;
    let a = textureLoad(flint_image_texture_{n}, p0, 0);
    let b = textureLoad(flint_image_texture_{n}, vec2<i32>(p1.x, p0.y), 0);
    let c = textureLoad(flint_image_texture_{n}, vec2<i32>(p0.x, p1.y), 0);
    let d = textureLoad(flint_image_texture_{n}, p1, 0);
    return mix(mix(a, b, f.x), mix(c, d, f.x), f.y);
}}

// Image `{n}` read as a 3D LUT laid out as a strip: N slices of N x N side
// by side (N*N wide, N high), blue choosing the slice left to right, red
// across each slice and green down it (the layout of Unreal's and most
// tools' strips). `c` and the result are 0..1, in the image's values
fn flint_lut_{n}(c: vec3<f32>) -> vec3<f32> {{
    let size = vec2<f32>(textureDimensions(flint_image_texture_{n}));
    let n = size.y;
    let rgb = clamp(c, vec3<f32>(0.0), vec3<f32>(1.0)) * (n - 1.0);
    let slice = floor(rgb.b);
    let next = min(slice + 1.0, n - 1.0);
    let x = rgb.r + 0.5;
    let y = (rgb.g + 0.5) / n;
    let lo = textureSampleLevel(flint_image_texture_{n}, flint_sampler, vec2<f32>((slice * n + x) / size.x, y), 0.0).rgb;
    let hi = textureSampleLevel(flint_image_texture_{n}, flint_sampler, vec2<f32>((next * n + x) / size.x, y), 0.0).rgb;
    return mix(lo, hi, rgb.b - slice);
}}
"#
    )
}

/// `user` (the WGSL of the style's last pass, its only one for a style of
/// one pass) inside its prelude and entry points.
pub fn compose(source: &StyleSource, user: &str) -> Composed {
    compose_pass(source, source.last_pass(), user)
}

/// `user` (the WGSL of pass `pass`) inside its prelude and entry points
/// (#412).
pub fn compose_pass(source: &StyleSource, pass: usize, user: &str) -> Composed {
    let prelude = prelude_pass(source, pass);
    let last = pass == source.last_pass();
    let prelude_lines = prelude.matches('\n').count();
    let mut user = user.to_string();
    if !user.ends_with('\n') {
        user.push('\n');
    }
    let user_lines = user.matches('\n').count();
    let text = format!("{prelude}{user}{}", epilogue(source, last));
    Composed {
        text,
        prelude_lines,
        user_lines,
    }
}

/// What the user writes when they have nothing yet: the input, unchanged.
pub const PASSTHROUGH: &str = "fn style(p: StylePixel) -> vec4<f32> {\n    return p.color;\n}\n";

/// The size in bytes of a style's `Params` uniform.
pub fn params_size(count: usize) -> usize {
    count.div_ceil(4) * 16 + 16
}

/// The settings rows a style's table holds: one (the scene's) unless it
/// reads the mask, which is what tells entities apart.
pub fn table_rows(mask: bool) -> usize {
    if mask {
        MAX_VARIANTS
    } else {
        1
    }
}

/// The size in bytes of a style's settings table (#414): its rows of
/// `Params`, then for a mask-reading style the slot of each mask id.
pub fn table_size(count: usize, mask: bool) -> usize {
    table_rows(mask) * params_size(count) + if mask { MAX_IDS * 4 } else { 0 }
}

/// The settings table's contents, as 32-bit words: `rows` (row 0 the
/// scene's) each laid out as `Params` with values in declaration order,
/// then, with `slots`, the row + 1 each mask id takes (0: not this
/// style's).
pub fn table_words(count: usize, rows: &[Vec<f32>], slots: Option<&[u32; MAX_IDS]>) -> Vec<u32> {
    let stride = params_size(count) / 4;
    let mask = slots.is_some();
    let mut words = vec![0u32; table_size(count, mask) / 4];
    for (r, row) in rows.iter().take(table_rows(mask)).enumerate() {
        for (i, v) in row.iter().take(count).enumerate() {
            words[r * stride + i] = v.to_bits();
        }
    }
    if let Some(slots) = slots {
        let at = table_rows(mask) * stride;
        words[at..at + MAX_IDS].copy_from_slice(slots);
    }
    words
}

// ---------------------------------------------------------------------------
// Checking with naga (native)
// ---------------------------------------------------------------------------

/// What a check found: warnings about a shader that compiles, and the
/// text to build.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Report {
    /// Loops in the user's code: each is guarded ([`LOOP_LIMIT`]) unless
    /// the guard is off ([`set_loop_guard`]).
    pub loops: usize,
    /// Texture reads in the user's code (the helpers' own not counted).
    pub texture_reads: usize,
    pub warnings: Vec<String>,
    /// The WGSL to give wgpu when it isn't the composed text: the module
    /// written back out by naga with every user loop guarded (#411).
    /// `None` when there is no loop to guard, the guard is off, or on the
    /// web (no naga there).
    pub guarded: Option<String>,
}

/// Texture reads a pixel may make before the check warns (a 9x9 blur is
/// 81): past it, a full-screen pass gets expensive fast.
pub const TEXTURE_READ_WARN: usize = 32;

/// The iterations one run of a user loop may make before the guard breaks
/// out of it (#411). Each loop has its own counter, reset where the loop
/// starts, so nested loops multiply: a 64x64 kernel is fine, a `loop {}`
/// stops after 4096 turns and its pixel draws whatever it reached.
pub const LOOP_LIMIT: u32 = 4096;

thread_local! {
    static LOOP_GUARD: std::cell::Cell<bool> = const { std::cell::Cell::new(true) };
}

/// Turn the loop guard off (or back on) for styles checked and built on
/// this thread from now on. It is on by default; off exists so a test can
/// show a guarded build draws what the unguarded one does.
#[doc(hidden)]
pub fn set_loop_guard(on: bool) {
    LOOP_GUARD.with(|g| g.set(on));
}

/// Parse and validate `user` as `source`'s shader, the way it will be
/// built, and guard its loops. `Err` is the message to show, lines in the
/// user's file.
pub fn check(source: &StyleSource, user: &str) -> Result<Report, String> {
    check_pass(source, source.last_pass(), user)
}

/// [`check`] for pass `pass` of `source` (#412).
#[cfg(not(target_arch = "wasm32"))]
pub fn check_pass(source: &StyleSource, pass: usize, user: &str) -> Result<Report, String> {
    let composed = compose_pass(source, pass, user);
    let file = source.passes[pass].wgsl_path.display().to_string();
    let mut module = naga::front::wgsl::parse_str(&composed.text).map_err(|e| {
        locate(
            &composed,
            &file,
            e.location(&composed.text)
                .map(|l| (l.line_number, l.line_position)),
            e.message(),
            source,
            pass,
        )
    })?;
    let mut validator = naga::valid::Validator::new(
        naga::valid::ValidationFlags::all(),
        naga::valid::Capabilities::default(),
    );
    validator.validate(&module).map_err(|e| {
        let mut message = e.as_inner().to_string();
        let mut source_err: Option<&dyn std::error::Error> =
            std::error::Error::source(e.as_inner());
        while let Some(inner) = source_err {
            message.push_str(&format!(": {inner}"));
            source_err = inner.source();
        }
        locate(
            &composed,
            &file,
            e.location(&composed.text)
                .map(|l| (l.line_number, l.line_position)),
            &message,
            source,
            pass,
        )
    })?;
    let mut report = inspect(&module, &composed, &file)?;
    if report.loops > 0 && LOOP_GUARD.with(|g| g.get()) {
        let user = user_range(&composed);
        guard_loops(&mut module, |span| in_range(&user, span));
        // The guard is ours, so a failure here is a framework bug: say so
        // rather than build the unguarded text
        let info = validator.validate(&module).map_err(|e| {
            format!("{file}: the loop guard made an invalid module (a Flint bug, #411): {e}")
        })?;
        let text =
            naga::back::wgsl::write_string(&module, &info, naga::back::wgsl::WriterFlags::empty())
                .map_err(|e| {
                    format!("{file}: writing the guarded module failed (a Flint bug, #411): {e}")
                })?;
        report.guarded = Some(text);
    } else if report.loops > 0 {
        report.warnings.push(format!(
            "{file}: warning: {} loop{} and the loop guard is off: a loop that never ends hangs \
             the GPU until the driver resets it (TDR)",
            report.loops,
            if report.loops == 1 { "" } else { "s" }
        ));
    }
    Ok(report)
}

/// The byte range of the user's code in the composed text.
#[cfg(not(target_arch = "wasm32"))]
fn user_range(composed: &Composed) -> std::ops::Range<usize> {
    let prelude_end = composed
        .text
        .match_indices('\n')
        .nth(composed.prelude_lines.saturating_sub(1))
        .map_or(0, |(i, _)| i + 1);
    let user_end = composed
        .text
        .match_indices('\n')
        .nth(composed.prelude_lines + composed.user_lines - 1)
        .map_or(composed.text.len(), |(i, _)| i + 1);
    prelude_end..user_end
}

#[cfg(not(target_arch = "wasm32"))]
fn in_range(range: &std::ops::Range<usize>, span: naga::Span) -> bool {
    span.to_range()
        .is_some_and(|r| r.start >= range.start && r.start < range.end)
}

/// Give every loop in the functions `in_user` picks a counter of its own,
/// so none runs more than [`LOOP_LIMIT`] times (#411):
///
/// ```text
/// guard = 0u;                      // before the loop: every run starts at 0
/// loop {
///     ...body...
///     continuing {
///         ...its own continuing...
///         guard = guard + 1u;
///         break if (its own break_if) || guard >= LOOP_LIMIT;
///     }
/// }
/// ```
///
/// `continue` jumps to `continuing`, so every turn counts. A loop that
/// ends on its own within the limit runs exactly as written. Returns the
/// number of loops guarded.
#[cfg(not(target_arch = "wasm32"))]
fn guard_loops(module: &mut naga::Module, in_user: impl Fn(naga::Span) -> bool) -> usize {
    let u32_ty = module.types.insert(
        naga::Type {
            name: None,
            inner: naga::TypeInner::Scalar(naga::Scalar::U32),
        },
        naga::Span::UNDEFINED,
    );
    let mut guarded = 0;
    let handles: Vec<_> = module
        .functions
        .iter()
        .filter(|(h, _)| in_user(module.functions.get_span(*h)))
        .map(|(h, _)| h)
        .collect();
    for h in handles {
        let f = &mut module.functions[h];
        let mut body = std::mem::take(&mut f.body);
        guard_block(&mut body, f, u32_ty, &mut guarded);
        f.body = body;
    }
    guarded
}

#[cfg(not(target_arch = "wasm32"))]
fn guard_block(
    block: &mut naga::Block,
    f: &mut naga::Function,
    u32_ty: naga::Handle<naga::Type>,
    count: &mut usize,
) {
    use naga::{BinaryOperator as B, Expression as E, Literal, Statement as S};
    let old = std::mem::take(block);
    for (mut statement, span) in old.span_into_iter() {
        match &mut statement {
            S::Loop {
                body,
                continuing,
                break_if,
            } => {
                guard_block(body, f, u32_ty, count);
                guard_block(continuing, f, u32_ty, count);
                let var = f.local_variables.append(
                    naga::LocalVariable {
                        name: Some(format!("flint_loop_guard_{count}")),
                        ty: u32_ty,
                        init: None,
                    },
                    span,
                );
                *count += 1;
                // Literals and the variable's pointer need no Emit
                let pointer = f.expressions.append(E::LocalVariable(var), span);
                let zero = f.expressions.append(E::Literal(Literal::U32(0)), span);
                let one = f.expressions.append(E::Literal(Literal::U32(1)), span);
                let limit = f
                    .expressions
                    .append(E::Literal(Literal::U32(LOOP_LIMIT)), span);
                block.push(
                    S::Store {
                        pointer,
                        value: zero,
                    },
                    span,
                );
                let load = f.expressions.append(E::Load { pointer }, span);
                let next = f.expressions.append(
                    E::Binary {
                        op: B::Add,
                        left: load,
                        right: one,
                    },
                    span,
                );
                continuing.push(S::Emit(naga::Range::new_from_bounds(load, next)), span);
                continuing.push(
                    S::Store {
                        pointer,
                        value: next,
                    },
                    span,
                );
                let done = f.expressions.append(
                    E::Binary {
                        op: B::GreaterEqual,
                        left: next,
                        right: limit,
                    },
                    span,
                );
                let last = match *break_if {
                    Some(own) => f.expressions.append(
                        E::Binary {
                            op: B::LogicalOr,
                            left: own,
                            right: done,
                        },
                        span,
                    ),
                    None => done,
                };
                continuing.push(S::Emit(naga::Range::new_from_bounds(done, last)), span);
                *break_if = Some(last);
            }
            S::Block(b) => guard_block(b, f, u32_ty, count),
            S::If { accept, reject, .. } => {
                guard_block(accept, f, u32_ty, count);
                guard_block(reject, f, u32_ty, count);
            }
            S::Switch { cases, .. } => {
                for case in cases {
                    guard_block(&mut case.body, f, u32_ty, count);
                }
            }
            _ => {}
        }
        block.push(statement, span);
    }
}

/// `file:line:col: message`, with the line remapped into the user's file
/// and the offending line quoted; a hint when the user called a helper of
/// an input they didn't declare.
///
/// `at` is the composed module's 1-based (line, column): naga's natively,
/// the browser compiler's on the web (#415).
fn locate(
    composed: &Composed,
    file: &str,
    at: Option<(u32, u32)>,
    message: &str,
    source: &StyleSource,
    pass: usize,
) -> String {
    let mut out = match at {
        Some((line_number, line_position)) => match composed.user_line(line_number as usize) {
            Some(line) => {
                let text = composed
                    .text
                    .lines()
                    .nth(line_number as usize - 1)
                    .unwrap_or("");
                format!(
                    "{file}:{line}:{line_position}: {message}\n    {line} | {}",
                    text.trim_end()
                )
            }
            None if message.contains("'style'") => format!(
                "{file}: no `fn style(p: StylePixel) -> vec4<f32>`; the style draws what it returns"
            ),
            None => format!(
                "{file}: {message} (in the generated code, line {line_number}; check the parameter names)"
            ),
        },
        None => format!("{file}: {message}"),
    };
    let missing = |helpers: &[&str]| helpers.iter().any(|h| message.contains(h));
    if !source.inputs.depth && missing(&["flint_depth", "flint_view_depth", "flint_world_pos"]) {
        out.push_str("\n    hint: add \"depth\" to inputs in the .shader.toml");
    }
    if !source.inputs.mask && missing(&["flint_mask_id", "flint_style_blend"]) {
        out.push_str("\n    hint: add \"mask\" to inputs in the .shader.toml");
    }
    if !source.inputs.edges && missing(&["flint_edges", "flint_edge"]) {
        out.push_str("\n    hint: add \"edges\" to inputs in the .shader.toml");
    }
    if missing(&["'flint_image_", "'flint_lut_"]) {
        let names: Vec<&str> = source.images.iter().map(|i| i.name.as_str()).collect();
        out.push_str(&format!(
            "\n    hint: an image is declared as [images.<name>] or an option with \
             kind = \"image\"; this style's images: {}",
            if names.is_empty() {
                "none".to_string()
            } else {
                names.join(", ")
            }
        ));
    }
    // A helper of a target this pass doesn't read (#412)
    if let Some(at) = message.find("flint_target_") {
        let rest = &message[at + "flint_target_".len()..];
        let name: String = rest
            .chars()
            .take_while(|c| c.is_ascii_alphanumeric() || *c == '_')
            .collect();
        let name = ["_size", "_load"]
            .iter()
            .find_map(|s| name.strip_suffix(s))
            .unwrap_or(&name);
        let reads = &source.passes[pass].reads;
        if !reads.iter().any(|r| source.targets[r.target].name == name) {
            let pass_name = &source.passes[pass].name;
            if source.targets.iter().any(|t| t.name == name) {
                out.push_str(&format!(
                    "\n    hint: add \"{name}\" to reads of pass \"{pass_name}\" in the .shader.toml"
                ));
            } else {
                out.push_str(&format!("\n    hint: no pass writes a target \"{name}\""));
            }
        }
    }
    out
}

/// The checks a parse doesn't make: the user defined `style`, declared no
/// bindings or entry points of their own, and how many loops and texture
/// reads their code has.
#[cfg(not(target_arch = "wasm32"))]
fn inspect(module: &naga::Module, composed: &Composed, file: &str) -> Result<Report, String> {
    let user = user_range(composed);
    let in_user = |span: naga::Span| in_range(&user, span);
    let line_of = |span: naga::Span| {
        let at = span.location(&composed.text);
        composed
            .user_line(at.line_number as usize)
            .map_or(String::new(), |l| format!(":{l}"))
    };

    let style = module
        .functions
        .iter()
        .find(|(_, f)| f.name.as_deref() == Some("style"));
    let Some((handle, _)) = style else {
        return Err(format!(
            "{file}: no `fn style(p: StylePixel) -> vec4<f32>`; the style draws what it returns"
        ));
    };
    let _ = handle;
    for (h, var) in module.global_variables.iter() {
        if var.binding.is_some() && in_user(module.global_variables.get_span(h)) {
            return Err(format!(
                "{file}{}: `{}` declares a binding; a style reads only the prelude's \
                 (flint_input, params, depth, mask, edges, images, and the targets in reads)",
                line_of(module.global_variables.get_span(h)),
                var.name.as_deref().unwrap_or("?")
            ));
        }
    }
    if module.entry_points.len() != 2 {
        return Err(format!(
            "{file}: declares an entry point; the framework writes vs_main and fs_main"
        ));
    }

    let mut report = Report::default();
    for (h, f) in module.functions.iter() {
        if !in_user(module.functions.get_span(h)) {
            continue;
        }
        report.loops += count_loops(&f.body);
        report.texture_reads += f
            .expressions
            .iter()
            .filter(|(_, e)| {
                matches!(
                    e,
                    naga::Expression::ImageSample { .. } | naga::Expression::ImageLoad { .. }
                )
            })
            .count();
        // Calls to the sampling helpers count as reads too
        report.texture_reads += f
            .body
            .iter()
            .map(|s| count_helper_calls(s, module))
            .sum::<usize>();
    }
    if report.texture_reads > TEXTURE_READ_WARN {
        report.warnings.push(format!(
            "{file}: warning: {} texture reads written out; each runs once a pixel",
            report.texture_reads
        ));
    }
    Ok(report)
}

#[cfg(not(target_arch = "wasm32"))]
fn count_loops(block: &naga::Block) -> usize {
    use naga::Statement as S;
    block
        .iter()
        .map(|s| match s {
            S::Loop {
                body, continuing, ..
            } => 1 + count_loops(body) + count_loops(continuing),
            S::Block(b) => count_loops(b),
            S::If { accept, reject, .. } => count_loops(accept) + count_loops(reject),
            S::Switch { cases, .. } => cases.iter().map(|c| count_loops(&c.body)).sum(),
            _ => 0,
        })
        .sum()
}

#[cfg(not(target_arch = "wasm32"))]
fn count_helper_calls(statement: &naga::Statement, module: &naga::Module) -> usize {
    use naga::Statement as S;
    let block = |b: &naga::Block| {
        b.iter()
            .map(|s| count_helper_calls(s, module))
            .sum::<usize>()
    };
    match statement {
        S::Call { function, .. } => {
            let name = module.functions[*function].name.as_deref().unwrap_or("");
            if name.starts_with("flint_image_size_")
                || (name.starts_with("flint_target_") && name.ends_with("_size"))
            {
                return 0;
            }
            // A tiled read is four loads, a LUT two samples
            if name.starts_with("flint_image_tiled_") {
                return 4;
            }
            if name.starts_with("flint_lut_") {
                return 2;
            }
            usize::from(
                name.starts_with("flint_image_")
                    || name.starts_with("flint_target_")
                    || matches!(
                        name,
                        "flint_sample"
                            | "flint_depth"
                            | "flint_view_depth"
                            | "flint_world_pos"
                            | "flint_mask_id"
                            | "flint_style_blend"
                            | "flint_params_at"
                            | "flint_edges"
                            | "flint_edge"
                    ),
            )
        }
        S::Loop {
            body, continuing, ..
        } => block(body) + block(continuing),
        S::Block(b) => block(b),
        S::If { accept, reject, .. } => block(accept) + block(reject),
        S::Switch { cases, .. } => cases.iter().map(|c| block(&c.body)).sum(),
        _ => 0,
    }
}

/// On the web there is no naga in the player: wgpu builds naga's WGSL front
/// end for wasm, but only its `webgl` backend calls it, so the linker drops
/// it and checking here would add it to the download (#415). The browser's
/// own WGSL compiler reports instead, a frame or two later
/// ([`compiler_error`], read when the build settles).
#[cfg(target_arch = "wasm32")]
pub fn check_pass(_source: &StyleSource, _pass: usize, _user: &str) -> Result<Report, String> {
    Ok(Report::default())
}

/// The first error in a compiler's report on `composed` (the browser's
/// `getCompilationInfo()` on the web), as `file:line:col: message` with the
/// line in the user's file, as [`check`] gives natively (#415). `None`
/// when it reported none.
pub fn compiler_error(
    source: &StyleSource,
    composed: &Composed,
    info: &wgpu::CompilationInfo,
) -> Option<String> {
    compiler_error_pass(source, source.last_pass(), composed, info)
}

/// [`compiler_error`] for pass `pass` of `source` (#412).
pub fn compiler_error_pass(
    source: &StyleSource,
    pass: usize,
    composed: &Composed,
    info: &wgpu::CompilationInfo,
) -> Option<String> {
    let errors: Vec<_> = info
        .messages
        .iter()
        .filter(|m| m.message_type == wgpu::CompilationMessageType::Error)
        .collect();
    let first = errors.first()?;
    let file = source.passes[pass].wgsl_path.display().to_string();
    let at = first
        .location
        .map(|l| (l.line_number, l.line_position))
        .filter(|(line, _)| *line > 0);
    let mut out = locate(composed, &file, at, first.message.trim(), source, pass);
    if errors.len() > 1 {
        out.push_str(&format!("\n    (and {} more)", errors.len() - 1));
    }
    Some(out)
}

// ---------------------------------------------------------------------------
// Loading: styles/*.shader.toml
// ---------------------------------------------------------------------------

struct Loaded {
    source: StyleSource,
    /// Bind cache names, leaked once with the descriptor.
    names: BindNames,
    /// The last error (load, check or build), for hosts to show.
    error: Mutex<Option<String>>,
    /// The images' error (#413): a file missing or that doesn't decode.
    /// Kept apart from the shader's, which a good build clears.
    image_error: Mutex<Option<String>>,
    /// The `.shader.toml`'s modified time when last read
    /// ([`reload_changed`]).
    #[cfg(not(target_arch = "wasm32"))]
    toml_modified: Mutex<Option<std::time::SystemTime>>,
}

/// Names leaked once a style's install: bind cache keys, pass labels and
/// target names.
#[derive(Clone)]
struct BindNames {
    input: &'static str,
    extra: &'static str,
    passes: Vec<PassNames>,
    /// Each target's texture names (#412): two for history (ping-ponged),
    /// else the same one twice.
    targets: Vec<[&'static str; 2]>,
}

#[derive(Clone, Copy)]
struct PassNames {
    frame: &'static str,
    /// Group 3 of a pass that reads targets: the images, then its targets.
    targets: &'static str,
    /// The render pass label (`--gpu-time` reads the style's own).
    pass: &'static str,
}

/// One message from the passes' errors, or `None` when every pass builds.
fn join_errors(errors: &[Option<String>]) -> Option<String> {
    let all: Vec<&str> = errors.iter().flatten().map(String::as_str).collect();
    (!all.is_empty()).then(|| all.join("\n"))
}

static LOADED: RwLock<Vec<Arc<Loaded>>> = RwLock::new(Vec::new());

fn loaded(name: &str) -> Option<Arc<Loaded>> {
    LOADED
        .read()
        .unwrap_or_else(|e| e.into_inner())
        .iter()
        .find(|l| l.source.name == name)
        .cloned()
}

/// The folders a scene finds styles in, nearest first: `styles/` in the
/// scene's directory, in each directory above it up to the project root,
/// then in the mounted engine (ADR 0114; [`AssetKind::Styles`]).
pub fn style_dirs(scope: &AssetScope) -> Vec<PathBuf> {
    scope.search_dirs(AssetKind::Styles)
}

/// [`load_for`] for a host that kept only the scene's directory: the
/// project is found from it.
pub fn load_dir(scene_dir: &Path) -> Vec<String> {
    load_for(&AssetScope::for_dir(scene_dir))
}

/// Register every user style a scene can see: each `*.shader.toml` the
/// project's one search lists from `scope`
/// ([`Project::list`](flint_core::project::Project::list)). Call it before
/// the scene's `[post_process]` is read, or its tables name an unknown
/// style. Returns one message for each file that couldn't be loaded (and
/// each warning a check raised); a style whose shader fails still
/// registers, drawing a passthrough.
///
/// Which of two styles of one name wins (#426): the nearest to the scene,
/// as for every asset. In one `styles/` folder a style is its file's name,
/// so there is one of a name. The registry is one for the process, so a
/// style this scene sees in a nearer file than the one loaded (for an
/// earlier scene, from another folder) takes its place at the same
/// registry index, as a hot reload does. Styles only an earlier scene's
/// folders had stay registered.
pub fn load_for(scope: &AssetScope) -> Vec<String> {
    let mut messages = Vec::new();
    for listed in scope.list(AssetKind::Styles) {
        if let Err(e) = load_file(&listed.path, &mut messages) {
            messages.push(e);
        }
    }
    messages
}

/// Register the style `path` describes (see [`load_for`]): new, or in
/// place of the style of its name loaded from another file.
pub fn load_file(path: &Path, messages: &mut Vec<String>) -> Result<(), String> {
    // Loaded from this very file: a changed one is the reload's to read
    if loaded_from(path).is_some() {
        return Ok(());
    }
    let source = read_source(path)?;
    match loaded(&source.name) {
        Some(existing) => {
            tracing::info!(
                "style '{}': {} takes over from {} (the nearest styles/ to the scene wins)",
                source.name,
                shown(path),
                shown(&existing.source.toml_path)
            );
            install(source, messages, true);
        }
        None => install(source, messages, false),
    }
    Ok(())
}

/// The loaded style whose description is the file at `path`.
fn loaded_from(path: &Path) -> Option<Arc<Loaded>> {
    LOADED
        .read()
        .unwrap_or_else(|e| e.into_inner())
        .iter()
        .find(|l| flint_core::project::same_path(&l.source.toml_path, path))
        .cloned()
}

/// Read and check the description at `path` (not its shader).
pub fn read_source(path: &Path) -> Result<StyleSource, String> {
    let text =
        flint_core::vfs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
    parse_source(&text, path)
}

/// Check `text` as the description at `path` (which needn't exist yet).
pub fn parse_source(text: &str, path: &Path) -> Result<StyleSource, String> {
    let def: ShaderStyleDef =
        toml::from_str(text).map_err(|e| format!("{}: {e}", path.display()))?;
    def.check(path)
}

/// Check the style at `path` the way a host loads it, its description and
/// then its shader, without a GPU and without registering it (#416:
/// `flint style check`). `Err` is the message a host would show.
#[cfg(not(target_arch = "wasm32"))]
///
/// Every pass is checked (#412): the errors of all that fail, one after
/// another, or one report adding up the passes' loops, reads and warnings
/// (the description's notes among them). `guarded` is `None`: there's no
/// one module to build.
pub fn check_file(path: &Path) -> Result<Report, String> {
    let source = read_source(path)?;
    let mut total = Report {
        warnings: source.notes.clone(),
        ..Report::default()
    };
    let mut errors = Vec::new();
    for (i, pass) in source.passes.iter().enumerate() {
        let user = match flint_core::vfs::read_to_string(&pass.wgsl_path) {
            Ok(user) => user,
            Err(e) => {
                errors.push(format!("{}: {e}", pass.wgsl_path.display()));
                continue;
            }
        };
        match check_pass(&source, i, &user) {
            Ok(report) => {
                total.loops += report.loops;
                total.texture_reads += report.texture_reads;
                total.warnings.extend(report.warnings);
            }
            Err(e) => errors.push(e),
        }
    }
    if errors.is_empty() {
        if source.passes.len() == 1 {
            // One pass: its own report, guarded text and all
            let user = flint_core::vfs::read_to_string(&source.wgsl_path)
                .map_err(|e| format!("{}: {e}", source.wgsl_path.display()))?;
            let mut report = check(&source, &user)?;
            report.warnings.splice(0..0, source.notes.iter().cloned());
            return Ok(report);
        }
        Ok(total)
    } else {
        Err(errors.join("\n"))
    }
}

/// The modified time of `path` on disk, for the reload polls (native
/// only: hot reload reads the disk, never the vfs, and a bundle never
/// changes). Through `flint_core::file_watch`, which in the player never
/// waits on the disk (#633): the stamp taken at a load and the one a poll
/// compares it with must come from the same place.
#[cfg(not(target_arch = "wasm32"))]
fn modified(path: &Path) -> Option<std::time::SystemTime> {
    flint_core::file_watch::modified(path)
}

/// Whether user WGSL calls `flint_style_blend` outside a line comment.
fn calls_style_blend(user: &str) -> bool {
    user.lines().any(|line| {
        let code = line.split("//").next().unwrap_or("");
        code.contains("flint_style_blend")
    })
}

/// Make `source`'s descriptor and register it: appended, or with
/// `replace`, put where the style of the same name stands (#275).
fn install(source: StyleSource, messages: &mut Vec<String>, replace: bool) {
    // A shader that fails registers anyway: it draws a passthrough and its
    // error waits for the host, so a typo never takes a style's settings away
    messages.extend(source.notes.iter().cloned());
    let wgsl: Vec<_> = source
        .passes
        .iter()
        .map(|p| flint_core::vfs::read_to_string(&p.wgsl_path))
        .collect();
    // A shader that reads its own blend draws it (#396): the framework's
    // cross-fade would apply it twice. Decided when the description is
    // read, so a shader that starts or stops calling it takes effect at the
    // next load or `.shader.toml` edit. Any pass of several may (#412)
    let blends_itself = wgsl
        .iter()
        .any(|w| w.as_ref().is_ok_and(|user| calls_style_blend(user)));
    let pass_errors: Vec<Option<String>> = wgsl
        .into_iter()
        .enumerate()
        .map(|(i, w)| match w {
            Err(e) => Some(format!("{}: {e}", source.passes[i].wgsl_path.display())),
            Ok(user) => match check_pass(&source, i, &user) {
                Ok(report) => {
                    messages.extend(report.warnings);
                    None
                }
                Err(e) => Some(e),
            },
        })
        .collect();
    let error = join_errors(&pass_errors);
    messages.extend(pass_errors.into_iter().flatten());
    // Every image it may bind is there to read (a bundle that didn't pack
    // one is refused, #413); the pass decodes them when it first draws
    let mut missing_images = Vec::new();
    for image in &source.images {
        let files: Vec<&str> = match &image.file {
            Some(file) => vec![file.as_str()],
            None => source
                .options
                .iter()
                .find(|(n, _)| *n == image.name)
                .map(|(_, o)| o.choices.iter().map(String::as_str).collect())
                .unwrap_or_default(),
        };
        for file in files {
            let path = source.dir().join(file);
            if !flint_core::vfs::is_file(&path) {
                missing_images.push(format!(
                    "{}: image not found (style \"{}\", image {})",
                    path.display(),
                    source.name,
                    image.name
                ));
            }
        }
    }
    messages.extend(missing_images.iter().cloned());
    let image_error = (!missing_images.is_empty()).then(|| missing_images.join("\n"));
    let leak = |s: String| -> &'static str { Box::leak(s.into_boxed_str()) };
    let params: Vec<StyleParam> = source
        .params
        .iter()
        .map(|(name, p)| StyleParam::new(leak(name.clone()), p.default, p.min..=p.max))
        .collect();
    let options: Vec<StyleOption> = source
        .options
        .iter()
        .map(|(name, o)| {
            let choices: Vec<&'static str> = o.choices.iter().map(|c| leak(c.clone())).collect();
            // A fixed list (an image option's found once, here): closed, so
            // a scene naming another value is warned at load (#318)
            StyleOption::new(
                leak(name.clone()),
                leak(o.default.clone()),
                Box::leak(choices.into_boxed_slice()),
            )
        })
        .collect();
    let mut hints: Vec<StyleHint> = source
        .params
        .iter()
        .zip(&params)
        .filter(|((_, p), _)| p.label.is_some() || p.help.is_some() || p.integer)
        .map(|((name, p), sp)| {
            let hint = StyleHint::new(
                sp.name,
                leak(p.label.clone().unwrap_or_else(|| super::humanize(name))),
                leak(p.help.clone().unwrap_or_default()),
            );
            if p.integer {
                hint.integer()
            } else {
                hint
            }
        })
        .collect();
    hints.extend(
        source
            .options
            .iter()
            .zip(&options)
            .filter(|((_, o), _)| o.label.is_some() || o.help.is_some())
            .map(|((name, o), so)| {
                StyleHint::new(
                    so.name,
                    leak(o.label.clone().unwrap_or_else(|| super::humanize(name))),
                    leak(o.help.clone().unwrap_or_default()),
                )
            }),
    );
    let name = leak(source.name.clone());
    let mut built = StyleDescriptor::new(name, leak(source.label.clone()), source.stage, create);
    if let Some(component) = &source.component {
        built = built.component(leak(component.clone()));
    }
    let built = built
        .params(Box::leak(params.into_boxed_slice()))
        .options(Box::leak(options.into_boxed_slice()))
        .uses_mask(if source.inputs.mask {
            MaskUse::Always
        } else {
            MaskUse::Never
        })
        .needs(if source.inputs.edges {
            &[StyleNeed::Edges]
        } else {
            &[]
        })
        .hints(Box::leak(hints.into_boxed_slice()))
        .feeds_bloom(source.feeds_bloom);
    let descriptor: &'static StyleDescriptor = Box::leak(Box::new(if blends_itself {
        built.blends_itself()
    } else {
        built
    }));
    // The chain's bind cache keys on these names, so each install gets its
    // own: a replaced style (#275) builds new buffers and layouts, and a
    // cached group under the old name would bind the old pass's buffers
    // (frozen at the old values, or the wrong size for a new param count)
    static GENERATION: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let generation = GENERATION.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let several = source.passes.len() > 1;
    let names = BindNames {
        input: leak(format!("{name} style input BG #{generation}")),
        extra: leak(format!("{name} style depth/mask BG #{generation}")),
        passes: source
            .passes
            .iter()
            .map(|p| PassNames {
                frame: leak(if several {
                    format!("{name} style {} frame BG #{generation}", p.name)
                } else {
                    format!("{name} style frame BG #{generation}")
                }),
                targets: leak(format!("{name} style {} targets BG #{generation}", p.name)),
                pass: leak(if several {
                    format!("{name} style {}", p.name)
                } else {
                    format!("{name} style pass")
                }),
            })
            .collect(),
        // Not by generation: a target is keyed by name and format, and one
        // asked for again at the same size is the same texture (#412)
        targets: source
            .targets
            .iter()
            .map(|t| {
                let base = format!("{name}.{}", t.name);
                if t.history {
                    [leak(format!("{base}.a")), leak(format!("{base}.b"))]
                } else {
                    let one = leak(base);
                    [one, one]
                }
            })
            .collect(),
    };
    let loaded = Arc::new(Loaded {
        #[cfg(not(target_arch = "wasm32"))]
        toml_modified: Mutex::new(modified(&source.toml_path)),
        source,
        names,
        error: Mutex::new(error),
        image_error: Mutex::new(image_error),
    });
    let mut all = LOADED.write().unwrap_or_else(|e| e.into_inner());
    match all
        .iter_mut()
        .find(|l| replace && l.source.name == loaded.source.name)
    {
        Some(slot) => {
            *slot = loaded;
            drop(all);
            super::replace_runtime(descriptor);
        }
        None => {
            all.push(loaded);
            drop(all);
            super::register_runtime(descriptor);
        }
    }
}

/// What [`reload_changed`] did.
#[derive(Debug, Default, Clone, PartialEq)]
pub struct Reload {
    /// Styles whose `.shader.toml` was read again and replaced (new
    /// parameters, a new stage, another shader). A host re-reads its
    /// scene's `[post_process.styles.<name>]` for each, since the tables
    /// were checked against the old parameters.
    pub replaced: Vec<String>,
    /// Styles found in a style folder for the first time.
    pub added: Vec<String>,
    /// Load errors and check warnings, one line each.
    pub messages: Vec<String>,
}

impl Reload {
    /// Whether the registry changed.
    pub fn changed(&self) -> bool {
        !self.replaced.is_empty() || !self.added.is_empty()
    }
}

/// [`reload_changed_for`] for a host that kept only the scene's directory.
/// The project is found from it each call: a host that polls holds an
/// [`AssetScope`] and calls [`reload_changed_for`].
pub fn reload_changed(scene_dir: &Path) -> Reload {
    #[cfg(not(target_arch = "wasm32"))]
    return reload_changed_for(&AssetScope::for_dir(scene_dir));
    #[cfg(target_arch = "wasm32")]
    {
        let _ = scene_dir;
        Reload::default()
    }
}

/// Hot reload of the descriptions (#275), natively: read again every
/// loaded `.shader.toml` whose modified time moved, and load any new one
/// in the style folders `scope` searches ([`style_dirs`]), by the same
/// rule a load follows: a file that appears nearer to the scene than the
/// style of its name that is loaded takes over, and when it goes the next
/// one out is loaded again. A description that no longer reads
/// keeps the style as it was and shows the error ([`errors`]); one that
/// does replaces the style at its registry index, so the renderer builds
/// it again on the next frame. The WGSL itself is polled by the pass, so
/// a shader edit needs nothing from the host.
///
/// Costs one `stat` a loaded style and one directory listing a style
/// folder: call it a few times a second, not every frame. On the web it
/// does nothing.
pub fn reload_changed_for(scope: &AssetScope) -> Reload {
    let mut reload = Reload::default();
    #[cfg(not(target_arch = "wasm32"))]
    {
        let current: Vec<Arc<Loaded>> = LOADED.read().unwrap_or_else(|e| e.into_inner()).clone();
        for l in current {
            let now = modified(&l.source.toml_path);
            if now.is_none() || now == *l.toml_modified.lock().unwrap_or_else(|e| e.into_inner()) {
                continue;
            }
            match read_source(&l.source.toml_path) {
                Ok(source) => {
                    let name = source.name.clone();
                    install(source, &mut reload.messages, true);
                    tracing::info!("style '{name}': {} reloaded", l.source.toml_path.display());
                    reload.replaced.push(name);
                }
                Err(e) => {
                    // Keep drawing what was there; don't read it again until
                    // it changes once more
                    reload.messages.push(e.clone());
                    *l.error.lock().unwrap_or_else(|e| e.into_inner()) = Some(e);
                    *l.toml_modified.lock().unwrap_or_else(|e| e.into_inner()) = now;
                }
            }
        }
        let before: Vec<(String, PathBuf)> = LOADED
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .iter()
            .map(|l| (l.source.name.clone(), l.source.toml_path.clone()))
            .collect();
        // New files; one that failed is tried again only once it changes,
        // so a broken new file reports once, not four times a second
        static REFUSED: Mutex<BTreeMap<PathBuf, Option<std::time::SystemTime>>> =
            Mutex::new(BTreeMap::new());
        // The folders in the search's order, the nearest file of a name the
        // one that counts: `Project::list`'s rule, read through
        // `file_watch` so the poll never waits on the disk (#633)
        let mut seen = std::collections::HashSet::new();
        for dir in style_dirs(scope) {
            let Some(mut entries) = flint_core::file_watch::list(&dir) else {
                continue;
            };
            entries.sort();
            for (path, _) in entries {
                let Some(name) = AssetKind::Styles.info().name_of(&path) else {
                    continue;
                };
                if !seen.insert(name) || loaded_from(&path).is_some() {
                    continue;
                }
                let now = modified(&path);
                let mut refused = REFUSED.lock().unwrap_or_else(|e| e.into_inner());
                if refused.get(&path) == Some(&now) {
                    continue;
                }
                match load_file(&path, &mut reload.messages) {
                    Ok(()) => {
                        refused.remove(&path);
                    }
                    Err(e) => {
                        reload.messages.push(e);
                        refused.insert(path, now);
                    }
                }
            }
        }
        for l in LOADED.read().unwrap_or_else(|e| e.into_inner()).iter() {
            let name = &l.source.name;
            match before.iter().find(|(n, _)| n == name) {
                None => reload.added.push(name.clone()),
                // Another file took the name over: its parameters may differ
                Some((_, path)) if *path != l.source.toml_path => {
                    if !reload.replaced.contains(name) {
                        reload.replaced.push(name.clone());
                    }
                }
                Some(_) => {}
            }
        }
    }
    #[cfg(target_arch = "wasm32")]
    let _ = scope;
    reload
}

/// Re-read the settings of each style `reload` touched from `authored`
/// (the scene's `[post_process]`, parsed again now that the registry
/// knows their new parameters) into `config`, leaving every other style as
/// the host has it (#275).
pub fn refresh_settings(
    config: &mut crate::postprocess::PostProcessConfig,
    authored: &crate::postprocess::PostProcessConfig,
    reload: &Reload,
) {
    for name in reload.replaced.iter().chain(&reload.added) {
        config.styles.restore(name, &authored.styles);
    }
}

/// Every loaded user style with its current error, for a host to show
/// (a console line, an overlay). Empty when all of them compile.
pub fn errors() -> Vec<(String, String)> {
    LOADED
        .read()
        .unwrap_or_else(|e| e.into_inner())
        .iter()
        .filter_map(|l| {
            let shader = l.error.lock().unwrap_or_else(|e| e.into_inner()).clone();
            let images = l
                .image_error
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .clone();
            let error = match (shader, images) {
                (Some(s), Some(i)) => format!("{s}\n{i}"),
                (s, i) => s.or(i)?,
            };
            Some((l.source.name.clone(), error))
        })
        .collect()
}

/// Record that style `name`'s pass panicked while it was built (the
/// renderer caught it): it stays off, and its error says so.
pub fn report_build_panic(name: &str) {
    if let Some(l) = loaded(name) {
        let e = format!(
            "{}: building the style panicked; it stays off until its files change",
            l.source.toml_path.display()
        );
        tracing::warn!("style '{name}': {e}");
        *l.error.lock().unwrap_or_else(|e| e.into_inner()) = Some(e);
    }
}

/// The source a loaded user style was read from.
pub fn source(name: &str) -> Option<StyleSource> {
    loaded(name).map(|l| l.source.clone())
}

// ---------------------------------------------------------------------------
// The pass
// ---------------------------------------------------------------------------

#[repr(C)]
#[derive(Copy, Clone, Pod, Zeroable)]
struct FlintFrameUniforms {
    inv_view_proj: [[f32; 4]; 4],
    resolution: [f32; 2],
    texel: [f32; 2],
    depth_size: [f32; 2],
    time: f32,
    frame: u32,
    near: f32,
    far: f32,
    srgb: f32,
    pixels_per_point: f32,
    camera_pos: [f32; 3],
    mask_id: u32,
    pass_size: [f32; 2],
    history: f32,
    pass_index: u32,
}

struct Build {
    /// The user's pipeline, or `None` while it has never compiled.
    pipeline: Option<wgpu::RenderPipeline>,
    /// A build waiting for the compiler's verdict before it draws (#415).
    pending: Option<Pending>,
    /// The WGSL file's modified time when last read.
    #[cfg(not(target_arch = "wasm32"))]
    modified: Option<std::time::SystemTime>,
    /// Frames since the file was last looked at.
    #[cfg(not(target_arch = "wasm32"))]
    since_poll: u32,
}

/// A pipeline made but not yet trusted: it draws once both of the GPU's
/// answers say it compiled (#415). Natively both are ready as soon as it
/// is made; on the web they are browser promises, settled a frame or two
/// later, and polled each frame until then (an invalid pipeline in a pass
/// would lose the whole frame, so the last good build draws meanwhile).
struct Pending {
    pipeline: wgpu::RenderPipeline,
    composed: Composed,
    /// The shader compiler's messages, with composed-module lines.
    info: Settling<wgpu::CompilationInfo>,
    /// What the validation error scope caught (the shader, or the
    /// pipeline against its layout).
    scope: Settling<Option<wgpu::Error>>,
}

/// A future polled by hand once a frame, never awaited: nothing in a frame
/// may block on the browser.
enum Settling<T> {
    Waiting(std::pin::Pin<Box<dyn std::future::Future<Output = T> + Send>>),
    Done(T),
}

impl<T> Settling<T> {
    fn new(future: impl std::future::Future<Output = T> + Send + 'static) -> Self {
        Settling::Waiting(Box::pin(future))
    }

    /// Poll once if still waiting; whether it has settled.
    fn poll(&mut self) -> bool {
        if let Settling::Waiting(future) = self {
            let mut cx = std::task::Context::from_waker(std::task::Waker::noop());
            match future.as_mut().poll(&mut cx) {
                std::task::Poll::Ready(value) => *self = Settling::Done(value),
                std::task::Poll::Pending => return false,
            }
        }
        true
    }

    fn take(self) -> Option<T> {
        match self {
            Settling::Done(value) => Some(value),
            Settling::Waiting(_) => None,
        }
    }
}

/// One pass of a user style on the GPU (#412).
struct PassGpu {
    /// What it writes: its target's format, or the stage's.
    format: wgpu::TextureFormat,
    /// Group 3 of a pass that reads targets: the style's images, then the
    /// targets it reads. A pass that reads none binds the images' own group.
    targets_bgl: Option<wgpu::BindGroupLayout>,
    layout: wgpu::PipelineLayout,
    /// Its own frame uniform: `pass_size` and `pass_index` differ by pass,
    /// and a buffer holds one value a frame.
    frame_buffer: wgpu::Buffer,
    build: Mutex<Build>,
    error: Mutex<Option<String>>,
}

/// What the last frame drew, for history targets (#277's rule): trusted
/// only on the frame after, in the same epoch, into the same textures.
#[derive(Default)]
struct History {
    /// Which of each history target's two textures this frame writes.
    parity: usize,
    last: Option<LastFrame>,
}

struct LastFrame {
    frame: u64,
    epoch: u64,
    /// Every target texture it used; held, so an address can't be reused.
    textures: Vec<Arc<super::StyleTexture>>,
}

struct CustomPass {
    loaded: Arc<Loaded>,
    frame_bgl: wgpu::BindGroupLayout,
    input_bgl: wgpu::BindGroupLayout,
    extra_bgl: Option<wgpu::BindGroupLayout>,
    /// The settings table, shared by every pass.
    params_buffer: wgpu::Buffer,
    /// The passes in order (one for a style with a top-level `shader`).
    passes: Vec<PassGpu>,
    /// Draws the input unchanged, while a pass can't.
    passthrough: wgpu::RenderPipeline,
    /// The style's images (#413), group 3.
    images: Option<Mutex<ImageSet>>,
    /// Group 2 when a style binds images or reads targets but reads none of
    /// the optional buffers: an empty group keeps group 3 at 3.
    empty: Option<wgpu::BindGroup>,
    /// The history targets' parity and what the last frame drew (#412).
    history: Mutex<History>,
    /// Whether the variant overflow has been warned about.
    warned: std::sync::atomic::AtomicBool,
}

fn create(d: &StyleDevice) -> Box<dyn StylePass> {
    Box::new(build(d))
}

fn build(d: &StyleDevice) -> CustomPass {
    let loaded = loaded(d.descriptor.name).expect("a user style is loaded before it is built");
    let device = d.device;
    let source = &loaded.source;
    let frame_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("User Style Frame BGL"),
        entries: &[uniform_entry(0), uniform_entry(1)],
    });
    let input_bgl = kit::sampled_layout(device, "User Style Input BGL");
    let images = ImageSet::new(device, source);
    let reads_targets = source.passes.iter().any(|p| !p.reads.is_empty());
    let extra_bgl = (source.inputs.any() || images.is_some() || reads_targets).then(|| {
        let mut entries = Vec::new();
        if source.inputs.depth {
            entries.push(texture_entry(0, wgpu::TextureSampleType::Depth));
        }
        if source.inputs.mask {
            entries.push(texture_entry(1, wgpu::TextureSampleType::Uint));
        }
        if source.inputs.edges {
            entries.push(texture_entry(
                2,
                wgpu::TextureSampleType::Float { filterable: true },
            ));
        }
        device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("User Style Depth/Mask BGL"),
            entries: &entries,
        })
    });
    let empty = extra_bgl
        .as_ref()
        .filter(|_| !source.inputs.any())
        .map(|bgl| {
            device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("User Style Empty BG"),
                layout: bgl,
                entries: &[],
            })
        });
    let passes: Vec<PassGpu> = source
        .passes
        .iter()
        .map(|p| {
            // The images first (bindings 0..n, as #413 laid them out), then
            // the targets this pass reads
            let targets_bgl = (!p.reads.is_empty()).then(|| {
                let image_entries = (0..source.images.len()).map(|i| {
                    texture_entry(
                        i as u32,
                        wgpu::TextureSampleType::Float { filterable: true },
                    )
                });
                let target_entries = p.reads.iter().enumerate().map(|(b, r)| {
                    texture_entry(
                        (source.images.len() + b) as u32,
                        wgpu::TextureSampleType::Float {
                            filterable: source.targets[r.target].format.filterable(),
                        },
                    )
                });
                let entries: Vec<wgpu::BindGroupLayoutEntry> =
                    image_entries.chain(target_entries).collect();
                device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                    label: Some("User Style Images + Targets BGL"),
                    entries: &entries,
                })
            });
            let mut layouts = vec![&frame_bgl, &input_bgl];
            let group3 = targets_bgl.as_ref().or(images.as_ref().map(|i| &i.layout));
            if let Some(extra) = &extra_bgl {
                if source.inputs.any() || group3.is_some() {
                    layouts.push(extra);
                }
            }
            if let Some(g) = group3 {
                layouts.push(g);
            }
            let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some("User Style Layout"),
                bind_group_layouts: &layouts,
                push_constant_ranges: &[],
            });
            PassGpu {
                format: p
                    .target
                    .map_or(d.format, |t| source.targets[t].format.wgpu()),
                targets_bgl,
                layout,
                frame_buffer: kit::uniform_buffer::<FlintFrameUniforms>(device, "User Style Frame"),
                build: Mutex::new(Build {
                    pipeline: None,
                    pending: None,
                    #[cfg(not(target_arch = "wasm32"))]
                    modified: None,
                    #[cfg(not(target_arch = "wasm32"))]
                    since_poll: 0,
                }),
                error: Mutex::new(None),
            }
        })
        .collect();
    // The passthrough is the last pass's prelude with no inputs, images or
    // targets: always compiles
    let last = source.passes[source.last_pass()].clone();
    let bare = StyleSource {
        inputs: Inputs::default(),
        images: Vec::new(),
        passes: vec![PassSource {
            reads: Vec::new(),
            target: None,
            ..last
        }],
        targets: Vec::new(),
        ..source.clone()
    };
    let passthrough_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some("User Style Passthrough Layout"),
        bind_group_layouts: &[&frame_bgl, &input_bgl],
        push_constant_ranges: &[],
    });
    let (_, passthrough) = pipeline(
        device,
        &passthrough_layout,
        &compose(&bare, PASSTHROUGH).text,
        d.format,
    );
    let pass = CustomPass {
        params_buffer: device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("User Style Params"),
            size: table_size(source.uniform_fields(), source.inputs.mask) as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        }),
        frame_bgl,
        input_bgl,
        extra_bgl,
        passes,
        passthrough,
        images: images.map(Mutex::new),
        empty,
        history: Mutex::new(History::default()),
        warned: Default::default(),
        loaded,
    };
    for i in 0..pass.passes.len() {
        pass.rebuild(device, i);
    }
    pass
}

fn uniform_entry(binding: u32) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility: wgpu::ShaderStages::FRAGMENT,
        ty: wgpu::BindingType::Buffer {
            ty: wgpu::BufferBindingType::Uniform,
            has_dynamic_offset: false,
            min_binding_size: None,
        },
        count: None,
    }
}

fn texture_entry(binding: u32, sample_type: wgpu::TextureSampleType) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility: wgpu::ShaderStages::FRAGMENT,
        ty: wgpu::BindingType::Texture {
            sample_type,
            view_dimension: wgpu::TextureViewDimension::D2,
            multisampled: false,
        },
        count: None,
    }
}

fn pipeline(
    device: &wgpu::Device,
    layout: &wgpu::PipelineLayout,
    text: &str,
    format: wgpu::TextureFormat,
) -> (wgpu::ShaderModule, wgpu::RenderPipeline) {
    let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("User Style Shader"),
        source: wgpu::ShaderSource::Wgsl(text.into()),
    });
    let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("User Style Pipeline"),
        layout: Some(layout),
        vertex: wgpu::VertexState {
            module: &module,
            entry_point: Some("vs_main"),
            buffers: &[],
            compilation_options: Default::default(),
        },
        fragment: Some(wgpu::FragmentState {
            module: &module,
            entry_point: Some("fs_main"),
            targets: &[Some(wgpu::ColorTargetState {
                format,
                blend: None,
                write_mask: wgpu::ColorWrites::ALL,
            })],
            compilation_options: Default::default(),
        }),
        primitive: wgpu::PrimitiveState::default(),
        depth_stencil: None,
        multisample: wgpu::MultisampleState::default(),
        multiview: None,
        cache: None,
    });
    (module, pipeline)
}

/// How many frames pass between looks at the WGSL file's modified time:
/// about four times a second at 60 fps, and one `stat` each.
#[cfg(not(target_arch = "wasm32"))]
const POLL_FRAMES: u32 = 15;

// ---------------------------------------------------------------------------
// Images (#413)
// ---------------------------------------------------------------------------

/// One image binding and what it holds now.
struct ImageSlot {
    source: ImageSource,
    /// An image option's choices and default (`None` for a fixed image).
    option: Option<OptionSource>,
    /// The file loaded now (relative to the style's folder) and its view.
    loaded: Option<(String, wgpu::TextureView)>,
    /// That file's modified time when read, for the poll.
    #[cfg(not(target_arch = "wasm32"))]
    modified: Option<std::time::SystemTime>,
    /// A file that failed, with its modified time then: not tried again
    /// until it changes, and its error.
    failed: Option<(String, Option<std::time::SystemTime>, String)>,
}

/// A style's images: decoded through the vfs when first drawn or chosen,
/// read again when their files change (natively), bound in group 3.
struct ImageSet {
    dir: PathBuf,
    slots: Vec<ImageSlot>,
    layout: wgpu::BindGroupLayout,
    /// Made again whenever a slot's view changes.
    bind: Option<wgpu::BindGroup>,
    /// Bumped whenever a slot's view changes: the key of the bind groups a
    /// pass that reads targets makes over the images and its targets (#412).
    generation: u64,
    #[cfg(not(target_arch = "wasm32"))]
    since_poll: u32,
}

/// What one [`ImageSet::update`] did.
#[derive(Debug, Default, PartialEq)]
struct ImageUpdate {
    /// Files read (first loads, new choices and reloads).
    loaded: Vec<String>,
    /// Every slot has an image to bind.
    ready: bool,
    /// The slots' errors, one line each.
    error: Option<String>,
}

impl ImageSet {
    fn new(device: &wgpu::Device, source: &StyleSource) -> Option<Self> {
        if source.images.is_empty() {
            return None;
        }
        let entries: Vec<wgpu::BindGroupLayoutEntry> = (0..source.images.len())
            .map(|i| {
                texture_entry(
                    i as u32,
                    wgpu::TextureSampleType::Float { filterable: true },
                )
            })
            .collect();
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("User Style Images BGL"),
            entries: &entries,
        });
        let slots = source
            .images
            .iter()
            .map(|image| ImageSlot {
                option: source
                    .options
                    .iter()
                    .find(|(n, _)| *n == image.name)
                    .map(|(_, o)| o.clone()),
                source: image.clone(),
                loaded: None,
                #[cfg(not(target_arch = "wasm32"))]
                modified: None,
                failed: None,
            })
            .collect();
        Some(Self {
            dir: source.dir().to_path_buf(),
            slots,
            layout,
            bind: None,
            generation: 0,
            #[cfg(not(target_arch = "wasm32"))]
            since_poll: 0,
        })
    }

    /// Load what each slot should hold: its file, or for an option the
    /// file `choose(name)` names (its default when that isn't a choice).
    /// With `poll`, natively, a file whose modified time moved is read
    /// again. A file that fails keeps the slot's last good image.
    fn update(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        choose: &dyn Fn(&str) -> String,
        poll: bool,
    ) -> ImageUpdate {
        let mut update = ImageUpdate::default();
        for slot in &mut self.slots {
            let want = match (&slot.source.file, &slot.option) {
                (Some(file), _) => file.clone(),
                (None, Some(option)) => {
                    let chosen = choose(&slot.source.name);
                    if option.choices.contains(&chosen) {
                        chosen
                    } else {
                        option.default.clone()
                    }
                }
                (None, None) => continue,
            };
            let path = self.dir.join(&want);
            #[cfg(not(target_arch = "wasm32"))]
            let now = modified(&path);
            #[cfg(target_arch = "wasm32")]
            let now: Option<std::time::SystemTime> = None;
            let is_loaded = slot.loaded.as_ref().is_some_and(|(f, _)| *f == want);
            #[cfg(not(target_arch = "wasm32"))]
            let stale = is_loaded && poll && now.is_some() && now != slot.modified;
            #[cfg(target_arch = "wasm32")]
            let stale = {
                let _ = poll;
                false
            };
            let tried = slot
                .failed
                .as_ref()
                .is_some_and(|(f, at, _)| *f == want && *at == now);
            if is_loaded && !stale {
                // Back on a file that reads: an old choice's failure is past
                if slot.failed.as_ref().is_some_and(|(f, _, _)| *f != want) {
                    slot.failed = None;
                }
                continue;
            }
            if tried {
                continue;
            }
            match upload_image(device, queue, &path, slot.source.srgb, &slot.source.name) {
                Ok(view) => {
                    slot.loaded = Some((want.clone(), view));
                    #[cfg(not(target_arch = "wasm32"))]
                    {
                        slot.modified = now;
                    }
                    slot.failed = None;
                    self.bind = None;
                    self.generation += 1;
                    update.loaded.push(want);
                }
                Err(e) => slot.failed = Some((want, now, e)),
            }
        }
        // A slot showing another file than it was asked for (a new choice
        // that failed) keeps drawing its last good one; the error says so
        let errors: Vec<&str> = self
            .slots
            .iter()
            .filter_map(|s| s.failed.as_ref().map(|(_, _, e)| e.as_str()))
            .collect();
        update.error = (!errors.is_empty()).then(|| errors.join("\n"));
        update.ready = self.slots.iter().all(|s| s.loaded.is_some());
        update
    }

    /// The group 3 bind group over every slot's view (all loaded).
    fn bind_group(&mut self, device: &wgpu::Device) -> &wgpu::BindGroup {
        let slots = &self.slots;
        let layout = &self.layout;
        self.bind.get_or_insert_with(|| {
            let entries: Vec<wgpu::BindGroupEntry> = slots
                .iter()
                .enumerate()
                .map(|(i, s)| wgpu::BindGroupEntry {
                    binding: i as u32,
                    resource: wgpu::BindingResource::TextureView(
                        &s.loaded.as_ref().expect("every slot is loaded").1,
                    ),
                })
                .collect();
            device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("User Style Images BG"),
                layout,
                entries: &entries,
            })
        })
    }
}

/// Decode the image at `path` (through the vfs) into a texture of its own,
/// 8 bits a channel, one mip: `Rgba8UnormSrgb` with `srgb` (read back as
/// linear light), else `Rgba8Unorm` (the stored values).
fn upload_image(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    path: &Path,
    srgb: bool,
    name: &str,
) -> Result<wgpu::TextureView, String> {
    let image = crate::open_image(path)
        .map_err(|e| format!("{}: {e}", path.display()))?
        .to_rgba8();
    let (width, height) = image.dimensions();
    let max = device.limits().max_texture_dimension_2d;
    if width == 0 || height == 0 || width > max || height > max {
        return Err(format!(
            "{}: {width}x{height} is not a size this GPU binds (1..{max} a side)",
            path.display()
        ));
    }
    let size = wgpu::Extent3d {
        width,
        height,
        depth_or_array_layers: 1,
    };
    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some(&format!("User Style Image {name}")),
        size,
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: if srgb {
            wgpu::TextureFormat::Rgba8UnormSrgb
        } else {
            wgpu::TextureFormat::Rgba8Unorm
        },
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    });
    queue.write_texture(
        wgpu::ImageCopyTexture {
            texture: &texture,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        &image,
        wgpu::ImageDataLayout {
            offset: 0,
            bytes_per_row: Some(width * 4),
            rows_per_image: Some(height),
        },
        size,
    );
    Ok(texture.create_view(&wgpu::TextureViewDescriptor::default()))
}

/// Clear `view` to transparent black: a history target whose past isn't
/// this picture's reads as nothing (#412).
fn clear(encoder: &mut wgpu::CommandEncoder, view: &wgpu::TextureView) {
    let _ = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
        label: Some("User Style History Clear"),
        color_attachments: &[Some(wgpu::RenderPassColorAttachment {
            view,
            resolve_target: None,
            ops: wgpu::Operations {
                load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                store: wgpu::StoreOp::Store,
            },
        })],
        depth_stencil_attachment: None,
        timestamp_writes: None,
        occlusion_query_set: None,
    });
}

impl CustomPass {
    /// Record pass `pass`'s error (or that it builds), and the style's
    /// error as all of its passes' together.
    fn set_error(&self, pass: usize, error: Option<String>) {
        let name = &self.loaded.source.name;
        {
            let mut slot = self.passes[pass]
                .error
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            if let Some(e) = &error {
                if slot.as_ref() != Some(e) {
                    tracing::warn!("style '{name}': {e}");
                }
            } else if slot.is_some() {
                if self.passes.len() > 1 {
                    tracing::info!(
                        "style '{name}': pass '{}' compiles again",
                        self.loaded.source.passes[pass].name
                    );
                } else {
                    tracing::info!("style '{name}' compiles again");
                }
            }
            *slot = error;
        }
        let all: Vec<Option<String>> = self
            .passes
            .iter()
            .map(|p| p.error.lock().unwrap_or_else(|e| e.into_inner()).clone())
            .collect();
        *self.loaded.error.lock().unwrap_or_else(|e| e.into_inner()) = join_errors(&all);
    }

    /// Read, check and build pass `i`'s shader; on failure keep what was
    /// drawing (its last good build, or the passthrough).
    fn rebuild(&self, device: &wgpu::Device, i: usize) {
        let source = &self.loaded.source;
        let path = &source.passes[i].wgsl_path;
        let gpu = &self.passes[i];
        #[cfg(not(target_arch = "wasm32"))]
        let modified = modified(path);
        let user = match flint_core::vfs::read_to_string(path) {
            Ok(user) => user,
            Err(e) => {
                self.set_error(i, Some(format!("{}: {e}", path.display())));
                return;
            }
        };
        let mut build = gpu.build.lock().unwrap_or_else(|e| e.into_inner());
        #[cfg(not(target_arch = "wasm32"))]
        {
            build.modified = modified;
        }
        let report = match check_pass(source, i, &user) {
            Ok(report) => report,
            Err(e) => {
                drop(build);
                self.set_error(i, Some(e));
                return;
            }
        };
        // naga has passed it natively; the compiler's messages and the error
        // scope catch what the backend's own compiler still refuses (and, on
        // the web, everything). Neither is awaited: the build waits as
        // `pending` until both settle (#415)
        //
        // The guarded module when it has loops (#411), else the text as
        // composed. naga wrote the guarded one, so its lines aren't the
        // user's: a compiler message on it says "in the generated code"
        let composed = match report.guarded {
            Some(text) => Composed {
                text,
                prelude_lines: 0,
                user_lines: 0,
            },
            None => compose_pass(source, i, &user),
        };
        device.push_error_scope(wgpu::ErrorFilter::Validation);
        let (module, made) = pipeline(device, &gpu.layout, &composed.text, gpu.format);
        let scope = Settling::new(device.pop_error_scope());
        let info = Settling::new(module.get_compilation_info());
        build.pending = Some(Pending {
            pipeline: made,
            composed,
            info,
            scope,
        });
        self.settle(i, &mut build);
    }

    /// Poll pass `i`'s pending build; once the GPU has answered, draw it,
    /// or keep what was drawing and say why. Natively the answer is in on
    /// the first poll, inside [`Self::rebuild`].
    fn settle(&self, i: usize, build: &mut Build) {
        let Some(pending) = build.pending.as_mut() else {
            return;
        };
        let info_in = pending.info.poll();
        if !(pending.scope.poll() && info_in) {
            return;
        }
        let Some(pending) = build.pending.take() else {
            return;
        };
        let source = &self.loaded.source;
        let info = pending.info.take();
        let refused = pending.scope.take().flatten();
        let compiled = info
            .as_ref()
            .and_then(|info| compiler_error_pass(source, i, &pending.composed, info));
        match (compiled, refused) {
            (Some(e), _) => self.set_error(i, Some(e)),
            (None, Some(e)) => self.set_error(
                i,
                Some(format!(
                    "{}: the GPU refused it: {e}",
                    source.passes[i].wgsl_path.display()
                )),
            ),
            (None, None) => {
                build.pipeline = Some(pending.pipeline);
                self.set_error(i, None);
            }
        }
    }

    /// Bring the images up to date for this frame (#413): the scene's
    /// choices, and natively a look at the files' modified times every
    /// [`POLL_FRAMES`]. Records their error, logged when it changes.
    /// Whether every image has something to bind.
    fn update_images(&self, frame: &StyleFrame<'_>) -> bool {
        let Some(images) = &self.images else {
            return true;
        };
        let mut images = images.lock().unwrap_or_else(|e| e.into_inner());
        #[cfg(not(target_arch = "wasm32"))]
        let poll = {
            images.since_poll += 1;
            let due = images.since_poll >= POLL_FRAMES;
            if due {
                images.since_poll = 0;
            }
            due
        };
        #[cfg(target_arch = "wasm32")]
        let poll = false;
        let update = images.update(
            frame.device,
            frame.queue,
            &|name| frame.option(name).to_string(),
            poll,
        );
        let name = &self.loaded.source.name;
        for file in &update.loaded {
            tracing::debug!("style '{name}': image {file} read");
        }
        let mut slot = self
            .loaded
            .image_error
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        if *slot != update.error {
            match &update.error {
                Some(e) => tracing::warn!("style '{name}': {e}"),
                None => tracing::info!("style '{name}': its images read again"),
            }
            *slot = update.error;
        }
        update.ready
    }

    /// Rebuild each pass whose WGSL file changed on disk (native only).
    #[cfg(not(target_arch = "wasm32"))]
    fn poll(&self, device: &wgpu::Device) {
        for (i, pass) in self.loaded.source.passes.iter().enumerate() {
            let changed = {
                let mut build = self.passes[i]
                    .build
                    .lock()
                    .unwrap_or_else(|e| e.into_inner());
                build.since_poll += 1;
                if build.since_poll < POLL_FRAMES {
                    continue;
                }
                build.since_poll = 0;
                let now = modified(&pass.wgsl_path);
                now.is_some() && now != build.modified
            };
            if changed {
                self.rebuild(device, i);
            }
        }
    }

    /// The frame uniform of pass `i`, drawing `size`.
    fn uniforms(
        &self,
        frame: &StyleFrame<'_>,
        i: usize,
        size: [u32; 2],
        history: bool,
    ) -> FlintFrameUniforms {
        let camera = frame.camera;
        FlintFrameUniforms {
            inv_view_proj: camera.inv_view_proj,
            resolution: [frame.width as f32, frame.height as f32],
            texel: frame.texel_size(),
            depth_size: [frame.depth_size[0] as f32, frame.depth_size[1] as f32],
            time: frame.time,
            frame: frame.frame as u32,
            near: camera.near,
            far: camera.far,
            srgb: if frame.srgb() { 1.0 } else { 0.0 },
            pixels_per_point: frame.pixels_per_point,
            camera_pos: camera.position,
            mask_id: u32::from(
                frame
                    .style_id(self.loaded.source.name.as_str())
                    .unwrap_or(0),
            ),
            pass_size: [size[0] as f32, size[1] as f32],
            history: if history { 1.0 } else { 0.0 },
            pass_index: i as u32,
        }
    }

    /// Pass `i`'s group 0: its frame uniform and the shared settings table.
    fn frame_bind(&self, frame: &StyleFrame<'_>, i: usize) -> Arc<wgpu::BindGroup> {
        let name = self.loaded.names.passes[i].frame;
        frame.bind(name, 0, || {
            frame.device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some(name),
                layout: &self.frame_bgl,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: self.passes[i].frame_buffer.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: self.params_buffer.as_entire_binding(),
                    },
                ],
            })
        })
    }

    /// The input unchanged, through the last pass's frame uniform.
    fn draw_passthrough(
        &self,
        frame: &StyleFrame<'_>,
        encoder: &mut wgpu::CommandEncoder,
        input_bg: &wgpu::BindGroup,
        output: &wgpu::TextureView,
    ) {
        let last = self.passes.len() - 1;
        let uniforms = self.uniforms(frame, last, [frame.width, frame.height], false);
        frame.queue.write_buffer(
            &self.passes[last].frame_buffer,
            0,
            bytemuck::cast_slice(&[uniforms]),
        );
        let frame_bg = self.frame_bind(frame, last);
        kit::fullscreen_pass(
            encoder,
            self.loaded.names.passes[last].pass,
            output,
            &self.passthrough,
            &[&frame_bg, input_bg],
        );
    }
}

impl StylePass for CustomPass {
    fn encode(
        &self,
        frame: &StyleFrame<'_>,
        encoder: &mut wgpu::CommandEncoder,
        input: &StyleInput<'_>,
        output: &wgpu::TextureView,
    ) {
        #[cfg(not(target_arch = "wasm32"))]
        self.poll(frame.device);

        let source = &self.loaded.source;
        let names = &self.loaded.names;
        // The settings table (#414): row 0 the scene's, then one a variant
        // Each row is the params, then the options (#413) as u32 indices
        // written as their bits
        let row = |param: &dyn Fn(&str) -> f32, option: &dyn Fn(&str) -> u32| -> Vec<f32> {
            source
                .params
                .iter()
                .map(|(name, p)| param(name).clamp(p.min, p.max))
                .chain(
                    source
                        .options
                        .iter()
                        .map(|(name, _)| f32::from_bits(option(name))),
                )
                .collect()
        };
        let index = |name: &str, value: &str| {
            source
                .options
                .iter()
                .find(|(n, _)| n == name)
                .map_or(0, |(_, o)| o.index(value))
        };
        let mut rows = vec![row(&|n| frame.param(n), &|n| index(n, frame.option(n)))];
        let words = if source.inputs.mask {
            let mut slots = [0u32; MAX_IDS];
            let mut overflow = false;
            for (id, v) in frame.variants() {
                if v.off {
                    continue; // an opt-out reads as no entity of this style
                }
                let Some(slot) = slots.get_mut(id as usize) else {
                    overflow = true;
                    continue;
                };
                if rows.len() == MAX_VARIANTS {
                    overflow = true;
                    *slot = 1; // the scene's settings
                    continue;
                }
                rows.push(row(&|n| frame.variant_param(v, n), &|n| {
                    index(n, frame.variant_option(v, n))
                }));
                *slot = rows.len() as u32;
            }
            if overflow && !self.warned.swap(true, std::sync::atomic::Ordering::Relaxed) {
                tracing::warn!(
                    "style '{}': more per-entity variants than the {} a frame holds (or mask \
                     ids past {MAX_IDS}); the rest take the scene's settings",
                    source.name,
                    MAX_VARIANTS - 1
                );
            }
            table_words(source.uniform_fields(), &rows, Some(&slots))
        } else {
            table_words(source.uniform_fields(), &rows, None)
        };
        let images_ready = self.update_images(frame);
        frame
            .queue
            .write_buffer(&self.params_buffer, 0, bytemuck::cast_slice(&words));
        let input_bg = frame.sampled_input(names.input, &self.input_bgl, input);

        // Every pass must have built once (a build the browser hasn't
        // answered for yet settles here, #415): a chain missing a link draws
        // nothing meaningful, so the style passes its input through
        let mut builds: Vec<std::sync::MutexGuard<'_, Build>> = self
            .passes
            .iter()
            .map(|p| p.build.lock().unwrap_or_else(|e| e.into_inner()))
            .collect();
        for (i, build) in builds.iter_mut().enumerate() {
            self.settle(i, build);
        }
        if builds.iter().any(|b| b.pipeline.is_none()) {
            drop(builds);
            self.draw_passthrough(frame, encoder, &input_bg, output);
            return;
        }
        // An image that never read (a missing file) leaves the style a
        // passthrough with its error, as a shader that doesn't compile does
        if !images_ready {
            drop(builds);
            self.draw_passthrough(frame, encoder, &input_bg, output);
            return;
        }
        // The mask and the edges are there whenever this style is on and
        // reads them; if a host ever draws without one (or the edge pass
        // failed to build), the passthrough keeps the frame whole
        let extra_bg = match &self.extra_bgl {
            None => None,
            Some(_) if !source.inputs.any() => None,
            Some(bgl)
                if (!source.inputs.mask || frame.mask.is_some())
                    && (!source.inputs.edges || frame.edges.is_some()) =>
            {
                Some(frame.depth_bind(names.extra, frame.edges_key(), || {
                    let mut entries = Vec::new();
                    if source.inputs.depth {
                        entries.push(wgpu::BindGroupEntry {
                            binding: 0,
                            resource: wgpu::BindingResource::TextureView(frame.depth),
                        });
                    }
                    if let Some(mask) = frame.mask.filter(|_| source.inputs.mask) {
                        entries.push(wgpu::BindGroupEntry {
                            binding: 1,
                            resource: wgpu::BindingResource::TextureView(mask),
                        });
                    }
                    if let Some(edges) = frame.edges.as_ref().filter(|_| source.inputs.edges) {
                        entries.push(wgpu::BindGroupEntry {
                            binding: 2,
                            resource: wgpu::BindingResource::TextureView(&edges.view),
                        });
                    }
                    frame.device.create_bind_group(&wgpu::BindGroupDescriptor {
                        label: Some(names.extra),
                        layout: bgl,
                        entries: &entries,
                    })
                }))
            }
            Some(_) => {
                drop(builds);
                self.draw_passthrough(frame, encoder, &input_bg, output);
                return;
            }
        };

        // This frame's targets (#412): each history target's pair, the
        // others once
        let sizes: Vec<[u32; 2]> = source
            .targets
            .iter()
            .map(|t| t.size(frame.width, frame.height))
            .collect();
        let textures: Vec<[Arc<super::StyleTexture>; 2]> = source
            .targets
            .iter()
            .enumerate()
            .map(|(k, t)| {
                let [a, b] = names.targets[k];
                let [w, h] = sizes[k];
                let first = frame.target_sized(a, t.format.wgpu(), w, h);
                let second = if t.history {
                    frame.target_sized(b, t.format.wgpu(), w, h)
                } else {
                    first.clone()
                };
                [first, second]
            })
            .collect();
        let mut history = self.history.lock().unwrap_or_else(|e| e.into_inner());
        let used: Vec<Arc<super::StyleTexture>> = textures.iter().flatten().cloned().collect();
        let trusted = history.last.as_ref().is_some_and(|last| {
            last.frame.wrapping_add(1) == frame.frame
                && last.epoch == frame.history_epoch
                && last.textures.len() == used.len()
                && last
                    .textures
                    .iter()
                    .zip(&used)
                    .all(|(a, b)| Arc::ptr_eq(a, b))
        });
        let write = history.parity;
        let previous = 1 - write;
        if !trusted {
            for (k, t) in source.targets.iter().enumerate() {
                if t.history {
                    clear(encoder, &textures[k][previous].view);
                }
            }
        }

        let mut images = self
            .images
            .as_ref()
            .map(|i| i.lock().unwrap_or_else(|e| e.into_inner()));
        for (i, pass) in source.passes.iter().enumerate() {
            let size = pass
                .target
                .map_or([frame.width, frame.height], |t| sizes[t]);
            let uniforms = self.uniforms(frame, i, size, trusted);
            frame.queue.write_buffer(
                &self.passes[i].frame_buffer,
                0,
                bytemuck::cast_slice(&[uniforms]),
            );
            let frame_bg = self.frame_bind(frame, i);
            let pick = |r: &ReadSource| if r.previous { previous } else { write };
            // Group 3: the images and the targets this pass reads, keyed by
            // what changes the views (each read's size, which of a history
            // pair it is this frame, and the images' generation); or the
            // images' own group for a pass that reads no target
            let targets_bg = self.passes[i].targets_bgl.as_ref().map(|bgl| {
                let start = images.as_ref().map_or(0, |im| im.generation);
                let key = pass.reads.iter().fold(
                    0xcbf2_9ce4_8422_2325u64 ^ start.wrapping_mul(0x9e37_79b9_7f4a_7c15),
                    |h, r| {
                        let [w, ht] = sizes[r.target];
                        let side = if source.targets[r.target].history {
                            pick(r) as u64 + 1
                        } else {
                            0
                        };
                        let v = (u64::from(w) << 34) ^ (u64::from(ht) << 4) ^ side;
                        (h ^ v).wrapping_mul(0x0100_0000_01b3)
                    },
                );
                frame.bind(names.passes[i].targets, key, || {
                    let mut entries: Vec<wgpu::BindGroupEntry> = images
                        .as_ref()
                        .map(|im| {
                            im.slots
                                .iter()
                                .enumerate()
                                .map(|(b, s)| wgpu::BindGroupEntry {
                                    binding: b as u32,
                                    resource: wgpu::BindingResource::TextureView(
                                        &s.loaded.as_ref().expect("every image is loaded").1,
                                    ),
                                })
                                .collect()
                        })
                        .unwrap_or_default();
                    let first = source.images.len();
                    entries.extend(pass.reads.iter().enumerate().map(|(b, r)| {
                        wgpu::BindGroupEntry {
                            binding: (first + b) as u32,
                            resource: wgpu::BindingResource::TextureView(
                                &textures[r.target][pick(r)].view,
                            ),
                        }
                    }));
                    frame.device.create_bind_group(&wgpu::BindGroupDescriptor {
                        label: Some(names.passes[i].targets),
                        layout: bgl,
                        entries: &entries,
                    })
                })
            });
            let images_bg: Option<&wgpu::BindGroup> = match (&targets_bg, images.as_mut()) {
                (None, Some(im)) => Some(im.bind_group(frame.device)),
                _ => None,
            };
            let group3: Option<&wgpu::BindGroup> = targets_bg.as_deref().or(images_bg);
            let mut groups: Vec<&wgpu::BindGroup> = vec![&frame_bg, &input_bg];
            if let Some(extra) = &extra_bg {
                groups.push(extra);
            } else if group3.is_some() {
                if let Some(empty) = &self.empty {
                    groups.push(empty);
                }
            }
            if let Some(g) = group3 {
                groups.push(g);
            }
            let out = match pass.target {
                Some(t) => &textures[t][write].view,
                None => output,
            };
            let pipeline = builds[i].pipeline.as_ref().expect("checked above");
            kit::fullscreen_pass(encoder, names.passes[i].pass, out, pipeline, &groups);
        }
        if source.has_history() {
            history.parity = previous;
        }
        history.last = Some(LastFrame {
            frame: frame.frame,
            epoch: frame.history_epoch,
            textures: used,
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn source(toml_text: &str, file: &str) -> Result<StyleSource, String> {
        let def: ShaderStyleDef = toml::from_str(toml_text).map_err(|e| e.to_string())?;
        def.check(Path::new(file))
    }

    /// #396: a shader that reads its own blend opts out of the framework's
    /// cross-fade; a mention in a comment doesn't count.
    #[test]
    fn a_shader_calling_flint_style_blend_blends_itself() {
        assert!(calls_style_blend(
            "fn style(p: StylePixel) -> vec4<f32> {\n    return p.color * flint_style_blend(p.uv);\n}\n"
        ));
        assert!(!calls_style_blend(
            "// no flint_style_blend here: the framework fades me\nfn style(p: StylePixel) -> vec4<f32> {\n    return p.color;\n}\n"
        ));
    }

    fn duotone() -> StyleSource {
        source(
            r#"
            version = 1
            name = "duotone"
            shader = "duotone.wgsl"
            inputs = ["depth"]
            [params.mix]
            default = 1.0
            min = 0.0
            max = 1.0
            [params.ink_r]
            default = 0.1
            min = 0.0
            max = 1.0
            "#,
            "styles/duotone.shader.toml",
        )
        .unwrap()
    }

    #[test]
    fn descriptions_are_checked() {
        let ok = duotone();
        assert_eq!(ok.stage, StyleStage::Display);
        assert!(ok.inputs.depth && !ok.inputs.mask);
        assert_eq!(ok.label, "Duotone");
        assert_eq!(ok.wgsl_path, Path::new("styles").join("duotone.wgsl"));
        let names: Vec<&str> = ok.params.iter().map(|(n, _)| n.as_str()).collect();
        assert_eq!(names, ["ink_r", "mix"], "uniform order is by name");

        let base = "version = 1\nname = \"x\"\nshader = \"x.wgsl\"\n";
        let file = "styles/x.shader.toml";
        let err = |extra: &str| source(&format!("{base}{extra}"), file).unwrap_err();
        assert!(source(base, "styles/y.shader.toml")
            .unwrap_err()
            .contains("must match"));
        assert!(err("stage = \"late\"\n").contains("\"display\" or \"hdr\""));
        assert!(err("inputs = [\"normals\"]\n").contains("unknown input"));
        assert!(err("[params.flint_x]\ndefault = 0\nmin = 0\nmax = 1\n").contains("flint"));
        // Every table's own keys (#299)
        assert!(err("[params.order]\ndefault = 0\nmin = 0\nmax = 1\n").contains("taken"));
        assert!(err("[params.a]\ndefault = 2\nmin = 0\nmax = 1\n").contains("outside"));
        assert!(err("wobble = 1\n").contains("unknown field"));
        // feeds_bloom (#300): hdr styles only, off by default
        assert!(err("feeds_bloom = true\n").contains("stage = \"hdr\""));
        assert!(!source(base, file).unwrap().feeds_bloom);
        assert!(
            source(
                &format!("{base}stage = \"hdr\"\nfeeds_bloom = true\n"),
                file
            )
            .unwrap()
            .feeds_bloom
        );
        let up = "version = 1\nname = \"x\"\nshader = \"../x.wgsl\"\n";
        assert!(source(up, file).unwrap_err().contains("beside it"));
        let v2 = "version = 2\nname = \"x\"\nshader = \"x.wgsl\"\n";
        assert!(source(v2, file).unwrap_err().contains("version"));
        let builtin = "version = 1\nname = \"ascii\"\nshader = \"x.wgsl\"\n";
        assert!(source(builtin, "styles/ascii.shader.toml")
            .unwrap_err()
            .contains("built-in"));
    }

    #[test]
    fn a_mask_reading_style_is_per_entity() {
        let base = "version = 1\nname = \"x\"\nshader = \"x.wgsl\"\n";
        let file = "styles/x.shader.toml";
        assert_eq!(source(base, file).unwrap().component, None);
        let masked = source(&format!("{base}inputs = [\"mask\"]\n"), file).unwrap();
        assert_eq!(masked.component.as_deref(), Some("x_style"));
    }

    #[test]
    fn the_settings_table_holds_a_row_a_variant_and_a_slot_an_id() {
        // Two params: a 32-byte row; 16 rows, then 256 slots
        assert_eq!(table_size(2, false), 32);
        assert_eq!(table_size(2, true), 16 * 32 + 256 * 4);
        let rows = vec![vec![0.5, 1.0], vec![0.25, 2.0]];
        let mut slots = [0u32; MAX_IDS];
        slots[3] = 2;
        let words = table_words(2, &rows, Some(&slots));
        assert_eq!(words.len(), table_size(2, true) / 4);
        let f = |i: usize| f32::from_bits(words[i]);
        assert_eq!((f(0), f(1)), (0.5, 1.0), "row 0, the scene's");
        assert_eq!((f(8), f(9)), (0.25, 2.0), "row 1 at the next 32 bytes");
        assert_eq!(words[16 * 8 + 3], 2, "id 3 takes row 1 (+1)");
        let bare = table_words(2, &rows, None);
        assert_eq!(bare.len(), 8, "a style without the mask holds row 0 alone");
    }

    #[test]
    fn neighbours_settings_are_readable() {
        let s = source(
            "version = 1\nname = \"x\"\nshader = \"x.wgsl\"\ninputs = [\"mask\"]\n\
             [params.glow]\ndefault = 0.5\nmin = 0.0\nmax = 1.0\n",
            "styles/x.shader.toml",
        )
        .unwrap();
        let user = "fn style(p: StylePixel) -> vec4<f32> {\n    \
                    let n = flint_params_at(p.uv + flint.texel);\n    \
                    return p.color * (params.glow + n.glow);\n}\n";
        let report = check(&s, user).unwrap();
        assert_eq!(report.texture_reads, 1, "flint_params_at reads the mask");
    }

    const HEAD: &str = "version = 1\nname = \"x\"\n";
    const FILE: &str = "styles/x.shader.toml";

    fn passes(body: &str) -> Result<StyleSource, String> {
        source(&format!("{HEAD}{body}"), FILE)
    }

    /// #412: `[[passes]]` are checked when the description is read, and
    /// each error names the passes.
    #[test]
    fn passes_are_checked_and_errors_name_them() {
        let ok = passes(
            r#"
            [[passes]]
            name = "down"
            shader = "down.wgsl"
            target = "small"
            scale = 0.5
            [[passes]]
            shader = "blur.wgsl"
            target = "blurred"
            format = "rgba8"
            scale = 0.5
            reads = ["small"]
            [[passes]]
            name = "out"
            shader = "out.wgsl"
            reads = ["blurred", "small"]
            "#,
        )
        .unwrap();
        let names: Vec<&str> = ok.passes.iter().map(|p| p.name.as_str()).collect();
        assert_eq!(names, ["down", "pass2", "out"], "a default name by place");
        assert_eq!(ok.targets.len(), 2);
        assert_eq!(ok.targets[1].format, TargetFormat::Rgba8);
        assert_eq!(ok.targets[0].size(1920, 1080), [960, 540]);
        assert_eq!(ok.passes[2].reads.len(), 2);
        assert!(ok.passes[2].reads.iter().all(|r| !r.previous));
        assert!(ok.notes.is_empty(), "{:?}", ok.notes);
        let files: Vec<_> = ok.wgsl_paths().collect();
        assert_eq!(files[1], Path::new("styles").join("blur.wgsl"));

        let err = |body: &str| passes(body).unwrap_err();
        let pass = |name: &str, extra: &str| {
            format!("[[passes]]\nname = \"{name}\"\nshader = \"{name}.wgsl\"\n{extra}\n")
        };
        // A read before the write names both passes
        let e = err(&format!(
            "{}{}{}",
            pass("a", "target = \"ta\"\nreads = [\"tb\"]"),
            pass("b", "target = \"tb\""),
            pass("c", "reads = [\"ta\", \"tb\"]"),
        ));
        assert!(
            e.contains("pass \"a\" reads target \"tb\" before pass \"b\" writes it"),
            "{e}"
        );
        // A cycle names its passes in order
        let e = err(&format!(
            "{}{}{}",
            pass("a", "target = \"ta\"\nreads = [\"tb\"]"),
            pass("b", "target = \"tb\"\nreads = [\"ta\"]"),
            pass("c", "reads = [\"tb\"]"),
        ));
        assert!(
            e.contains("a cycle") && e.contains("\"a\" -> \"b\" -> \"a\""),
            "{e}"
        );
        // A pass reading its own target without history
        let e = err(&format!(
            "{}{}",
            pass("a", "target = \"ta\"\nreads = [\"ta\"]"),
            pass("b", "reads = [\"ta\"]"),
        ));
        assert!(e.contains("which it writes: a cycle"), "{e}");
        // ... is last frame's with history
        let h = passes(&format!(
            "{}{}",
            pass("a", "target = \"ta\"\nhistory = true\nreads = [\"ta\"]"),
            pass("b", "reads = [\"ta\"]"),
        ))
        .unwrap();
        assert!(h.passes[0].reads[0].previous && !h.passes[1].reads[0].previous);
        assert!(h.has_history());
        // Shape: targets on every pass but the last, none on it
        assert!(err(&pass("a", "target = \"t\"")).contains("is the last"));
        assert!(err(&format!("{}{}", pass("a", ""), pass("b", ""))).contains("writes nothing"));
        assert!(err(&pass("a", "scale = 0.5")).contains("but no target"));
        assert!(err(&format!(
            "{}{}",
            pass("a", "target = \"t\"\nformat = \"rgb565\""),
            pass("b", "")
        ))
        .contains("rgba16f, rgba8 or r32f"));
        assert!(err(&format!(
            "{}{}",
            pass("a", "target = \"t\"\nscale = 2"),
            pass("b", "")
        ))
        .contains("0.0625..1"));
        assert!(err(&format!(
            "{}{}",
            pass("a", "target = \"t\""),
            pass("b", "reads = [\"u\"]")
        ))
        .contains("which no pass writes"));
        assert!(err(&format!(
            "{}{}{}",
            pass("a", "target = \"t\""),
            pass("b", "target = \"t\""),
            pass("c", "")
        ))
        .contains("both write target \"t\""));
        assert!(err(&format!("{}{}", pass("a", ""), pass("a", ""))).contains("two passes"));
        assert!(err("shader = \"x.wgsl\"\n[[passes]]\nshader = \"y.wgsl\"\n").contains("not both"));
        assert!(err("").contains("no shader"));
        assert!(err(&pass("a", "").replace("a.wgsl", "../a.wgsl")).contains("beside it"));
        assert!(
            err(&format!("{}reads = [\"t\"]\nwobble = 1\n", pass("a", "")))
                .contains("unknown field")
        );
        // A target nothing reads is a note, not an error
        let unread = passes(&format!("{}{}", pass("a", "target = \"t\""), pass("b", ""))).unwrap();
        assert!(
            unread.notes[0].contains("no pass reads it"),
            "{:?}",
            unread.notes
        );
    }

    /// #412: at most four targets, and together no more than four
    /// full-size rgba16f (history counting twice).
    #[test]
    fn targets_are_capped_in_count_and_memory() {
        let chain = |targets: &[&str]| {
            let mut body = String::new();
            for (i, t) in targets.iter().enumerate() {
                body.push_str(&format!(
                    "[[passes]]\nshader = \"p{i}.wgsl\"\ntarget = \"t{i}\"\n{t}\n"
                ));
            }
            let reads: Vec<String> = (0..targets.len()).map(|i| format!("\"t{i}\"")).collect();
            body.push_str(&format!(
                "[[passes]]\nshader = \"last.wgsl\"\nreads = [{}]\n",
                reads.join(", ")
            ));
            passes(&body)
        };
        assert!(
            chain(&[""; 4]).is_ok(),
            "four full-size rgba16f: the budget"
        );
        assert!(chain(&[""; 5]).unwrap_err().contains("at most 4"));
        assert!(
            chain(&["history = true", "", ""]).is_ok(),
            "16 + 8 + 8: on the line"
        );
        let e = chain(&["history = true", "", "", "format = \"rgba8\""]).unwrap_err();
        assert!(e.contains("budget is 32"), "{e}");
        assert!(chain(&[
            "history = true",
            "",
            "scale = 0.5",
            "format = \"r32f\"\nscale = 0.5"
        ])
        .is_ok());
    }

    /// #412: every pass's prelude validates, whatever it reads: each
    /// format, this frame's or last frame's, with and without other inputs.
    #[test]
    fn pass_preludes_validate_for_every_read() {
        for inputs in ["[]", "[\"depth\", \"mask\", \"edges\"]"] {
            for stage in ["display", "hdr"] {
                let s = passes(&format!(
                    "stage = \"{stage}\"\ninputs = {inputs}\n\
                     [[passes]]\nshader = \"a.wgsl\"\ntarget = \"h\"\nhistory = true\nreads = [\"h\"]\n\
                     [[passes]]\nshader = \"b.wgsl\"\ntarget = \"f\"\nformat = \"r32f\"\nscale = 0.5\nreads = [\"h\"]\n\
                     [[passes]]\nshader = \"c.wgsl\"\ntarget = \"e\"\nformat = \"rgba8\"\nreads = [\"f\", \"h\"]\n\
                     [[passes]]\nshader = \"d.wgsl\"\nreads = [\"e\", \"f\", \"h\"]\n"
                ))
                .unwrap();
                for i in 0..s.passes.len() {
                    check_pass(&s, i, PASSTHROUGH)
                        .unwrap_or_else(|e| panic!("{stage} {inputs} pass {i}: {e}"));
                }
                // The helpers of what the last pass reads
                let user = "fn style(p: StylePixel) -> vec4<f32> {\n    \
                            return flint_target_e(p.uv) + flint_target_f(p.uv) \
                            + flint_target_h_load(vec2<i32>(p.pos)) \
                            + vec4<f32>(flint_target_f_size(), flint.pass_size) * flint.history;\n}\n";
                let report = check_pass(&s, 3, user).unwrap();
                assert_eq!(report.texture_reads, 3, "the size isn't a read");
                // A target the pass doesn't read gets a hint
                let e = check_pass(&s, 0, user).unwrap_err();
                assert!(e.contains("add \"e\" to reads of pass \"pass1\""), "{e}");
            }
        }
    }

    /// #412 with #413: a style's images and the targets a pass reads share
    /// group 3, the images first, so neither moves the other.
    #[test]
    fn images_and_targets_share_group_three() {
        let s = passes(
            "[images.paper]\nfile = \"paper.png\"\n\
             [[passes]]\nshader = \"a.wgsl\"\ntarget = \"t\"\n\
             [[passes]]\nshader = \"b.wgsl\"\nreads = [\"t\"]\n",
        )
        .unwrap();
        let first = prelude_pass(&s, 0);
        assert!(first.contains("@group(3) @binding(0) var flint_image_texture_paper"));
        assert!(
            !first.contains("flint_target_t"),
            "the first pass reads no target"
        );
        let last = prelude_pass(&s, 1);
        assert!(last.contains("@group(3) @binding(0) var flint_image_texture_paper"));
        assert!(
            last.contains("@group(3) @binding(1) var flint_target_t_texture"),
            "the target after the image"
        );
        let user = "fn style(p: StylePixel) -> vec4<f32> {\n    \
                    return flint_image_paper(p.uv) * flint_target_t(p.uv);\n}\n";
        check_pass(&s, 1, user).unwrap();
        check_pass(&s, 0, PASSTHROUGH).unwrap();
    }

    #[test]
    fn params_round_up_to_vec4s_and_a_pad() {
        assert_eq!(params_size(0), 16);
        assert_eq!(params_size(1), 32);
        assert_eq!(params_size(4), 32);
        assert_eq!(params_size(5), 48);
        assert_eq!(std::mem::size_of::<FlintFrameUniforms>(), 144);
    }

    #[test]
    fn passthrough_and_prelude_validate_for_every_shape() {
        for stage in ["display", "hdr"] {
            for inputs in [
                "[]",
                "[\"depth\"]",
                "[\"mask\"]",
                "[\"color\", \"depth\", \"mask\"]",
                "[\"edges\"]",
                "[\"depth\", \"mask\", \"edges\"]",
            ] {
                let s = source(
                    &format!(
                        "version = 1\nname = \"x\"\nshader = \"x.wgsl\"\nstage = \"{stage}\"\n\
                         inputs = {inputs}\n[params.a]\ndefault = 0\nmin = 0\nmax = 1\n"
                    ),
                    "styles/x.shader.toml",
                )
                .unwrap();
                let report =
                    check(&s, PASSTHROUGH).unwrap_or_else(|e| panic!("{stage} {inputs}: {e}"));
                assert_eq!(report.loops, 0);
            }
        }
    }

    #[test]
    fn errors_point_into_the_users_file() {
        let s = duotone();
        let user =
            "fn style(p: StylePixel) -> vec4<f32> {\n    let x = 1.0\n    return p.color;\n}\n";
        let e = check(&s, user).unwrap_err();
        assert!(e.starts_with(&format!("{}:", s.wgsl_path.display())), "{e}");
        // The missing `;` is reported at the line after it (line 3)
        assert!(e.contains(":3:") || e.contains(":2:"), "{e}");
        assert!(e.contains("return p.color") || e.contains("let x"), "{e}");

        // A validation error (a type mismatch), not a parse error
        let user = "fn style(p: StylePixel) -> vec4<f32> {\n    return p.color.rgb;\n}\n";
        let e = check(&s, user).unwrap_err();
        assert!(e.contains(":2:") || e.contains("style"), "{e}");
    }

    #[test]
    fn helpers_of_undeclared_inputs_get_a_hint() {
        let s = duotone(); // depth, no mask
        let user = "fn style(p: StylePixel) -> vec4<f32> {\n    \
                    return p.color * flint_style_blend(p.uv);\n}\n";
        let e = check(&s, user).unwrap_err();
        assert!(e.contains("add \"mask\" to inputs"), "{e}");
        let user = "fn style(p: StylePixel) -> vec4<f32> {\n    \
                    return p.color * flint_view_depth(p.uv);\n}\n";
        check(&s, user).unwrap();
    }

    #[test]
    fn the_contract_is_enforced() {
        let s = duotone();
        let e = check(
            &s,
            "fn shade(p: StylePixel) -> vec4<f32> { return p.color; }\n",
        )
        .unwrap_err();
        assert!(e.contains("no `fn style"), "{e}");
        let user = "@group(3) @binding(0) var extra: texture_2d<f32>;\n\
                    fn style(p: StylePixel) -> vec4<f32> { return p.color; }\n";
        let e = check(&s, user).unwrap_err();
        assert!(e.contains("declares a binding") && e.contains(":1"), "{e}");
    }

    #[test]
    fn loops_and_reads_are_counted() {
        let s = duotone();
        let user = r#"
fn style(p: StylePixel) -> vec4<f32> {
    var acc = vec4<f32>(0.0);
    for (var i = 0; i < 4; i++) {
        for (var j = 0; j < 4; j++) {
            acc += flint_sample(p.uv + vec2<f32>(f32(i), f32(j)) * flint.texel);
        }
    }
    return acc / 16.0 * params.mix;
}
"#;
        let report = check(&s, user).unwrap();
        assert_eq!(report.loops, 2);
        assert_eq!(report.texture_reads, 1, "one call site, written out once");
        assert!(report.warnings.is_empty(), "guarded: {:?}", report.warnings);
        assert!(report.guarded.is_some());

        // With the guard off the loops are only warned about
        set_loop_guard(false);
        let report = check(&s, user).unwrap();
        set_loop_guard(true);
        assert_eq!(report.guarded, None);
        assert_eq!(report.warnings.len(), 1);
        assert!(report.warnings[0].contains("2 loops"));
    }

    /// #411: every user loop gets a counter of its own (reset where it
    /// starts) and a `break if` at [`LOOP_LIMIT`], the module still
    /// validates, and the prelude's own code is left alone.
    #[test]
    fn every_user_loop_is_guarded() {
        let s = duotone();
        let user = r#"
fn spin(x: f32) -> f32 {
    var y = x;
    loop {
        y = y + 1.0;
    }
    return y;
}
fn style(p: StylePixel) -> vec4<f32> {
    var acc = 0.0;
    var k = 0;
    while (true) {
        k += 1;
        if (k > 3) { continue; }
        for (var i = 0; i < 9; i++) { acc += 1.0; }
    }
    loop {
        acc += 1.0;
        continuing { break if acc > 2.0; }
    }
    return p.color * spin(acc);
}
"#;
        let report = check(&s, user).unwrap();
        assert_eq!(report.loops, 4);
        let text = report.guarded.expect("guarded");
        // It parses and validates as naga wrote it
        let module = naga::front::wgsl::parse_str(&text).unwrap_or_else(|e| panic!("{e}\n{text}"));
        naga::valid::Validator::new(
            naga::valid::ValidationFlags::all(),
            naga::valid::Capabilities::default(),
        )
        .validate(&module)
        .unwrap();
        let limit = format!(">= {LOOP_LIMIT}u");
        assert_eq!(text.matches(&limit).count(), 4, "{text}");
        assert_eq!(text.matches("break if").count(), 4, "{text}");
        // The user's own break_if is kept, or-ed with the guard
        assert!(text.contains("> 2f) || ("), "{text}");
        // The prelude has no loops; the written module none we didn't add
        assert_eq!(text.matches("loop {").count(), 4, "{text}");
    }

    #[test]
    fn user_lines_map_back() {
        let s = duotone();
        let c = compose(&s, "a\nb\n");
        assert_eq!(c.user_lines, 2);
        assert_eq!(c.user_line(c.prelude_lines + 1), Some(1));
        assert_eq!(c.user_line(c.prelude_lines + 2), Some(2));
        assert_eq!(c.user_line(c.prelude_lines + 3), None);
        assert_eq!(c.user_line(1), None);
        assert_eq!(c.text.lines().nth(c.prelude_lines), Some("a"));
    }

    fn device() -> Option<(wgpu::Device, wgpu::Queue)> {
        pollster::block_on(async {
            let instance = crate::gpu_select::instance();
            let adapter = crate::gpu_select::request_adapter(&instance, None).await?;
            adapter
                .request_device(&wgpu::DeviceDescriptor::default(), None)
                .await
                .ok()
        })
    }

    /// Hot reload on a real device: a good build, a broken edit that keeps
    /// the last good pipeline and reports, and a fix the poll picks up.
    #[test]
    fn hot_reload_keeps_the_last_good_build() {
        let Some((device, _queue)) = device() else {
            crate::gpu_select::skip_without_gpu("no GPU adapter");
            return;
        };
        let dir = std::env::temp_dir().join(format!("flint-user-reload-{}", std::process::id()));
        let styles = dir.join("styles");
        std::fs::create_dir_all(&styles).unwrap();
        let wgsl = styles.join("reload.wgsl");
        std::fs::write(
            styles.join("utest_reload.shader.toml"),
            "version = 1\nname = \"utest_reload\"\nshader = \"reload.wgsl\"\ninputs = [\"depth\", \"mask\"]\n",
        )
        .unwrap();
        std::fs::write(&wgsl, PASSTHROUGH).unwrap();
        assert!(load_dir(&dir).is_empty());
        let descriptor = super::super::find("utest_reload").unwrap();
        let pass = build(&StyleDevice {
            device: &device,
            descriptor,
            format: wgpu::TextureFormat::Rgba8UnormSrgb,
        });
        let error = || {
            errors()
                .into_iter()
                .find(|(n, _)| n == "utest_reload")
                .map(|(_, e)| e)
        };
        assert_eq!(error(), None);
        assert!(pass.passes[0].build.lock().unwrap().pipeline.is_some());

        // Break it on disk; the poll notices within POLL_FRAMES frames
        std::thread::sleep(std::time::Duration::from_millis(30));
        std::fs::write(
            &wgsl,
            "fn style(p: StylePixel) -> vec4<f32> {\n    return 1.0;\n}\n",
        )
        .unwrap();
        for _ in 0..POLL_FRAMES {
            pass.poll(&device);
        }
        let e = error().expect("the broken edit is reported");
        assert!(e.contains("reload.wgsl:"), "{e}");
        assert!(
            pass.passes[0].build.lock().unwrap().pipeline.is_some(),
            "last good build kept"
        );

        // Fix it: the error clears
        std::thread::sleep(std::time::Duration::from_millis(30));
        std::fs::write(
            &wgsl,
            "fn style(p: StylePixel) -> vec4<f32> {\n    return p.color * flint_style_blend(p.uv) + flint_depth(p.uv);\n}\n",
        )
        .unwrap();
        for _ in 0..POLL_FRAMES {
            pass.poll(&device);
        }
        assert_eq!(error(), None);
        let _ = std::fs::remove_dir_all(&dir);
    }

    fn message(
        message_type: wgpu::CompilationMessageType,
        text: &str,
        at: Option<(u32, u32)>,
    ) -> wgpu::CompilationMessage {
        wgpu::CompilationMessage {
            message: text.to_string(),
            message_type,
            location: at.map(|(line_number, line_position)| wgpu::SourceLocation {
                line_number,
                line_position,
                offset: 0,
                length: 1,
            }),
        }
    }

    /// The browser's report names lines of the composed module; the error
    /// shown names the user's file and line, quoted, as naga's does (#415).
    #[test]
    fn browser_messages_point_into_the_users_file() {
        use wgpu::CompilationMessageType::{Error, Warning};
        let s = duotone();
        let user = "fn style(p: StylePixel) -> vec4<f32> {\n    return p.color * glow;\n}\n";
        let c = compose(&s, user);
        let line = c.prelude_lines as u32 + 2;
        let info = wgpu::CompilationInfo {
            messages: vec![
                message(Warning, "unused variable", Some((line, 5))),
                message(Error, "unresolved identifier 'glow'", Some((line, 22))),
                message(Error, "a second one", Some((line, 1))),
            ],
        };
        let e = compiler_error(&s, &c, &info).unwrap();
        let file = s.wgsl_path.display().to_string();
        assert!(
            e.starts_with(&format!("{file}:2:22: unresolved identifier 'glow'")),
            "{e}"
        );
        assert!(
            e.contains("2 | ") && e.contains("return p.color * glow;"),
            "{e}"
        );
        assert!(e.contains("(and 1 more)"), "{e}");

        // Warnings alone are no error
        let info = wgpu::CompilationInfo {
            messages: vec![message(Warning, "unused variable", Some((line, 5)))],
        };
        assert_eq!(compiler_error(&s, &c, &info), None);

        // A line in the generated code, and no location at all
        let info = wgpu::CompilationInfo {
            messages: vec![message(Error, "bad param", Some((3, 1)))],
        };
        let e = compiler_error(&s, &c, &info).unwrap();
        assert!(e.contains("in the generated code, line 3"), "{e}");
        let info = wgpu::CompilationInfo {
            messages: vec![message(Error, "Getting compilation info failed", None)],
        };
        assert_eq!(
            compiler_error(&s, &c, &info).unwrap(),
            format!("{file}: Getting compilation info failed")
        );
    }

    /// Which pipeline this is (wgpu hashes resources by identity).
    fn identity(p: &wgpu::RenderPipeline) -> u64 {
        use std::hash::{Hash, Hasher};
        let mut h = std::collections::hash_map::DefaultHasher::new();
        p.hash(&mut h);
        h.finish()
    }

    /// A future that answers on its `polls`th poll, as a browser promise
    /// settles a frame or two after the pipeline was made.
    struct Late<T> {
        polls: u32,
        value: Option<T>,
    }

    impl<T: Unpin> std::future::Future for Late<T> {
        type Output = T;
        fn poll(
            mut self: std::pin::Pin<&mut Self>,
            _: &mut std::task::Context<'_>,
        ) -> std::task::Poll<T> {
            self.polls -= 1;
            if self.polls == 0 {
                std::task::Poll::Ready(self.value.take().unwrap())
            } else {
                std::task::Poll::Pending
            }
        }
    }

    /// The web's path, driven natively: a build the compiler hasn't
    /// answered for draws nothing new and reports nothing; when the answer
    /// comes frames later, an error lands in `errors()` with the user's
    /// line and the last good build keeps drawing; a clean answer swaps
    /// the new build in (#415).
    #[test]
    fn a_late_compiler_answer_settles_on_a_later_frame() {
        let Some((device, _queue)) = device() else {
            crate::gpu_select::skip_without_gpu("no GPU adapter");
            return;
        };
        let dir = std::env::temp_dir().join(format!("flint-user-late-{}", std::process::id()));
        let styles = dir.join("styles");
        std::fs::create_dir_all(&styles).unwrap();
        std::fs::write(
            styles.join("utest_late.shader.toml"),
            "version = 1\nname = \"utest_late\"\nshader = \"late.wgsl\"\n",
        )
        .unwrap();
        std::fs::write(styles.join("late.wgsl"), PASSTHROUGH).unwrap();
        assert!(load_dir(&dir).is_empty());
        let descriptor = super::super::find("utest_late").unwrap();
        let format = wgpu::TextureFormat::Rgba8UnormSrgb;
        let pass = build(&StyleDevice {
            device: &device,
            descriptor,
            format,
        });
        let error = || {
            errors()
                .into_iter()
                .find(|(n, _)| n == "utest_late")
                .map(|(_, e)| e)
        };
        assert_eq!(error(), None);
        let source = pass.loaded.source.clone();
        let user = "fn style(p: StylePixel) -> vec4<f32> {\n    return p.color * glow;\n}\n";
        let composed = compose(&source, user);
        let line = composed.prelude_lines as u32 + 2;
        let (_, made) = pipeline(
            &device,
            &pass.passes[0].layout,
            &compose(&source, PASSTHROUGH).text,
            format,
        );
        let first = pass.passes[0]
            .build
            .lock()
            .unwrap()
            .pipeline
            .as_ref()
            .map(identity)
            .unwrap();
        let made_id = identity(&made);
        pass.passes[0].build.lock().unwrap().pending = Some(Pending {
            pipeline: made,
            composed: composed.clone(),
            info: Settling::new(Late {
                polls: 3,
                value: Some(wgpu::CompilationInfo {
                    messages: vec![message(
                        wgpu::CompilationMessageType::Error,
                        "unresolved identifier 'glow'",
                        Some((line, 22)),
                    )],
                }),
            }),
            scope: Settling::new(Late {
                polls: 2,
                value: Some(None),
            }),
        });
        let settle = || {
            let mut build = pass.passes[0].build.lock().unwrap();
            pass.settle(0, &mut build);
            build.pending.is_some()
        };
        assert!(settle(), "frame 1: still waiting");
        assert!(settle(), "frame 2: the scope is in, the report isn't");
        assert_eq!(error(), None, "nothing to say until both answer");
        assert!(!settle(), "frame 3: settled");
        let e = error().expect("the browser's error is reported");
        assert!(
            e.contains("late.wgsl:2:22: unresolved identifier 'glow'"),
            "{e}"
        );
        assert_eq!(
            pass.passes[0]
                .build
                .lock()
                .unwrap()
                .pipeline
                .as_ref()
                .map(identity)
                .unwrap(),
            first,
            "the last good build keeps drawing"
        );

        // A clean answer, a frame late: the new build draws, the error clears
        let (_, made) = pipeline(
            &device,
            &pass.passes[0].layout,
            &compose(&source, PASSTHROUGH).text,
            format,
        );
        let made_id_2 = identity(&made);
        assert_ne!(made_id, made_id_2);
        pass.passes[0].build.lock().unwrap().pending = Some(Pending {
            pipeline: made,
            composed,
            info: Settling::new(Late {
                polls: 2,
                value: Some(wgpu::CompilationInfo { messages: vec![] }),
            }),
            scope: Settling::new(std::future::ready(None)),
        });
        assert!(settle());
        assert!(error().is_some(), "the old error stands while waiting");
        assert!(!settle());
        assert_eq!(error(), None);
        assert_eq!(
            pass.passes[0]
                .build
                .lock()
                .unwrap()
                .pipeline
                .as_ref()
                .map(identity)
                .unwrap(),
            made_id_2
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn load_dir_registers_styles_and_keeps_broken_ones_as_passthroughs() {
        let dir = std::env::temp_dir().join(format!("flint-user-styles-{}", std::process::id()));
        let styles = dir.join("styles");
        std::fs::create_dir_all(&styles).unwrap();
        std::fs::write(
            styles.join("utest_good.shader.toml"),
            "version = 1\nname = \"utest_good\"\nshader = \"good.wgsl\"\n\
             [params.k]\ndefault = 0.5\nmin = 0\nmax = 1\nlabel = \"Kay\"\n",
        )
        .unwrap();
        std::fs::write(styles.join("good.wgsl"), PASSTHROUGH).unwrap();
        std::fs::write(
            styles.join("utest_bad.shader.toml"),
            "version = 1\nname = \"utest_bad\"\nshader = \"bad.wgsl\"\n",
        )
        .unwrap();
        std::fs::write(
            styles.join("bad.wgsl"),
            "fn style(p: StylePixel) -> f32 {\n  return 1.0;\n}\n",
        )
        .unwrap();
        std::fs::write(styles.join("notes.toml"), "ignored = true\n").unwrap();

        let messages = load_dir(&dir);
        assert_eq!(messages.len(), 1, "{messages:?}");
        assert!(messages[0].contains("bad.wgsl"), "{messages:?}");
        let good = super::super::find("utest_good").expect("registered");
        assert_eq!(good.param(&Default::default(), "k"), 0.5);
        assert_eq!(good.label_for("k"), "Kay");
        assert!(
            super::super::find("utest_bad").is_some(),
            "a broken shader still registers"
        );
        assert!(errors().iter().any(|(n, _)| n == "utest_bad"));
        assert!(!errors().iter().any(|(n, _)| n == "utest_good"));
        // A scene table naming it reads its params now
        let tables: BTreeMap<String, toml::Table> =
            toml::from_str("[utest_good]\nk = 0.25\n").unwrap();
        let (settings, warnings) = super::super::StyleSettings::from_tables(&tables);
        assert!(warnings.is_empty(), "{warnings:?}");
        assert_eq!(settings.param("utest_good", "k"), Some(0.25));
        // Loading again is a no-op. Counted by name: tests running beside
        // this one register styles of their own in the same registry
        let copies = || {
            super::super::registry()
                .iter()
                .filter(|d| d.name.starts_with("utest_good") || d.name.starts_with("utest_bad"))
                .count()
        };
        let before = copies();
        assert!(load_dir(&dir).is_empty());
        assert_eq!(copies(), before);
        let _ = std::fs::remove_dir_all(&dir);
    }

    fn index_of(name: &str) -> usize {
        super::super::registry()
            .iter()
            .position(|d| d.name == name)
            .unwrap()
    }

    /// #426, by ADR 0114's one rule: of two user styles of one name, the
    /// one in the `styles/` nearest the scene wins. A scene two folders
    /// down finds a style where the scene-then-parent search found none;
    /// the next scene, in another folder, gets its own nearest, in the same
    /// registry slot; and a nearer file that appears later takes over at
    /// the next reload poll, and gives the name back when it goes.
    #[test]
    fn the_nearest_styles_folder_wins_a_name() {
        let root = std::env::temp_dir().join(format!("flint-user-426-{}", std::process::id()));
        let cave = root.join("levels/act/cave");
        let other = root.join("other");
        std::fs::create_dir_all(&cave).unwrap();
        std::fs::create_dir_all(&other).unwrap();
        std::fs::write(root.join("flint.toml"), "").unwrap();
        let write = |folder: &str, k: f32| {
            let styles = root.join(folder).join("styles");
            std::fs::create_dir_all(&styles).unwrap();
            std::fs::write(
                styles.join("utest_near.shader.toml"),
                format!(
                    "version = 1\nname = \"utest_near\"\nshader = \"near.wgsl\"\n\
                     [params.k]\ndefault = {k}\nmin = 0\nmax = 9\n"
                ),
            )
            .unwrap();
            std::fs::write(styles.join("near.wgsl"), PASSTHROUGH).unwrap();
            styles.join("utest_near.shader.toml")
        };
        let at_root = write("", 1.5);
        let in_levels = write("levels", 2.5);
        let k = || {
            super::super::find("utest_near")
                .unwrap()
                .param(&Default::default(), "k")
        };
        let from = || loaded("utest_near").unwrap().source.toml_path.clone();
        let cave_scope = AssetScope::for_dir(&cave);
        let other_scope = AssetScope::for_dir(&other);
        // What the loader takes is what the resolver lists
        let listed = cave_scope.list(AssetKind::Styles);
        assert_eq!(listed.len(), 1);
        assert!(flint_core::project::same_path(&listed[0].path, &in_levels));

        assert!(load_for(&cave_scope).is_empty());
        assert_eq!(k(), 2.5, "levels/styles is nearer the cave than the root's");
        assert!(flint_core::project::same_path(&from(), &in_levels));
        let index = index_of("utest_near");

        // Another scene, whose nearest is the root's
        assert!(load_for(&other_scope).is_empty());
        assert_eq!(k(), 1.5);
        assert!(flint_core::project::same_path(&from(), &at_root));
        assert_eq!(index_of("utest_near"), index, "one slot for the name");
        // ... and back, every time, in whichever order scenes load
        assert!(load_for(&cave_scope).is_empty());
        assert_eq!(k(), 2.5);
        assert!(load_for(&cave_scope).is_empty());
        assert_eq!(k(), 2.5);

        // A nearer one appears while the scene is open
        assert!(!reload_changed_for(&cave_scope).changed());
        let in_act = write("levels/act", 3.5);
        let reload = reload_changed_for(&cave_scope);
        assert_eq!(reload.replaced, vec!["utest_near".to_string()]);
        assert!(reload.added.is_empty());
        assert_eq!(k(), 3.5);
        assert_eq!(index_of("utest_near"), index);
        assert!(!reload_changed_for(&cave_scope).changed(), "settled");
        // It goes: the next one out has the name again
        std::fs::remove_file(&in_act).unwrap();
        let reload = reload_changed_for(&cave_scope);
        assert_eq!(reload.replaced, vec!["utest_near".to_string()]);
        assert_eq!(k(), 2.5);
        let _ = std::fs::remove_dir_all(&root);
    }

    /// #275: an edited `.shader.toml` replaces its style at the same
    /// registry index with the new parameters; a broken edit keeps the old
    /// one and reports; a new file in the folder is picked up; and the
    /// scene's settings are read again for what changed.
    #[test]
    fn toml_edits_replace_the_style_in_place() {
        let dir = std::env::temp_dir().join(format!("flint-user-toml-{}", std::process::id()));
        let styles = dir.join("styles");
        std::fs::create_dir_all(&styles).unwrap();
        let toml_path = styles.join("utest_edit.shader.toml");
        std::fs::write(
            &toml_path,
            "version = 1\nname = \"utest_edit\"\nshader = \"edit.wgsl\"\n\
             [params.a]\ndefault = 0.5\nmin = 0\nmax = 1\n",
        )
        .unwrap();
        std::fs::write(styles.join("edit.wgsl"), PASSTHROUGH).unwrap();
        assert!(load_dir(&dir).is_empty());
        let index = index_of("utest_edit");
        let old = super::super::find("utest_edit").unwrap();
        let old_bind = loaded("utest_edit").unwrap().names.passes[0].frame;
        assert!(
            reload_changed(&dir).replaced.is_empty(),
            "nothing changed yet"
        );

        // A new parameter, a new label
        std::thread::sleep(std::time::Duration::from_millis(30));
        std::fs::write(
            &toml_path,
            "version = 1\nname = \"utest_edit\"\nlabel = \"Edited\"\nshader = \"edit.wgsl\"\n\
             [params.a]\ndefault = 0.5\nmin = 0\nmax = 1\n\
             [params.b]\ndefault = 2\nmin = 0\nmax = 4\n",
        )
        .unwrap();
        let reload = reload_changed(&dir);
        assert_eq!(reload.replaced, vec!["utest_edit".to_string()]);
        assert!(reload.changed());
        let new = super::super::find("utest_edit").unwrap();
        assert!(!std::ptr::eq(old, new), "a new descriptor");
        assert_eq!(index_of("utest_edit"), index, "at the same index");
        // Its bind groups are cached under new names: the old ones hold the
        // old pass's buffers
        assert_ne!(
            loaded("utest_edit").unwrap().names.passes[0].frame,
            old_bind
        );
        assert_eq!(new.label, "Edited");
        assert_eq!(new.param(&Default::default(), "b"), 2.0);
        assert_eq!(super::source("utest_edit").unwrap().params.len(), 2);

        // The scene's table, read again, now keeps `b`
        let tables: BTreeMap<String, toml::Table> =
            toml::from_str("[utest_edit]\nb = 3\n").unwrap();
        let (styles_read, warnings) = super::super::StyleSettings::from_tables(&tables);
        assert!(warnings.is_empty(), "{warnings:?}");
        let authored = crate::postprocess::PostProcessConfig {
            styles: styles_read,
            ..Default::default()
        };
        let mut live = crate::postprocess::PostProcessConfig::default();
        live.styles.set_enabled("invert", true); // the host's own edit, kept
        refresh_settings(&mut live, &authored, &reload);
        assert_eq!(live.styles.param("utest_edit", "b"), Some(3.0));
        assert!(live.styles.is_enabled("utest_edit"));
        assert!(live.styles.is_enabled("invert"));

        // A broken edit keeps the style as it was and says why
        std::thread::sleep(std::time::Duration::from_millis(30));
        std::fs::write(
            &toml_path,
            "version = 1\nname = \"utest_edit\"\nshader = \n",
        )
        .unwrap();
        let reload = reload_changed(&dir);
        assert!(reload.replaced.is_empty());
        assert_eq!(reload.messages.len(), 1, "{:?}", reload.messages);
        assert!(std::ptr::eq(super::super::find("utest_edit").unwrap(), new));
        assert!(errors().iter().any(|(n, _)| n == "utest_edit"));
        // ... and isn't read again until it changes once more
        assert!(reload_changed(&dir).messages.is_empty());

        // A new file is added
        std::fs::write(
            styles.join("utest_added.shader.toml"),
            "version = 1\nname = \"utest_added\"\nshader = \"edit.wgsl\"\n",
        )
        .unwrap();
        let reload = reload_changed(&dir);
        assert_eq!(reload.added, vec!["utest_added".to_string()]);
        assert!(super::super::find("utest_added").is_some());
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// #275: the renderer's [`StyleSet`](super::super::StyleSet) throws a
    /// pass away when its descriptor is replaced, and builds the new one.
    #[test]
    fn a_replaced_descriptor_is_built_again() {
        let Some((device, _queue)) = device() else {
            crate::gpu_select::skip_without_gpu("no GPU adapter");
            return;
        };
        let dir = std::env::temp_dir().join(format!("flint-user-set-{}", std::process::id()));
        let styles = dir.join("styles");
        std::fs::create_dir_all(&styles).unwrap();
        let toml_path = styles.join("utest_set.shader.toml");
        std::fs::write(
            &toml_path,
            "version = 1\nname = \"utest_set\"\nshader = \"set.wgsl\"\n",
        )
        .unwrap();
        std::fs::write(styles.join("set.wgsl"), PASSTHROUGH).unwrap();
        assert!(load_dir(&dir).is_empty());
        let mut config = crate::postprocess::PostProcessConfig {
            enabled: true,
            ..Default::default()
        };
        config.styles.set_enabled("utest_set", true);
        let ids = crate::style_mask::StyleIds::default();
        let format = wgpu::TextureFormat::Rgba8UnormSrgb;
        let mut set = super::super::StyleSet::default();
        assert!(set.ensure(&device, format, &config, &ids).is_empty());
        let active = |set: &super::super::StyleSet| {
            set.active(StyleStage::Display, &config, &ids)
                .iter()
                .filter(|(d, _)| d.name == "utest_set")
                .map(|(d, _)| *d as *const StyleDescriptor)
                .collect::<Vec<_>>()
        };
        let before = active(&set);
        assert_eq!(before.len(), 1);

        std::thread::sleep(std::time::Duration::from_millis(30));
        std::fs::write(
            &toml_path,
            "version = 1\nname = \"utest_set\"\nshader = \"set.wgsl\"\n\
             [params.k]\ndefault = 1\nmin = 0\nmax = 1\n",
        )
        .unwrap();
        assert_eq!(reload_changed(&dir).replaced.len(), 1);
        assert!(active(&set).is_empty(), "the stale pass isn't drawn");
        assert!(set.ensure(&device, format, &config, &ids).is_empty());
        let after = active(&set);
        assert_eq!(after.len(), 1);
        assert_ne!(before, after, "built from the new descriptor");
        let _ = std::fs::remove_dir_all(&dir);
    }

    fn printed() -> StyleSource {
        source(
            r#"
            version = 1
            name = "printed"
            shader = "printed.wgsl"
            [params.grain]
            default = 1.0
            min = 0.0
            max = 4.0
            [options.finish]
            default = "matte"
            choices = ["matte", "gloss", "semi-gloss"]
            [options.grade]
            kind = "image"
            default = "luts/warm.png"
            choices = ["luts/warm.png", "luts/cold.png"]
            [images.paper]
            file = "paper.png"
            srgb = true
            "#,
            "styles/printed.shader.toml",
        )
        .unwrap()
    }

    /// #413: options and images in a description, and what it may not say.
    #[test]
    fn options_and_images_are_checked() {
        let s = printed();
        let names: Vec<&str> = s.options.iter().map(|(n, _)| n.as_str()).collect();
        assert_eq!(names, ["finish", "grade"], "uniform order is by name");
        assert_eq!(s.uniform_fields(), 3);
        let (_, finish) = &s.options[0];
        assert_eq!(finish.kind, OptionKind::Choice);
        assert_eq!(finish.index("gloss"), 1);
        assert_eq!(
            finish.index("plaid"),
            0,
            "an unknown value draws as the default"
        );
        let images: Vec<(&str, Option<&str>, bool)> = s
            .images
            .iter()
            .map(|i| (i.name.as_str(), i.file.as_deref(), i.srgb))
            .collect();
        assert_eq!(
            images,
            [("grade", None, false), ("paper", Some("paper.png"), true)],
            "binding order is by name; an image option binds too"
        );

        let base = "version = 1\nname = \"x\"\nshader = \"x.wgsl\"\n";
        let file = "styles/x.shader.toml";
        let err = |extra: &str| source(&format!("{base}{extra}"), file).unwrap_err();
        assert!(err("[options.m]\ndefault = \"a\"\n").contains("lists its choices"));
        assert!(
            err("[options.m]\ndefault = \"c\"\nchoices = [\"a\", \"b\"]\n")
                .contains("not one of its choices")
        );
        assert!(err("[options.m]\ndefault = \"a\"\nchoices = [\"a\", \"a\"]\n").contains("twice"));
        assert!(
            err("[options.m]\nkind = \"font\"\ndefault = \"a\"\nchoices = [\"a\"]\n")
                .contains("\"choice\" or \"image\"")
        );
        assert!(
            err("[options.m]\ndefault = \"a\"\nchoices = [\"a\"]\nsrgb = true\n")
                .contains("image option")
        );
        assert!(err("[images.p]\nfile = \"../p.png\"\n").contains("no .."));
        assert!(err("[images.p]\nfile = \"p.hdr\"\n").contains("png, jpg"));
        assert!(err("[images.flint_p]\nfile = \"p.png\"\n").contains("flint"));
        assert!(
            err("[images.p]\nfile = \"p.png\"\n[params.p]\ndefault = 0\nmin = 0\nmax = 1\n")
                .contains("parameter's name")
        );
        assert!(err(
            "[images.p]\nfile = \"p.png\"\n[options.p]\nkind = \"image\"\ndefault = \"p.png\"\n"
        )
        .contains("option's name"));
        assert!(err("[options.enabled]\ndefault = \"a\"\nchoices = [\"a\"]\n").contains("taken"));
        let many: String = (0..=MAX_IMAGES)
            .map(|i| format!("[images.i{i}]\nfile = \"i.png\"\n"))
            .collect();
        assert!(err(&many).contains("at most"));
    }

    /// #413 with #411: a style with images, options and a loop is written
    /// back out guarded, and the guarded module still validates with its
    /// group 3 images and option constants.
    #[test]
    fn guarded_images_and_options_validate() {
        let s = printed();
        let user = r#"
fn style(p: StylePixel) -> vec4<f32> {
    var acc = vec3<f32>(0.0);
    for (var i = 0; i < 4; i++) {
        acc += flint_image_tiled_paper(p.uv + vec2<f32>(f32(i)) * flint.texel).rgb;
    }
    var c = flint_lut_grade(p.color.rgb) * acc * 0.25;
    if (params.finish == FLINT_FINISH_GLOSS) {
        c *= params.grain;
    }
    return vec4<f32>(c, 1.0);
}
"#;
        let report = check(&s, user).unwrap_or_else(|e| panic!("{e}"));
        let guarded = report.guarded.expect("a loop is guarded");
        assert!(guarded.contains("@group(3) @binding(1)"), "{guarded}");
        let module = naga::front::wgsl::parse_str(&guarded).unwrap_or_else(|e| panic!("{e}"));
        naga::valid::Validator::new(
            naga::valid::ValidationFlags::all(),
            naga::valid::Capabilities::default(),
        )
        .validate(&module)
        .unwrap_or_else(|e| panic!("{e:?}"));
    }

    /// #413 with #414: a per-entity style's options sit in every row of its
    /// settings table, after the floats, and its prelude validates.
    #[test]
    fn per_entity_options_ride_in_each_row() {
        let s = source(
            "version = 1\nname = \"x\"\nshader = \"x.wgsl\"\ninputs = [\"mask\"]\n\
             [params.a]\ndefault = 0.5\nmin = 0\nmax = 1\n\
             [options.finish]\ndefault = \"matte\"\nchoices = [\"matte\", \"gloss\"]\n\
             [images.paper]\nfile = \"p.png\"\n",
            "styles/x.shader.toml",
        )
        .unwrap();
        let user = "fn style(p: StylePixel) -> vec4<f32> {\n    \
                    let n = flint_params_at(p.uv + flint.texel);\n    \
                    let gloss = f32(params.finish == FLINT_FINISH_GLOSS) + f32(n.finish);\n    \
                    return flint_image_paper(p.uv) * params.a * gloss;\n}\n";
        check(&s, user).unwrap_or_else(|e| panic!("{e}"));
        let rows = vec![vec![0.5, f32::from_bits(0)], vec![0.25, f32::from_bits(1)]];
        let words = table_words(s.uniform_fields(), &rows, Some(&[0u32; MAX_IDS]));
        let stride = params_size(s.uniform_fields()) / 4;
        assert_eq!(words[1], 0, "row 0: matte");
        assert_eq!(words[stride + 1], 1, "row 1: gloss");
        assert_eq!(f32::from_bits(words[stride]), 0.25);
    }

    /// #413: the prelude declares each image's helpers and each option's
    /// index and constants, and naga takes a shader that uses them all.
    #[test]
    fn images_and_options_validate() {
        let s = printed();
        let prelude = prelude(&s);
        assert!(prelude.contains("finish: u32,"), "{prelude}");
        assert!(prelude.contains("const FLINT_FINISH_GLOSS: u32 = 1u;"));
        assert!(prelude.contains("const FLINT_FINISH_SEMI_GLOSS: u32 = 2u;"));
        assert!(prelude.contains("@group(3) @binding(0) var flint_image_texture_grade"));
        assert!(prelude.contains("@group(3) @binding(1) var flint_image_texture_paper"));
        let user = r#"
fn style(p: StylePixel) -> vec4<f32> {
    var c = flint_lut_grade(p.color.rgb);
    let paper = flint_image_tiled_paper(p.pos / flint_image_size_paper());
    if (params.finish == FLINT_FINISH_GLOSS) {
        c = c * 1.1;
    }
    let edge = flint_image_paper(p.uv).a;
    return vec4<f32>(c * paper.rgb * edge * params.grain, 1.0);
}
"#;
        let report = check(&s, user).unwrap_or_else(|e| panic!("{e}"));
        // LUT 2 + tiled 4 + clamped 1 + size 0
        assert_eq!(report.texture_reads, 7);

        // An image the style doesn't declare: named, with the ones it has
        let user = "fn style(p: StylePixel) -> vec4<f32> {\n    \
                    return flint_image_brush(p.uv);\n}\n";
        let e = check(&s, user).unwrap_err();
        assert!(
            e.contains("[images.<name>]") && e.contains("grade, paper"),
            "{e}"
        );
    }

    /// #413: an image option with no list offers its default's folder; the
    /// list is fixed at load, so a scene naming another file is warned
    /// (#318) and `--style` refuses it.
    #[test]
    fn image_options_find_their_folder_and_warn_on_strangers() {
        let dir = std::env::temp_dir().join(format!("flint-user-images-{}", std::process::id()));
        let styles = dir.join("styles");
        std::fs::create_dir_all(styles.join("papers")).unwrap();
        let png = |path: &Path| {
            image::RgbaImage::from_pixel(2, 2, image::Rgba([200, 180, 150, 255]))
                .save(path)
                .unwrap()
        };
        png(&styles.join("papers").join("cotton.png"));
        png(&styles.join("papers").join("kraft.png"));
        std::fs::write(styles.join("papers").join("notes.txt"), "not an image").unwrap();
        std::fs::write(
            styles.join("utest_paper.shader.toml"),
            "version = 1\nname = \"utest_paper\"\nshader = \"paper.wgsl\"\n\
             [options.paper]\nkind = \"image\"\ndefault = \"papers/cotton.png\"\n\
             [images.missing]\nfile = \"nowhere.png\"\n",
        )
        .unwrap();
        std::fs::write(styles.join("paper.wgsl"), PASSTHROUGH).unwrap();
        let messages = load_dir(&dir);
        assert_eq!(messages.len(), 1, "{messages:?}");
        assert!(
            messages[0].contains("nowhere.png") && messages[0].contains("image not found"),
            "{messages:?}"
        );
        assert!(errors()
            .iter()
            .any(|(n, e)| n == "utest_paper" && e.contains("nowhere.png")));
        let d = super::super::find("utest_paper").unwrap();
        let option = d.options.iter().find(|o| o.name == "paper").unwrap();
        assert_eq!(option.choices, ["papers/cotton.png", "papers/kraft.png"]);
        assert!(option.closed());

        let tables: BTreeMap<String, toml::Table> =
            toml::from_str("[utest_paper]\npaper = \"papers/kraft.png\"\n").unwrap();
        let (settings, warnings) = super::super::StyleSettings::from_tables(&tables);
        assert!(warnings.is_empty(), "{warnings:?}");
        assert_eq!(
            settings.option("utest_paper", "paper"),
            Some("papers/kraft.png")
        );
        let tables: BTreeMap<String, toml::Table> =
            toml::from_str("[utest_paper]\npaper = \"papers/linen.png\"\n").unwrap();
        let (_, warnings) = super::super::StyleSettings::from_tables(&tables);
        assert_eq!(warnings.len(), 1, "{warnings:?}");
        assert!(
            warnings[0].contains("unknown value 'papers/linen.png'")
                && warnings[0].contains("papers/kraft.png"),
            "{warnings:?}"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// #413 on a real device: images load when first drawn, follow the
    /// scene's choice, read again when their file changes, and keep the
    /// last good picture when a file breaks.
    #[test]
    fn images_load_follow_choices_and_reload() {
        let Some((device, queue)) = device() else {
            crate::gpu_select::skip_without_gpu("no GPU adapter");
            return;
        };
        let dir = std::env::temp_dir().join(format!("flint-user-image-gpu-{}", std::process::id()));
        let styles = dir.join("styles");
        std::fs::create_dir_all(styles.join("luts")).unwrap();
        let png = |path: &Path, w: u32, v: u8| {
            image::RgbaImage::from_pixel(w, 1, image::Rgba([v, v, v, 255]))
                .save(path)
                .unwrap()
        };
        png(&styles.join("paper.png"), 4, 200);
        png(&styles.join("luts").join("a.png"), 2, 10);
        png(&styles.join("luts").join("b.png"), 2, 20);
        let toml_path = styles.join("utest_imgs.shader.toml");
        std::fs::write(
            &toml_path,
            "version = 1\nname = \"utest_imgs\"\nshader = \"imgs.wgsl\"\n\
             [options.grade]\nkind = \"image\"\ndefault = \"luts/a.png\"\n\
             [images.paper]\nfile = \"paper.png\"\n",
        )
        .unwrap();
        let source = read_source(&toml_path).unwrap();
        let mut set = ImageSet::new(&device, &source).unwrap();
        let choice = std::cell::RefCell::new(String::new());
        let choose = |_: &str| choice.borrow().clone();

        let first = set.update(&device, &queue, &choose, false);
        assert!(first.ready && first.error.is_none(), "{first:?}");
        assert_eq!(first.loaded, ["luts/a.png", "paper.png"]);
        set.bind_group(&device);
        assert!(set.bind.is_some());
        let again = set.update(&device, &queue, &choose, true);
        assert!(again.loaded.is_empty(), "nothing changed: {again:?}");
        assert!(set.bind.is_some(), "the bind group is kept");

        // The scene picks the other LUT
        *choice.borrow_mut() = "luts/b.png".into();
        let picked = set.update(&device, &queue, &choose, false);
        assert_eq!(picked.loaded, ["luts/b.png"]);
        assert!(set.bind.is_none(), "bound again over the new view");

        // The paper changes on disk: read again at the next poll only
        std::thread::sleep(std::time::Duration::from_millis(30));
        png(&styles.join("paper.png"), 8, 120);
        assert!(set
            .update(&device, &queue, &choose, false)
            .loaded
            .is_empty());
        assert_eq!(
            set.update(&device, &queue, &choose, true).loaded,
            ["paper.png"]
        );

        // A broken paper keeps the last good one and says so, once
        std::thread::sleep(std::time::Duration::from_millis(30));
        std::fs::write(styles.join("paper.png"), b"not a png").unwrap();
        let broken = set.update(&device, &queue, &choose, true);
        assert!(broken.ready, "the last good paper still binds");
        assert!(broken
            .error
            .as_deref()
            .is_some_and(|e| e.contains("paper.png")));
        assert!(set.update(&device, &queue, &choose, true).loaded.is_empty());
        std::thread::sleep(std::time::Duration::from_millis(30));
        png(&styles.join("paper.png"), 4, 90);
        let fixed = set.update(&device, &queue, &choose, true);
        assert_eq!(fixed.loaded, ["paper.png"]);
        assert!(fixed.error.is_none());

        // A missing image never loaded: not ready (the pass draws through)
        let mut source = source;
        source.images.push(ImageSource {
            name: "zz".into(),
            file: Some("gone.png".into()),
            srgb: false,
        });
        let mut set = ImageSet::new(&device, &source).unwrap();
        let missing = set.update(&device, &queue, &choose, false);
        assert!(!missing.ready);
        assert!(missing
            .error
            .as_deref()
            .is_some_and(|e| e.contains("gone.png")));
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// #275: a web bundle is a `MemorySource`; styles load from it through
    /// the vfs exactly as from disk, and a shader missing from it is an
    /// error (`flint bundle` reports it) rather than a crash.
    #[test]
    fn styles_load_from_a_bundle() {
        let source: flint_core::vfs::MemorySource = [
            (
                "styles/utest_bundled.shader.toml",
                "version = 1\nname = \"utest_bundled\"\nshader = \"bundled.wgsl\"\n\
                 [params.k]\ndefault = 0.5\nmin = 0\nmax = 1\n"
                    .as_bytes()
                    .to_vec(),
            ),
            ("styles/bundled.wgsl", PASSTHROUGH.as_bytes().to_vec()),
            (
                "styles/utest_unpacked.shader.toml",
                "version = 1\nname = \"utest_unpacked\"\nshader = \"missing.wgsl\"\n"
                    .as_bytes()
                    .to_vec(),
            ),
            ("levels/a.scene.toml", b"[scene]\nname = \"a\"\n".to_vec()),
        ]
        .into_iter()
        .collect();
        let messages = flint_core::vfs::scoped(Arc::new(source), || load_dir(Path::new("levels")));
        assert_eq!(messages.len(), 1, "{messages:?}");
        assert!(messages[0].contains("missing.wgsl"), "{messages:?}");
        let bundled = super::super::find("utest_bundled").expect("found via ../styles");
        assert_eq!(bundled.param(&Default::default(), "k"), 0.5);
        assert!(errors().iter().any(|(n, _)| n == "utest_unpacked"));
        assert!(!errors().iter().any(|(n, _)| n == "utest_bundled"));
    }

    /// `flint style new` (#416): every stage and set of inputs writes a
    /// description that checks and a shader that compiles with no warning,
    /// lists exactly the helpers its inputs give, and runs a check from
    /// the files on disk.
    #[test]
    fn templates_check_for_every_stage_and_input() {
        for stage in [StyleStage::Display, StyleStage::Hdr] {
            for bits in 0..8u8 {
                let inputs = Inputs {
                    depth: bits & 1 != 0,
                    mask: bits & 2 != 0,
                    edges: bits & 4 != 0,
                };
                let t = template("utest_new", stage, inputs);
                let path = Path::new("styles/utest_new.shader.toml");
                let source = parse_source(&t.toml, path).unwrap();
                assert_eq!(source.stage, stage);
                assert_eq!(source.inputs, inputs);
                assert_eq!(source.params.len(), 1);
                let report = check(&source, &t.wgsl).unwrap_or_else(|e| panic!("{e}\n{}", t.wgsl));
                assert!(report.warnings.is_empty(), "{:?}", report.warnings);
                assert_eq!(t.wgsl.contains("flint_view_depth"), inputs.depth);
                assert_eq!(t.wgsl.contains("flint_style_blend"), inputs.mask);
                assert_eq!(t.wgsl.contains("flint_edge("), inputs.edges);
                // Uncommenting the input lines still compiles
                let live = t.wgsl.replace("    // let ", "    let ");
                check(&source, &live).unwrap_or_else(|e| panic!("{e}\n{live}"));
            }
        }
        // From disk, as `flint style check` runs it
        let dir = std::env::temp_dir().join(format!("flint-style-new-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let t = template("utest_disk", StyleStage::Display, Inputs::default());
        let toml_path = dir.join("utest_disk.shader.toml");
        std::fs::write(&toml_path, &t.toml).unwrap();
        std::fs::write(dir.join("utest_disk.wgsl"), &t.wgsl).unwrap();
        assert!(check_file(&toml_path).unwrap().warnings.is_empty());
        std::fs::write(
            dir.join("utest_disk.wgsl"),
            "fn style(p: StylePixel) -> vec4<f32> {\n    retrun p.color;\n}\n",
        )
        .unwrap();
        let err = check_file(&toml_path).unwrap_err();
        assert!(err.contains("utest_disk.wgsl:2:"), "{err}");
        std::fs::remove_file(dir.join("utest_disk.wgsl")).unwrap();
        assert!(check_file(&toml_path)
            .unwrap_err()
            .contains("utest_disk.wgsl"));
        let _ = std::fs::remove_dir_all(&dir);
        // A built-in's name is refused before anything is written
        let t = template("ascii", StyleStage::Display, Inputs::default());
        assert!(parse_source(&t.toml, Path::new("styles/ascii.shader.toml"))
            .unwrap_err()
            .contains("built-in"));
    }
}
