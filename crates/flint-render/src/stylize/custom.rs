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
    pub shader: String,
    #[serde(default)]
    pub inputs: Vec<String>,
    #[serde(default)]
    pub params: BTreeMap<String, ParamDef>,
    /// Bloom reads this style's result (#300); hdr styles only.
    #[serde(default)]
    pub feeds_bloom: bool,
}

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
    /// The component an entity takes the style with (#414): `<name>_style`
    /// for a style that reads the mask, its fields the parameters. It
    /// makes the style per-entity, as `ascii_style` does ASCII: a
    /// `style_mask` naming it (or the component) switches it on with no
    /// table, and each entity's settings reach its own pixels as `params`.
    pub component: Option<String>,
    /// Parameters in uniform order (by name).
    pub params: Vec<(String, ParamDef)>,
    /// The `.shader.toml`.
    pub toml_path: PathBuf,
    /// The WGSL file it names, beside it.
    pub wgsl_path: PathBuf,
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
        let shader = Path::new(&self.shader);
        if shader.is_absolute() || shader.components().any(|c| c.as_os_str() == "..") {
            return Err(format!(
                "{at}: shader = \"{}\" must be a file beside it",
                self.shader
            ));
        }
        let dir = toml_path.parent().unwrap_or(Path::new("."));
        Ok(StyleSource {
            component: inputs.mask.then(|| component_name(&self.name)),
            label: self.label.unwrap_or_else(|| super::humanize(&self.name)),
            name: self.name,
            stage,
            inputs,
            feeds_bloom: self.feeds_bloom,
            params: self.params.into_iter().collect(),
            toml_path: toml_path.to_path_buf(),
            wgsl_path: dir.join(shader),
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

fn epilogue(source: &StyleSource) -> String {
    let stage = source.stage;
    let own = if source.inputs.mask {
        "params = flint_params_at(in.uv);"
    } else {
        "params = flint_params.rows[0];"
    };
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

/// The prelude for `source`: everything before the user's first line.
pub fn prelude(source: &StyleSource) -> String {
    let mut text = String::from(PRELUDE_FRAME);
    text.push_str("\nstruct Params {\n");
    for (name, _) in &source.params {
        text.push_str(&format!("    {name}: f32,\n"));
    }
    // Never empty, and the f32s above round up to 16 bytes before it
    text.push_str("    _flint_pad: vec4<f32>,\n};\n");
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
    text.push_str("\n// ---- user style ----\n");
    text
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
    let text = format!("{prelude}{user}{}", epilogue(source));
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
#[cfg(not(target_arch = "wasm32"))]
pub fn check(source: &StyleSource, user: &str) -> Result<Report, String> {
    let composed = compose(source, user);
    let file = source.wgsl_path.display().to_string();
    let mut module = naga::front::wgsl::parse_str(&composed.text).map_err(|e| {
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
                 (flint_input, params, depth, mask, edges)",
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
            usize::from(matches!(
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
            ))
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
    let wgsl = flint_core::vfs::read_to_string(&source.wgsl_path);
    // A shader that reads its own blend draws it (#396): the framework's
    // cross-fade would apply it twice. Decided when the description is
    // read, so a shader that starts or stops calling it takes effect at the
    // next load or `.shader.toml` edit
    let blends_itself = wgsl.as_ref().is_ok_and(|user| calls_style_blend(user));
    let error = match wgsl {
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
    let mut built = StyleDescriptor::new(name, leak(source.label.clone()), source.stage, create);
    if let Some(component) = &source.component {
        built = built.component(leak(component.clone()));
    }
    let built = built
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
    let mut layouts = vec![&frame_bgl, &input_bgl];
    if let Some(extra) = &extra_bgl {
        layouts.push(extra);
    }
    let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some("User Style Layout"),
        bind_group_layouts: &layouts,
        push_constant_ranges: &[],
    });
    // The passthrough is the same prelude with no inputs: always compiles
    let bare = StyleSource {
        inputs: Inputs::default(),
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
            size: table_size(source.params.len(), source.inputs.mask) as u64,
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
        warned: Default::default(),
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
        let report = match check(source, &user) {
            Ok(report) => report,
            Err(e) => {
                self.set_error(Some(e));
                return;
            }
        };
        // The guarded module when it has loops (#411), else the text as
        // composed
        let text = report
            .guarded
            .unwrap_or_else(|| compose(source, &user).text);
        // naga has passed it; the error scope catches what the backend's own
        // compiler still refuses (and, on the web, everything)
        device.push_error_scope(wgpu::ErrorFilter::Validation);
        let made = pipeline(device, &self.layout, &text, self.format);
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
        // The settings table (#414): row 0 the scene's, then one a variant
        let row = |param: &dyn Fn(&str) -> f32| -> Vec<f32> {
            source
                .params
                .iter()
                .map(|(name, p)| param(name).clamp(p.min, p.max))
                .collect()
        };
        let mut rows = vec![row(&|n| frame.param(n))];
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
                rows.push(row(&|n| frame.variant_param(v, n)));
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
            table_words(source.params.len(), &rows, Some(&slots))
        } else {
            table_words(source.params.len(), &rows, None)
        };
        frame
            .queue
            .write_buffer(&self.params_buffer, 0, bytemuck::cast_slice(&words));

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
                kit::fullscreen_pass(
                    encoder,
                    names.pass,
                    output,
                    &self.passthrough,
                    &[&frame_bg, &input_bg],
                );
                return;
            }
        };
        match (&build.pipeline, extra_bg) {
            (Some(p), Some(extra)) => kit::fullscreen_pass(
                encoder,
                names.pass,
                output,
                p,
                &[&frame_bg, &input_bg, &extra],
            ),
            (Some(p), None) => {
                kit::fullscreen_pass(encoder, names.pass, output, p, &[&frame_bg, &input_bg])
            }
            (None, _) => kit::fullscreen_pass(
                encoder,
                names.pass,
                output,
                &self.passthrough,
                &[&frame_bg, &input_bg],
            ),
        }
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
