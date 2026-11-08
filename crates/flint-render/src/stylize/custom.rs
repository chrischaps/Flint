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
    StyleParam, StylePass, StyleStage,
};
use bytemuck::{Pod, Zeroable};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, RwLock};

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
    /// Bloom reads this style's result (#300); hdr styles only.
    #[serde(default)]
    pub feeds_bloom: bool,
    /// `[[passes]]` (#412): each a WGSL `fn style` of its own, run in
    /// order. Every pass but the last writes a target of the style's own;
    /// the last writes the stage's output.
    #[serde(default)]
    pub passes: Vec<PassDef>,
}

/// One `[[passes]]` entry.
#[derive(Debug, Clone, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PassDef {
    /// For errors, F4 and `--gpu-time`; `pass1`, `pass2`... when absent.
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

/// The formats a target can be.
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
/// resolution.
pub const MAX_TARGETS: usize = 4;

/// What a style's targets may cost together, in bytes for each pixel of
/// its stage: four full-size `rgba16f` targets (66 MB at 1080p).
pub const TARGET_BUDGET: f32 = 32.0;

fn default_stage() -> String {
    "display".into()
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
    /// Parameters in uniform order (by name).
    pub params: Vec<(String, ParamDef)>,
    /// The `.shader.toml`.
    pub toml_path: PathBuf,
    /// Its passes in the order they run (one for a style with a top-level
    /// `shader`); the last writes the stage's output.
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
        if self.params.len() > MAX_PARAMS {
            return Err(format!("{at}: at most {MAX_PARAMS} parameters"));
        }
        let dir = toml_path.parent().unwrap_or(Path::new("."));
        let (passes, targets, notes) = check_passes(&at, dir, self.shader, self.passes)?;
        Ok(StyleSource {
            label: self.label.unwrap_or_else(|| super::humanize(&self.name)),
            name: self.name,
            stage,
            inputs,
            feeds_bloom: self.feeds_bloom,
            params: self.params.into_iter().collect(),
            toml_path: toml_path.to_path_buf(),
            passes,
            targets,
            notes,
        })
    }
}

/// How many parameters a style may declare: 64 floats is a 272-byte
/// uniform, well inside every backend's limit.
pub const MAX_PARAMS: usize = 64;

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
    // this pass's index, 0 first
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
@group(0) @binding(1) var<uniform> params: Params;
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

// How much of this style the entity at uv asks for: 0 where it isn't this
// style's (a `style_mask` naming it), else its blend 0..1 (#278)
fn flint_style_blend(uv: vec2<f32>) -> f32 {
    let m = textureLoad(flint_mask_texture, flint_mask_texel(uv), 0);
    if (flint.mask_id == 0u || m.r != flint.mask_id) {
        return 0.0;
    }
    return f32(m.g) / 65535.0;
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
/// (#412): `flint_target_<name>(uv)` (filtered, or nearest for `r32f`),
/// `flint_target_<name>_load(texel)` and `flint_target_<name>_size()`.
fn prelude_targets(source: &StyleSource, pass: usize) -> String {
    let mut text = String::new();
    for (binding, read) in source.passes[pass].reads.iter().enumerate() {
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

fn epilogue(stage: StyleStage, last: bool) -> String {
    let out = if !last {
        // A target holds what the pass returns, unchanged
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
    var p: StylePixel;
    p.uv = in.uv;
    p.pos = in.position.xy;
    p.color = flint_sample(in.uv);
    {out}
}}
"#
    )
}

/// The prelude for pass `pass` of `source`: everything before the user's
/// first line.
pub fn prelude(source: &StyleSource, pass: usize) -> String {
    let mut text = String::from(PRELUDE_FRAME);
    text.push_str("\nstruct Params {\n");
    for (name, _) in &source.params {
        text.push_str(&format!("    {name}: f32,\n"));
    }
    // Never empty, and the f32s above round up to 16 bytes before it
    text.push_str("    _flint_pad: vec4<f32>,\n};\n");
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
    text.push_str(&prelude_targets(source, pass));
    text.push_str("\n// ---- user style ----\n");
    text
}

/// `user` (the WGSL of pass `pass`) inside its prelude and entry points.
pub fn compose(source: &StyleSource, pass: usize, user: &str) -> Composed {
    let prelude = prelude(source, pass);
    let last = pass + 1 == source.passes.len();
    let prelude_lines = prelude.matches('\n').count();
    let mut user = user.to_string();
    if !user.ends_with('\n') {
        user.push('\n');
    }
    let user_lines = user.matches('\n').count();
    let text = format!("{prelude}{user}{}", epilogue(source.stage, last));
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

// ---------------------------------------------------------------------------
// Checking with naga (native)
// ---------------------------------------------------------------------------

/// What a check found: warnings about a shader that compiles.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Report {
    /// Loops in the user's code: each can hang the GPU if it never ends.
    pub loops: usize,
    /// Texture reads in the user's code (the helpers' own not counted).
    pub texture_reads: usize,
    pub warnings: Vec<String>,
}

/// Texture reads a pixel may make before the check warns (a 9x9 blur is
/// 81): past it, a full-screen pass gets expensive fast.
pub const TEXTURE_READ_WARN: usize = 32;

/// Parse and validate `user` as the shader of `source`'s pass `pass`, the
/// way it will be built. `Err` is the message to show, lines in the user's
/// file.
#[cfg(not(target_arch = "wasm32"))]
pub fn check(source: &StyleSource, pass: usize, user: &str) -> Result<Report, String> {
    let composed = compose(source, pass, user);
    let file = source.passes[pass].wgsl_path.display().to_string();
    let module = naga::front::wgsl::parse_str(&composed.text).map_err(|e| {
        locate(
            &composed,
            &file,
            e.location(&composed.text),
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
            e.location(&composed.text),
            &message,
            source,
            pass,
        )
    })?;
    inspect(&module, &composed, &file)
}

/// `file:line:col: message`, with the line remapped into the user's file
/// and the offending line quoted; a hint when the user called a helper of
/// an input they didn't declare.
#[cfg(not(target_arch = "wasm32"))]
fn locate(
    composed: &Composed,
    file: &str,
    at: Option<naga::SourceLocation>,
    message: &str,
    source: &StyleSource,
    pass: usize,
) -> String {
    let mut out = match at {
        Some(loc) => match composed.user_line(loc.line_number as usize) {
            Some(line) => {
                let text = composed
                    .text
                    .lines()
                    .nth(loc.line_number as usize - 1)
                    .unwrap_or("");
                format!(
                    "{file}:{line}:{}: {message}\n    {line} | {}",
                    loc.line_position,
                    text.trim_end()
                )
            }
            None if message.contains("'style'") => format!(
                "{file}: no `fn style(p: StylePixel) -> vec4<f32>`; the style draws what it returns"
            ),
            None => format!(
                "{file}: {message} (in the generated code, line {}; check the parameter names)",
                loc.line_number
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
    let in_user = |span: naga::Span| {
        span.to_range()
            .is_some_and(|r| r.start >= prelude_end && r.start < user_end)
    };
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
                 (flint_input, params, depth, mask, edges, and the targets in reads)",
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
    if report.loops > 0 {
        report.warnings.push(format!(
            "{file}: warning: {} loop{}: a loop that never ends hangs the GPU until the driver resets it \
             (TDR); keep its bound a constant or a clamped parameter",
            report.loops,
            if report.loops == 1 { "" } else { "s" }
        ));
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
            let target_read = name.starts_with("flint_target_") && !name.ends_with("_size");
            usize::from(
                target_read
                    || matches!(
                        name,
                        "flint_sample"
                            | "flint_depth"
                            | "flint_view_depth"
                            | "flint_world_pos"
                            | "flint_mask_id"
                            | "flint_style_blend"
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

/// On the web there is no naga in the player: the browser's own WGSL
/// compiler reports, through wgpu's error scope.
#[cfg(target_arch = "wasm32")]
pub fn check(_source: &StyleSource, _pass: usize, _user: &str) -> Result<Report, String> {
    Ok(Report::default())
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
    /// The empty group 2 of a style that reads targets but no depth, mask
    /// or edges.
    empty: &'static str,
    passes: Vec<PassNames>,
    /// Each target's texture names: two for history (ping-ponged), else
    /// the same one twice.
    targets: Vec<[&'static str; 2]>,
}

#[derive(Clone, Copy)]
struct PassNames {
    frame: &'static str,
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

/// The folders a scene in `scene_dir` finds styles in: its own `styles/`,
/// then its parent's (a game's `levels/` beside `styles/`), as models are.
pub fn style_dirs(scene_dir: &Path) -> Vec<PathBuf> {
    let mut dirs = vec![scene_dir.join("styles")];
    if let Some(parent) = scene_dir.parent() {
        dirs.push(parent.join("styles"));
    }
    dirs
}

/// Register every `*.shader.toml` in `scene_dir`'s style folders that
/// isn't registered yet. Call it before the scene's `[post_process]` is
/// read, or its tables name an unknown style. Returns one message for each
/// file that couldn't be loaded (and each warning a check raised); a
/// style whose shader fails still registers, drawing a passthrough.
pub fn load_dir(scene_dir: &Path) -> Vec<String> {
    let mut messages = Vec::new();
    for dir in style_dirs(scene_dir) {
        let Ok(mut entries) = flint_core::vfs::read_dir(&dir) else {
            continue;
        };
        entries.sort();
        for path in entries {
            let is_style = path
                .file_name()
                .and_then(|f| f.to_str())
                .is_some_and(|f| f.ends_with(SUFFIX));
            if is_style {
                if let Err(e) = load_file(&path, &mut messages) {
                    messages.push(e);
                }
            }
        }
    }
    messages
}

/// Register the style `path` describes (see [`load_dir`]).
pub fn load_file(path: &Path, messages: &mut Vec<String>) -> Result<(), String> {
    let source = read_source(path)?;
    if let Some(existing) = loaded(&source.name) {
        if existing.source.toml_path != source.toml_path {
            messages.push(format!(
                "{}: style \"{}\" is already loaded from {}; this one is skipped",
                path.display(),
                source.name,
                existing.source.toml_path.display()
            ));
        }
        return Ok(());
    }
    install(source, messages, false);
    Ok(())
}

/// Read and check the description at `path` (not its shader).
fn read_source(path: &Path) -> Result<StyleSource, String> {
    let text =
        flint_core::vfs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let def: ShaderStyleDef =
        toml::from_str(&text).map_err(|e| format!("{}: {e}", path.display()))?;
    def.check(path)
}

/// The modified time of `path` on disk, for the reload polls (native
/// only: hot reload reads the disk, never the vfs, and a bundle never
/// changes).
#[cfg(not(target_arch = "wasm32"))]
fn modified(path: &Path) -> Option<std::time::SystemTime> {
    std::fs::metadata(path).and_then(|m| m.modified()).ok()
}

/// Make `source`'s descriptor and register it: appended, or with
/// `replace`, put where the style of the same name stands (#275).
fn install(source: StyleSource, messages: &mut Vec<String>, replace: bool) {
    // A shader that fails registers anyway: it draws a passthrough and its
    // error waits for the host, so a typo never takes a style's settings away
    messages.extend(source.notes.iter().cloned());
    let pass_errors: Vec<Option<String>> = source
        .passes
        .iter()
        .enumerate()
        .map(
            |(i, pass)| match flint_core::vfs::read_to_string(&pass.wgsl_path) {
                Err(e) => Some(format!("{}: {e}", pass.wgsl_path.display())),
                Ok(user) => match check(&source, i, &user) {
                    Ok(report) => {
                        messages.extend(report.warnings);
                        None
                    }
                    Err(e) => Some(e),
                },
            },
        )
        .collect();
    let error = join_errors(&pass_errors);
    messages.extend(pass_errors.into_iter().flatten());
    let leak = |s: String| -> &'static str { Box::leak(s.into_boxed_str()) };
    let params: Vec<StyleParam> = source
        .params
        .iter()
        .map(|(name, p)| StyleParam::new(leak(name.clone()), p.default, p.min..=p.max))
        .collect();
    let hints: Vec<StyleHint> = source
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
    let name = leak(source.name.clone());
    let descriptor: &'static StyleDescriptor = Box::leak(Box::new(
        StyleDescriptor::new(name, leak(source.label.clone()), source.stage, create)
            .params(Box::leak(params.into_boxed_slice()))
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
            .feeds_bloom(source.feeds_bloom),
    ));
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
        empty: leak(format!("{name} style empty BG #{generation}")),
        passes: source
            .passes
            .iter()
            .map(|p| PassNames {
                frame: leak(format!("{name} style {} frame BG #{generation}", p.name)),
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

/// Hot reload of the descriptions (#275), natively: read again every
/// loaded `.shader.toml` whose modified time moved, and load any new one
/// in `scene_dir`'s style folders. A description that no longer reads
/// keeps the style as it was and shows the error ([`errors`]); one that
/// does replaces the style at its registry index, so the renderer builds
/// it again on the next frame. The WGSL itself is polled by the pass, so
/// a shader edit needs nothing from the host.
///
/// Costs one `stat` a loaded style and one directory listing a style
/// folder: call it a few times a second, not every frame. On the web it
/// does nothing.
pub fn reload_changed(scene_dir: &Path) -> Reload {
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
        let (before, known): (Vec<String>, Vec<PathBuf>) = LOADED
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .iter()
            .map(|l| (l.source.name.clone(), l.source.toml_path.clone()))
            .unzip();
        // New files; one that failed is tried again only once it changes,
        // so a broken new file reports once, not four times a second
        static REFUSED: Mutex<BTreeMap<PathBuf, Option<std::time::SystemTime>>> =
            Mutex::new(BTreeMap::new());
        for dir in style_dirs(scene_dir) {
            let Ok(mut entries) = std::fs::read_dir(&dir).map(|rd| {
                rd.filter_map(|e| e.ok().map(|e| e.path()))
                    .collect::<Vec<_>>()
            }) else {
                continue;
            };
            entries.sort();
            for path in entries {
                let is_style = path
                    .file_name()
                    .and_then(|f| f.to_str())
                    .is_some_and(|f| f.ends_with(SUFFIX));
                if !is_style || known.contains(&path) {
                    continue;
                }
                let now = modified(&path);
                let mut refused = REFUSED.lock().unwrap_or_else(|e| e.into_inner());
                if refused.get(&path) == Some(&now) {
                    continue;
                }
                match load_file(&path, &mut reload.messages) {
                    Ok(()) => {
                        // Skipped (its name is loaded from another folder):
                        // said once, like a failure
                        let took = LOADED
                            .read()
                            .unwrap_or_else(|e| e.into_inner())
                            .iter()
                            .any(|l| l.source.toml_path == path);
                        if took {
                            refused.remove(&path);
                        } else {
                            refused.insert(path, now);
                        }
                    }
                    Err(e) => {
                        reload.messages.push(e);
                        refused.insert(path, now);
                    }
                }
            }
        }
        reload.added = LOADED
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .iter()
            .map(|l| l.source.name.clone())
            .filter(|n| !before.contains(n))
            .collect();
    }
    #[cfg(target_arch = "wasm32")]
    let _ = scene_dir;
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
            let error = l.error.lock().unwrap_or_else(|e| e.into_inner()).clone()?;
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
    /// The pass's pipeline, or `None` while it has never compiled.
    pipeline: Option<wgpu::RenderPipeline>,
    /// The WGSL file's modified time when last read.
    #[cfg(not(target_arch = "wasm32"))]
    modified: Option<std::time::SystemTime>,
}

/// One pass of a user style on the GPU (#412).
struct PassGpu {
    /// What it writes: its target's format, or the stage's.
    format: wgpu::TextureFormat,
    /// Group 3: the targets it reads, when it reads any.
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
    /// Group 2 of a style that reads targets but no depth, mask or edges.
    empty_bgl: Option<wgpu::BindGroupLayout>,
    params_buffer: wgpu::Buffer,
    passes: Vec<PassGpu>,
    /// Draws the input unchanged, while a pass can't build.
    passthrough: wgpu::RenderPipeline,
    history: Mutex<History>,
    /// Frames since the WGSL files were last looked at.
    #[cfg(not(target_arch = "wasm32"))]
    since_poll: Mutex<u32>,
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
    let extra_bgl = source.inputs.any().then(|| {
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
    let reads_targets = source.passes.iter().any(|p| !p.reads.is_empty());
    let empty_bgl = (reads_targets && extra_bgl.is_none()).then(|| {
        device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("User Style Empty BGL"),
            entries: &[],
        })
    });
    let passes: Vec<PassGpu> = source
        .passes
        .iter()
        .map(|p| {
            let targets_bgl = (!p.reads.is_empty()).then(|| {
                let entries: Vec<wgpu::BindGroupLayoutEntry> = p
                    .reads
                    .iter()
                    .enumerate()
                    .map(|(b, r)| {
                        texture_entry(
                            b as u32,
                            wgpu::TextureSampleType::Float {
                                filterable: source.targets[r.target].format.filterable(),
                            },
                        )
                    })
                    .collect();
                device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                    label: Some("User Style Targets BGL"),
                    entries: &entries,
                })
            });
            let mut layouts = vec![&frame_bgl, &input_bgl];
            match (&extra_bgl, &empty_bgl, &targets_bgl) {
                (Some(extra), _, _) => layouts.push(extra),
                (None, Some(empty), Some(_)) => layouts.push(empty),
                _ => {}
            }
            if let Some(t) = &targets_bgl {
                layouts.push(t);
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
                    #[cfg(not(target_arch = "wasm32"))]
                    modified: None,
                }),
                error: Mutex::new(None),
            }
        })
        .collect();
    // The passthrough is the last pass's prelude with no inputs and no
    // targets: always compiles
    let last = source.passes.last().expect("a style has a pass").clone();
    let bare = StyleSource {
        inputs: Inputs::default(),
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
    let passthrough = pipeline(
        device,
        &passthrough_layout,
        &compose(&bare, 0, PASSTHROUGH).text,
        d.format,
    );
    let pass = CustomPass {
        params_buffer: device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("User Style Params"),
            size: params_size(source.params.len()) as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        }),
        frame_bgl,
        input_bgl,
        extra_bgl,
        empty_bgl,
        passes,
        passthrough,
        history: Mutex::new(History::default()),
        #[cfg(not(target_arch = "wasm32"))]
        since_poll: Mutex::new(0),
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
) -> wgpu::RenderPipeline {
    let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("User Style Shader"),
        source: wgpu::ShaderSource::Wgsl(text.into()),
    });
    device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
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
    })
}

/// Clear `view` to transparent black: a history target whose past isn't
/// this picture's reads as nothing.
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

/// How many frames pass between looks at the WGSL files' modified times:
/// about four times a second at 60 fps, and one `stat` a pass each.
#[cfg(not(target_arch = "wasm32"))]
const POLL_FRAMES: u32 = 15;

impl CustomPass {
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
        let modified = std::fs::metadata(path).and_then(|m| m.modified()).ok();
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
        if let Err(e) = check(source, i, &user) {
            drop(build);
            self.set_error(i, Some(e));
            return;
        }
        // naga has passed it; the error scope catches what the backend's own
        // compiler still refuses (and, on the web, everything)
        device.push_error_scope(wgpu::ErrorFilter::Validation);
        let made = pipeline(
            device,
            &gpu.layout,
            &compose(source, i, &user).text,
            gpu.format,
        );
        #[cfg(not(target_arch = "wasm32"))]
        let refused = pollster::block_on(device.pop_error_scope());
        #[cfg(target_arch = "wasm32")]
        let refused: Option<wgpu::Error> = {
            // No blocking on the web: the browser logs it, and the frame
            // after shows what it drew
            drop(device.pop_error_scope());
            None
        };
        match refused {
            Some(e) => {
                drop(build);
                self.set_error(
                    i,
                    Some(format!("{}: the GPU refused it: {e}", path.display())),
                );
            }
            None => {
                build.pipeline = Some(made);
                drop(build);
                self.set_error(i, None);
            }
        }
    }

    /// Rebuild each pass whose WGSL file changed on disk (native only).
    #[cfg(not(target_arch = "wasm32"))]
    fn poll(&self, device: &wgpu::Device) {
        {
            let mut since = self.since_poll.lock().unwrap_or_else(|e| e.into_inner());
            *since += 1;
            if *since < POLL_FRAMES {
                return;
            }
            *since = 0;
        }
        for (i, pass) in self.loaded.source.passes.iter().enumerate() {
            let now = std::fs::metadata(&pass.wgsl_path)
                .and_then(|m| m.modified())
                .ok();
            let changed = {
                let build = self.passes[i]
                    .build
                    .lock()
                    .unwrap_or_else(|e| e.into_inner());
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

    /// Pass `i`'s group 0: its frame uniform and the shared params.
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
        let mut values = vec![0.0f32; params_size(source.params.len()) / 4];
        for (slot, (name, p)) in values.iter_mut().zip(&source.params) {
            *slot = frame.param(name).clamp(p.min, p.max);
        }
        frame
            .queue
            .write_buffer(&self.params_buffer, 0, bytemuck::cast_slice(&values));
        let input_bg = frame.sampled_input(names.input, &self.input_bgl, input);

        // The mask and the edges are there whenever this style is on and
        // reads them; if a host ever draws without one (or the edge pass
        // failed to build), the passthrough keeps the frame whole
        let extra_bg = match &self.extra_bgl {
            None => None,
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
                self.draw_passthrough(frame, encoder, &input_bg, output);
                return;
            }
        };
        let empty_bg = self.empty_bgl.as_ref().map(|bgl| {
            frame.bind(names.empty, 0, || {
                frame.device.create_bind_group(&wgpu::BindGroupDescriptor {
                    label: Some(names.empty),
                    layout: bgl,
                    entries: &[],
                })
            })
        });

        // Every pass must have built once: a chain missing a link draws
        // nothing meaningful, so the style passes its input through
        let builds: Vec<std::sync::MutexGuard<'_, Build>> = self
            .passes
            .iter()
            .map(|p| p.build.lock().unwrap_or_else(|e| e.into_inner()))
            .collect();
        if builds.iter().any(|b| b.pipeline.is_none()) {
            drop(builds);
            self.draw_passthrough(frame, encoder, &input_bg, output);
            return;
        }

        // This frame's targets: each history target's pair, the others once
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
            let pick = |r: &ReadSource| {
                if r.previous {
                    previous
                } else {
                    write
                }
            };
            let targets_bg = self.passes[i].targets_bgl.as_ref().map(|bgl| {
                // Keyed by what changes the views: each read's size and,
                // for history, which of the pair it is this frame
                let key = pass.reads.iter().fold(0xcbf2_9ce4_8422_2325u64, |h, r| {
                    let [w, ht] = sizes[r.target];
                    let side = if source.targets[r.target].history {
                        pick(r) as u64 + 1
                    } else {
                        0
                    };
                    let v = (u64::from(w) << 34) ^ (u64::from(ht) << 4) ^ side;
                    (h ^ v).wrapping_mul(0x0100_0000_01b3)
                });
                frame.bind(names.passes[i].targets, key, || {
                    let entries: Vec<wgpu::BindGroupEntry> = pass
                        .reads
                        .iter()
                        .enumerate()
                        .map(|(b, r)| wgpu::BindGroupEntry {
                            binding: b as u32,
                            resource: wgpu::BindingResource::TextureView(
                                &textures[r.target][pick(r)].view,
                            ),
                        })
                        .collect();
                    frame.device.create_bind_group(&wgpu::BindGroupDescriptor {
                        label: Some(names.passes[i].targets),
                        layout: bgl,
                        entries: &entries,
                    })
                })
            });
            let mut groups: Vec<&wgpu::BindGroup> = vec![&frame_bg, &input_bg];
            match (&extra_bg, &empty_bg, &targets_bg) {
                (Some(extra), _, _) => groups.push(extra),
                (None, Some(empty), Some(_)) => groups.push(empty),
                _ => {}
            }
            if let Some(t) = &targets_bg {
                groups.push(t);
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
        assert_eq!(
            ok.passes[0].wgsl_path,
            Path::new("styles").join("duotone.wgsl")
        );
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
                    check(&s, i, PASSTHROUGH)
                        .unwrap_or_else(|e| panic!("{stage} {inputs} pass {i}: {e}"));
                }
                // The helpers of what the last pass reads
                let user = "fn style(p: StylePixel) -> vec4<f32> {\n    \
                            return flint_target_e(p.uv) + flint_target_f(p.uv) \
                            + flint_target_h_load(vec2<i32>(p.pos)) \
                            + vec4<f32>(flint_target_f_size(), flint.pass_size) * flint.history;\n}\n";
                let report = check(&s, 3, user).unwrap();
                assert_eq!(report.texture_reads, 3, "the size isn't a read");
                // A target the pass doesn't read gets a hint
                let e = check(&s, 0, user).unwrap_err();
                assert!(e.contains("add \"e\" to reads of pass \"pass1\""), "{e}");
            }
        }
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
                    check(&s, 0, PASSTHROUGH).unwrap_or_else(|e| panic!("{stage} {inputs}: {e}"));
                assert_eq!(report.loops, 0);
            }
        }
    }

    #[test]
    fn errors_point_into_the_users_file() {
        let s = duotone();
        let user =
            "fn style(p: StylePixel) -> vec4<f32> {\n    let x = 1.0\n    return p.color;\n}\n";
        let e = check(&s, 0, user).unwrap_err();
        assert!(
            e.starts_with(&format!("{}:", s.passes[0].wgsl_path.display())),
            "{e}"
        );
        // The missing `;` is reported at the line after it (line 3)
        assert!(e.contains(":3:") || e.contains(":2:"), "{e}");
        assert!(e.contains("return p.color") || e.contains("let x"), "{e}");

        // A validation error (a type mismatch), not a parse error
        let user = "fn style(p: StylePixel) -> vec4<f32> {\n    return p.color.rgb;\n}\n";
        let e = check(&s, 0, user).unwrap_err();
        assert!(e.contains(":2:") || e.contains("style"), "{e}");
    }

    #[test]
    fn helpers_of_undeclared_inputs_get_a_hint() {
        let s = duotone(); // depth, no mask
        let user = "fn style(p: StylePixel) -> vec4<f32> {\n    \
                    return p.color * flint_style_blend(p.uv);\n}\n";
        let e = check(&s, 0, user).unwrap_err();
        assert!(e.contains("add \"mask\" to inputs"), "{e}");
        let user = "fn style(p: StylePixel) -> vec4<f32> {\n    \
                    return p.color * flint_view_depth(p.uv);\n}\n";
        check(&s, 0, user).unwrap();
    }

    #[test]
    fn the_contract_is_enforced() {
        let s = duotone();
        let e = check(
            &s,
            0,
            "fn shade(p: StylePixel) -> vec4<f32> { return p.color; }\n",
        )
        .unwrap_err();
        assert!(e.contains("no `fn style"), "{e}");
        let user = "@group(3) @binding(0) var extra: texture_2d<f32>;\n\
                    fn style(p: StylePixel) -> vec4<f32> { return p.color; }\n";
        let e = check(&s, 0, user).unwrap_err();
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
        let report = check(&s, 0, user).unwrap();
        assert_eq!(report.loops, 2);
        assert_eq!(report.texture_reads, 1, "one call site, written out once");
        assert_eq!(report.warnings.len(), 1);
        assert!(report.warnings[0].contains("2 loops"));
    }

    #[test]
    fn user_lines_map_back() {
        let s = duotone();
        let c = compose(&s, 0, "a\nb\n");
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
        // Loading again is a no-op
        let before = super::super::registry().len();
        assert!(load_dir(&dir).is_empty());
        assert_eq!(super::super::registry().len(), before);
        let _ = std::fs::remove_dir_all(&dir);
    }

    fn index_of(name: &str) -> usize {
        super::super::registry()
            .iter()
            .position(|d| d.name == name)
            .unwrap()
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
}
