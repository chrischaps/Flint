//! The generated prelude and the composed module wgpu builds (moved from
//! `custom.rs`, #742).

use super::*;

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

const PRELUDE_FRAME: &str = concat!(
    r#"// ---- Flint style prelude (generated; ADR 0112) ----
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

"#,
    // The helpers built-in styles include too (#743): one text for both
    include_str!("../prelude_core.wgsl"),
    r#"
// A cheap, stable hash of a cell: 0..1
fn flint_hash(p: vec2<f32>) -> f32 {
    let q = fract(p * vec2<f32>(123.34, 456.21));
    let r = q + dot(q, q + 45.32);
    return fract(r.x * r.y);
}
"#
);

pub(super) fn prelude_sample(stage: StyleStage) -> String {
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

pub(super) const PRELUDE_DEPTH: &str = r#"
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
    return flint_unproject(flint.inv_view_proj, uv, flint_depth(uv));
}
"#;

pub(super) const PRELUDE_MASK: &str = r#"
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
    if (id >= arrayLength(&flint_slots)) {
        return 0u;
    }
    return flint_slots[id];
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

pub(super) const PRELUDE_EDGES: &str = r#"
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
pub(super) fn prelude_targets(source: &StyleSource, pass: usize) -> String {
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

pub(super) fn epilogue(source: &StyleSource, last: bool) -> String {
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
    // per-entity variant, and which row each mask id takes, as many of
    // each as the frame has (#740: the framework's variant table)
    text.push_str(
        "\nstruct FlintParamTable {\n    rows: array<Params>,\n};\n\
         @group(0) @binding(1) var<storage, read> flint_params: FlintParamTable;\n",
    );
    if source.inputs.mask {
        text.push_str(
            "// Each mask id's row + 1, 0 for none of this style's\n\
             @group(0) @binding(2) var<storage, read> flint_slots: array<u32>;\n",
        );
    }
    text.push_str(
        "// This pixel's settings: its entity's, else the scene's\n\
         var<private> params: Params;\n",
    );
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
pub(super) fn option_constants(source: &StyleSource) -> String {
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
///
/// An image with `mips = true` (#558) has its helpers pick a level from
/// the screen ([`image_mip_helpers`]); one without has the text it had
/// before #558, word for word, so a style that doesn't ask draws the same.
pub(super) fn prelude_image(binding: usize, image: &ImageSource) -> String {
    let n = &image.name;
    let what = image
        .file
        .as_deref()
        .map_or_else(|| "the scene's choice".to_string(), |f| f.to_string());
    let mut text = format!(
        r#"
// Image `{n}` ({what})
@group(3) @binding({binding}) var flint_image_texture_{n}: texture_2d<f32>;
"#
    );
    text.push_str(&if image.mips {
        image_mip_helpers(n)
    } else {
        image_helpers(n)
    });
    text.push_str(&image_lut(n));
    text
}

/// Image `n`'s clamped, size and tiled helpers, reading level 0.
fn image_helpers(n: &str) -> String {
    format!(
        r#"
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
"#
    )
}

/// Image `n`'s helpers over a mip chain (#558): the clamped and tiled reads
/// take the level the screen asks for (trilinear: two levels mixed), from
/// the derivatives of `uv`, and `_level` forms take one given. The level
/// function waives WGSL's derivative uniformity rule for itself, so a helper
/// called inside a branch still compiles in a browser; where neighbouring
/// pixels take different sides of the branch its level is a guess, and the
/// `_level` forms are the way to say it.
fn image_mip_helpers(n: &str) -> String {
    format!(
        r#"
// The mip level image `{n}` is read at for uv: log2 of how many of its
// texels one pixel spans (0 when it spans one or fewer), up to its last
@diagnostic(off, derivative_uniformity)
fn flint_image_lod_{n}(uv: vec2<f32>) -> f32 {{
    let st = uv * vec2<f32>(textureDimensions(flint_image_texture_{n}));
    let span = max(length(dpdx(st)), length(dpdy(st)));
    let last = f32(textureNumLevels(flint_image_texture_{n}) - 1u);
    return clamp(log2(max(span, 1.0)), 0.0, last);
}}

// Image `{n}` at uv (0..1 across it), bilinear, clamped at its edges, at
// the level the screen asks for
fn flint_image_{n}(uv: vec2<f32>) -> vec4<f32> {{
    return textureSampleLevel(flint_image_texture_{n}, flint_sampler, uv, flint_image_lod_{n}(uv));
}}

// Image `{n}` at uv at mip `level` (0 is the file; a fraction mixes two)
fn flint_image_level_{n}(uv: vec2<f32>, level: f32) -> vec4<f32> {{
    return textureSampleLevel(flint_image_texture_{n}, flint_sampler, uv, level);
}}

// Image `{n}`'s size in texels (its file's, level 0)
fn flint_image_size_{n}() -> vec2<f32> {{
    return vec2<f32>(textureDimensions(flint_image_texture_{n}));
}}

// Image `{n}` repeated at mip `level`, a whole level: bilinear across the
// seam
fn flint_image_tiled_at_{n}(uv: vec2<f32>, level: i32) -> vec4<f32> {{
    let dims = vec2<i32>(textureDimensions(flint_image_texture_{n}, level));
    let st = uv * vec2<f32>(dims) - 0.5;
    let base = floor(st);
    let f = st - base;
    let p0 = ((vec2<i32>(base) % dims) + dims) % dims;
    let p1 = (p0 + vec2<i32>(1)) % dims;
    let a = textureLoad(flint_image_texture_{n}, p0, level);
    let b = textureLoad(flint_image_texture_{n}, vec2<i32>(p1.x, p0.y), level);
    let c = textureLoad(flint_image_texture_{n}, vec2<i32>(p0.x, p1.y), level);
    let d = textureLoad(flint_image_texture_{n}, p1, level);
    return mix(mix(a, b, f.x), mix(c, d, f.x), f.y);
}}

// Image `{n}` repeated: uv 1.25 is uv 0.25, at mip `level` (a fraction
// mixes the two levels about it)
fn flint_image_tiled_level_{n}(uv: vec2<f32>, level: f32) -> vec4<f32> {{
    let last = f32(textureNumLevels(flint_image_texture_{n}) - 1u);
    let l = clamp(level, 0.0, last);
    let below = floor(l);
    let a = flint_image_tiled_at_{n}(uv, i32(below));
    let t = l - below;
    if (t <= 0.0) {{
        return a;
    }}
    return mix(a, flint_image_tiled_at_{n}(uv, i32(min(below + 1.0, last))), t);
}}

// Image `{n}` repeated: uv 1.25 is uv 0.25, at the level the screen asks for
fn flint_image_tiled_{n}(uv: vec2<f32>) -> vec4<f32> {{
    return flint_image_tiled_level_{n}(uv, flint_image_lod_{n}(uv));
}}
"#
    )
}

/// Image `n` read as a LUT: always level 0, whether it has mips or not.
fn image_lut(n: &str) -> String {
    format!(
        r#"
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

/// The settings table's rows (#414) as 32-bit words: `rows` (row 0 the
/// scene's) each laid out as `Params`, its `count` values in declaration
/// order. Which row each mask id takes is the table's other buffer.
pub fn row_words(count: usize, rows: &[Vec<f32>]) -> Vec<u32> {
    let stride = params_size(count) / 4;
    let mut words = vec![0u32; rows.len() * stride];
    for (r, row) in rows.iter().enumerate() {
        for (i, v) in row.iter().take(count).enumerate() {
            words[r * stride + i] = v.to_bits();
        }
    }
    words
}
