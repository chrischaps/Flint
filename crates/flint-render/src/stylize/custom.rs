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
    StyleOption, StyleParam, StylePass, StyleStage,
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
    pub shader: String,
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
}

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
    /// Parameters in uniform order (by name).
    pub params: Vec<(String, ParamDef)>,
    /// Options in uniform order (by name), after the parameters (#413).
    pub options: Vec<(String, OptionSource)>,
    /// Images in binding order (by name; group 3, #413).
    pub images: Vec<ImageSource>,
    /// The `.shader.toml`.
    pub toml_path: PathBuf,
    /// The WGSL file it names, beside it.
    pub wgsl_path: PathBuf,
}

impl StyleSource {
    /// The folder the description is in: its shader and images are here.
    pub fn dir(&self) -> &Path {
        self.toml_path.parent().unwrap_or(Path::new("."))
    }

    /// How many 4-byte fields its `Params` uniform has before the pad.
    pub fn uniform_fields(&self) -> usize {
        self.params.len() + self.options.len()
    }
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
        let shader = Path::new(&self.shader);
        if shader.is_absolute() || shader.components().any(|c| c.as_os_str() == "..") {
            return Err(format!(
                "{at}: shader = \"{}\" must be a file beside it",
                self.shader
            ));
        }
        let dir = toml_path.parent().unwrap_or(Path::new("."));

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
            label: self.label.unwrap_or_else(|| super::humanize(&self.name)),
            name: self.name,
            stage,
            inputs,
            feeds_bloom: self.feeds_bloom,
            params: self.params.into_iter().collect(),
            options,
            images,
            toml_path: toml_path.to_path_buf(),
            wgsl_path: dir.join(shader),
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

fn epilogue(stage: StyleStage) -> String {
    let out = if stage == StyleStage::Display {
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

/// The prelude for `source`: everything before the user's first line.
pub fn prelude(source: &StyleSource) -> String {
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

/// `user` (the style's WGSL) inside its prelude and entry points.
pub fn compose(source: &StyleSource, user: &str) -> Composed {
    let prelude = prelude(source);
    let prelude_lines = prelude.matches('\n').count();
    let mut user = user.to_string();
    if !user.ends_with('\n') {
        user.push('\n');
    }
    let user_lines = user.matches('\n').count();
    let text = format!("{prelude}{user}{}", epilogue(source.stage));
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

/// Parse and validate `user` as `source`'s shader, the way it will be
/// built. `Err` is the message to show, lines in the user's file.
#[cfg(not(target_arch = "wasm32"))]
pub fn check(source: &StyleSource, user: &str) -> Result<Report, String> {
    let composed = compose(source, user);
    let file = source.wgsl_path.display().to_string();
    let module = naga::front::wgsl::parse_str(&composed.text).map_err(|e| {
        locate(
            &composed,
            &file,
            e.location(&composed.text),
            e.message(),
            source,
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
                 (flint_input, params, depth, mask, edges, images)",
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
            if name.starts_with("flint_image_size_") {
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
pub fn check(_source: &StyleSource, _user: &str) -> Result<Report, String> {
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
    /// The images' error (#413): a file missing or that doesn't decode.
    /// Kept apart from the shader's, which a good build clears.
    image_error: Mutex<Option<String>>,
    /// The `.shader.toml`'s modified time when last read
    /// ([`reload_changed`]).
    #[cfg(not(target_arch = "wasm32"))]
    toml_modified: Mutex<Option<std::time::SystemTime>>,
}

#[derive(Clone, Copy)]
struct BindNames {
    frame: &'static str,
    input: &'static str,
    extra: &'static str,
    pass: &'static str,
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
    let error = match flint_core::vfs::read_to_string(&source.wgsl_path) {
        Err(e) => Some(format!("{}: {e}", source.wgsl_path.display())),
        Ok(user) => match check(&source, &user) {
            Ok(report) => {
                messages.extend(report.warnings);
                None
            }
            Err(e) => Some(e),
        },
    };
    if let Some(e) = &error {
        messages.push(e.clone());
    }
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
    let descriptor: &'static StyleDescriptor = Box::leak(Box::new(
        StyleDescriptor::new(name, leak(source.label.clone()), source.stage, create)
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
            .feeds_bloom(source.feeds_bloom),
    ));
    // The chain's bind cache keys on these names, so each install gets its
    // own: a replaced style (#275) builds new buffers and layouts, and a
    // cached group under the old name would bind the old pass's buffers
    // (frozen at the old values, or the wrong size for a new param count)
    static GENERATION: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let generation = GENERATION.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let names = BindNames {
        frame: leak(format!("{name} style frame BG #{generation}")),
        input: leak(format!("{name} style input BG #{generation}")),
        extra: leak(format!("{name} style depth/mask BG #{generation}")),
        pass: leak(format!("{name} style pass")),
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
}

struct Build {
    /// The user's pipeline, or `None` while it has never compiled.
    pipeline: Option<wgpu::RenderPipeline>,
    /// The WGSL file's modified time when last read.
    #[cfg(not(target_arch = "wasm32"))]
    modified: Option<std::time::SystemTime>,
    /// Frames since the file was last looked at.
    #[cfg(not(target_arch = "wasm32"))]
    since_poll: u32,
}

struct CustomPass {
    loaded: Arc<Loaded>,
    format: wgpu::TextureFormat,
    frame_bgl: wgpu::BindGroupLayout,
    input_bgl: wgpu::BindGroupLayout,
    extra_bgl: Option<wgpu::BindGroupLayout>,
    layout: wgpu::PipelineLayout,
    frame_buffer: wgpu::Buffer,
    params_buffer: wgpu::Buffer,
    /// Draws the input unchanged, while the user's shader can't.
    passthrough: wgpu::RenderPipeline,
    build: Mutex<Build>,
    /// The style's images (#413), group 3.
    images: Option<Mutex<ImageSet>>,
    /// Group 2 when a style binds images but reads none of the optional
    /// buffers: an empty group keeps the images at group 3.
    empty: Option<wgpu::BindGroup>,
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
    let extra_bgl = (source.inputs.any() || images.is_some()).then(|| {
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
    let mut layouts = vec![&frame_bgl, &input_bgl];
    if let Some(extra) = &extra_bgl {
        layouts.push(extra);
    }
    if let Some(images) = &images {
        layouts.push(&images.layout);
    }
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
    let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some("User Style Layout"),
        bind_group_layouts: &layouts,
        push_constant_ranges: &[],
    });
    // The passthrough is the same prelude with no inputs: always compiles
    let bare = StyleSource {
        inputs: Inputs::default(),
        images: Vec::new(),
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
        &compose(&bare, PASSTHROUGH).text,
        d.format,
    );
    let pass = CustomPass {
        format: d.format,
        frame_buffer: kit::uniform_buffer::<FlintFrameUniforms>(device, "User Style Frame"),
        params_buffer: device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("User Style Params"),
            size: params_size(source.uniform_fields()) as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        }),
        frame_bgl,
        input_bgl,
        extra_bgl,
        layout,
        passthrough,
        build: Mutex::new(Build {
            pipeline: None,
            #[cfg(not(target_arch = "wasm32"))]
            modified: None,
            #[cfg(not(target_arch = "wasm32"))]
            since_poll: 0,
        }),
        images: images.map(Mutex::new),
        empty,
        loaded,
    };
    pass.rebuild(device);
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
            let now = std::fs::metadata(&path).and_then(|m| m.modified()).ok();
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

impl CustomPass {
    fn set_error(&self, error: Option<String>) {
        let mut slot = self.loaded.error.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(e) = &error {
            if slot.as_ref() != Some(e) {
                tracing::warn!("style '{}': {e}", self.loaded.source.name);
            }
        } else if slot.is_some() {
            tracing::info!("style '{}' compiles again", self.loaded.source.name);
        }
        *slot = error;
    }

    /// Read, check and build the user's shader; on failure keep what was
    /// drawing (the last good build, or the passthrough).
    fn rebuild(&self, device: &wgpu::Device) {
        let source = &self.loaded.source;
        #[cfg(not(target_arch = "wasm32"))]
        let modified = std::fs::metadata(&source.wgsl_path)
            .and_then(|m| m.modified())
            .ok();
        let user = match flint_core::vfs::read_to_string(&source.wgsl_path) {
            Ok(user) => user,
            Err(e) => {
                self.set_error(Some(format!("{}: {e}", source.wgsl_path.display())));
                return;
            }
        };
        let mut build = self.build.lock().unwrap_or_else(|e| e.into_inner());
        #[cfg(not(target_arch = "wasm32"))]
        {
            build.modified = modified;
        }
        if let Err(e) = check(source, &user) {
            self.set_error(Some(e));
            return;
        }
        // naga has passed it; the error scope catches what the backend's own
        // compiler still refuses (and, on the web, everything)
        device.push_error_scope(wgpu::ErrorFilter::Validation);
        let made = pipeline(
            device,
            &self.layout,
            &compose(source, &user).text,
            self.format,
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
            Some(e) => self.set_error(Some(format!(
                "{}: the GPU refused it: {e}",
                source.wgsl_path.display()
            ))),
            None => {
                build.pipeline = Some(made);
                self.set_error(None);
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

    /// Rebuild when the WGSL file changed on disk (native only).
    #[cfg(not(target_arch = "wasm32"))]
    fn poll(&self, device: &wgpu::Device) {
        let changed = {
            let mut build = self.build.lock().unwrap_or_else(|e| e.into_inner());
            build.since_poll += 1;
            if build.since_poll < POLL_FRAMES {
                return;
            }
            build.since_poll = 0;
            let now = std::fs::metadata(&self.loaded.source.wgsl_path)
                .and_then(|m| m.modified())
                .ok();
            now.is_some() && now != build.modified
        };
        if changed {
            self.rebuild(device);
        }
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
        let names = self.loaded.names;
        let camera = frame.camera;
        let uniforms = FlintFrameUniforms {
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
            mask_id: u32::from(frame.style_id(source.name.as_str()).unwrap_or(0)),
        };
        frame
            .queue
            .write_buffer(&self.frame_buffer, 0, bytemuck::cast_slice(&[uniforms]));
        let mut values = vec![0.0f32; params_size(source.uniform_fields()) / 4];
        for (slot, (name, p)) in values.iter_mut().zip(&source.params) {
            *slot = frame.param(name).clamp(p.min, p.max);
        }
        // Options follow as u32 indices, written as their bits
        for (slot, (name, o)) in values[source.params.len()..]
            .iter_mut()
            .zip(&source.options)
        {
            *slot = f32::from_bits(o.index(frame.option(name)));
        }
        let images_ready = self.update_images(frame);
        frame
            .queue
            .write_buffer(&self.params_buffer, 0, bytemuck::cast_slice(&values));

        let frame_bg = frame.bind(names.frame, 0, || {
            frame.device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some(names.frame),
                layout: &self.frame_bgl,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: self.frame_buffer.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: self.params_buffer.as_entire_binding(),
                    },
                ],
            })
        });
        let input_bg = frame.sampled_input(names.input, &self.input_bgl, input);

        let build = self.build.lock().unwrap_or_else(|e| e.into_inner());
        let passthrough = |encoder: &mut wgpu::CommandEncoder| {
            kit::fullscreen_pass(
                encoder,
                names.pass,
                output,
                &self.passthrough,
                &[&frame_bg, &input_bg],
            )
        };
        let Some(pipeline) = &build.pipeline else {
            passthrough(encoder);
            return;
        };
        // An image that never read (a missing file) leaves the style a
        // passthrough with its error, as a shader that doesn't compile does
        if !images_ready {
            passthrough(encoder);
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
                passthrough(encoder);
                return;
            }
        };
        let mut groups: Vec<&wgpu::BindGroup> = vec![&frame_bg, &input_bg];
        if let Some(extra) = &extra_bg {
            groups.push(extra);
        } else if let Some(empty) = &self.empty {
            groups.push(empty);
        }
        let mut images = self
            .images
            .as_ref()
            .map(|i| i.lock().unwrap_or_else(|e| e.into_inner()));
        if let Some(images) = images.as_mut() {
            groups.push(images.bind_group(frame.device));
        }
        kit::fullscreen_pass(encoder, names.pass, output, pipeline, &groups);
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
    fn params_round_up_to_vec4s_and_a_pad() {
        assert_eq!(params_size(0), 16);
        assert_eq!(params_size(1), 32);
        assert_eq!(params_size(4), 32);
        assert_eq!(params_size(5), 48);
        assert_eq!(std::mem::size_of::<FlintFrameUniforms>(), 128);
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
        assert_eq!(report.warnings.len(), 1);
        assert!(report.warnings[0].contains("2 loops"));
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
        assert!(pass.build.lock().unwrap().pipeline.is_some());

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
            pass.build.lock().unwrap().pipeline.is_some(),
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
        let old_bind = loaded("utest_edit").unwrap().names.frame;
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
        assert_ne!(loaded("utest_edit").unwrap().names.frame, old_bind);
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
}
