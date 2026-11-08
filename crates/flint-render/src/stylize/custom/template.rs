//! The starting files `flint style new` writes (#416): a description and a
//! commented passthrough that lists every field and helper the chosen
//! stage and inputs give, so an author (or an agent) never has to read the
//! book to learn the contract.

use super::{Inputs, StyleStage};

/// A new style's two files.
#[derive(Debug, Clone, PartialEq)]
pub struct Template {
    /// `<name>.shader.toml`
    pub toml: String,
    /// `<name>.wgsl`
    pub wgsl: String,
}

/// The files for a style called `name` (already a valid style name: see
/// [`super::ShaderStyleDef::check`]). The shader draws its input unchanged
/// at the example parameter's default, so a new style is safe to switch on
/// before it does anything.
pub fn template(name: &str, stage: StyleStage, inputs: Inputs) -> Template {
    Template {
        toml: toml(name, stage, inputs),
        wgsl: wgsl(name, stage, inputs),
    }
}

fn input_names(inputs: Inputs) -> Vec<&'static str> {
    let mut names = Vec::new();
    if inputs.depth {
        names.push("depth");
    }
    if inputs.mask {
        names.push("mask");
    }
    if inputs.edges {
        names.push("edges");
    }
    names
}

fn toml(name: &str, stage: StyleStage, inputs: Inputs) -> String {
    let stage_name = match stage {
        StyleStage::Display => "display",
        StyleStage::Hdr => "hdr",
    };
    let inputs_list = input_names(inputs)
        .iter()
        .map(|i| format!("\"{i}\""))
        .collect::<Vec<_>>()
        .join(", ");
    let label = super::super::humanize(name);
    let mask_note = if inputs.mask {
        format!(
            "\n# With \"mask\", the style reaches the entities that carry\n\
             #   [entities.<n>.style_mask]\n\
             #   style = \"{name}\"\n\
             # and flint_style_blend(uv) says how much of it each pixel asks for.\n"
        )
    } else {
        String::new()
    };
    format!(
        r#"# A user-authored style (ADR 0112), written by `flint style new {name}`.
# Edit it and {name}.wgsl beside it; no engine change, no rebuild. The
# player, `flint edit` and `flint render` load every styles/*.shader.toml
# beside a scene (or one folder up), and natively reload it while they run.
#
#   flint style check {name}.shader.toml     the shader, checked without a GPU
#   flint style prelude {name}.shader.toml   the code it is compiled inside
#   flint render <scene> --style {name}:amount=1
#   flint render <scene> --sweep {name}.amount=0,0.5,1
#
# Or switch it on from a scene:
#   [post_process.styles.{name}]
#   amount = 1.0

# The format's version; this engine reads 1.
version = 1
# Lowercase letters, digits and _; the file is <name>.shader.toml.
name = "{name}"
# What the F4 menu and --gpu-time call it.
label = "{label}"
# "display": the finished picture, after tonemapping, at output resolution.
# "hdr": linear light before fog, bloom and the grade, at the scene's
# resolution (add `feeds_bloom = true` for bloom to read the result).
stage = "{stage_name}"
# The WGSL beside this file. It holds one function, `style`.
shader = "{name}.wgsl"
# Colour is always there. "depth" gives flint_depth / flint_view_depth /
# flint_world_pos, "mask" gives flint_mask_id / flint_style_blend, and
# "edges" gives flint_edges / flint_edge (the shared geometry edges).
inputs = [{inputs_list}]
{mask_note}
# Each [params.<name>] becomes `params.<name>` (an f32) in the shader and a
# slider in F4. default/min/max are required; label, help and integer are
# optional. Values from a scene or --style are clamped to min..max. Three
# params named <base>_r/_g/_b fold into one colour picker.
[params.amount]
default = 0.0
min = 0.0
max = 1.0
label = "Amount"
help = "How far the example effect goes (0 = the picture unchanged)"
"#
    )
}

fn wgsl(name: &str, stage: StyleStage, inputs: Inputs) -> String {
    let (color_note, example) = match stage {
        StyleStage::Display => (
            "a display colour (what the eye sees, 0..1)",
            "    // The example: toward grey by `amount`. Replace it with your own.\n    \
             let grey = vec3<f32>(flint_luma(c.rgb));\n    \
             return vec4<f32>(mix(c.rgb, grey, params.amount), c.a);",
        ),
        StyleStage::Hdr => (
            "linear light, which may exceed 1",
            "    // The example: toward grey by `amount`, keeping the light's energy.\n    \
             // Replace it with your own.\n    \
             let grey = vec3<f32>(flint_luma(c.rgb));\n    \
             return vec4<f32>(mix(c.rgb, grey, params.amount), c.a);",
        ),
    };
    let mut helpers = String::from(
        "//   flint_sample(uv) -> vec4      the picture anywhere (neighbours, offsets),\n\
         //                                 in the same space as p.color\n\
         //   flint_luma(rgb) -> f32        perceived brightness\n\
         //   flint_hash(v: vec2) -> f32    a stable 0..1 noise\n\
         //   flint_to_display(rgb), flint_to_linear(rgb)   sRGB encode / decode\n",
    );
    let mut tries = String::new();
    if inputs.depth {
        helpers.push_str(
            "//   flint_depth(uv) -> f32        the depth buffer, 0 (near) .. 1 (far)\n\
             //   flint_view_depth(uv) -> f32   metres from the camera\n\
             //   flint_world_pos(uv) -> vec3   the world position under the pixel\n",
        );
        tries.push_str(
            "    // let metres = flint_view_depth(p.uv);\n    \
             // let world = flint_world_pos(p.uv);\n",
        );
    }
    if inputs.mask {
        helpers.push_str(
            "//   flint_mask_id(uv) -> u32      the style mask id (0 = no style)\n\
             //   flint_style_blend(uv) -> f32  0 off this style's entities, else their blend\n",
        );
        tries.push_str(&format!(
            "    // Only on entities with `style_mask {{ style = \"{name}\" }}`:\n    \
             // let mine = flint_style_blend(p.uv);\n"
        ));
    }
    if inputs.edges {
        helpers.push_str(
            "//   flint_edges(uv) -> vec4       r silhouette, g crease, ba direction across\n\
             //   flint_edge(uv) -> f32         the stronger of silhouette and crease\n",
        );
        tries.push_str("    // let line = flint_edge(p.uv);\n");
    }
    let missing: Vec<&str> = ["depth", "mask", "edges"]
        .into_iter()
        .filter(|i| !input_names(inputs).contains(i))
        .collect();
    if !missing.is_empty() {
        helpers.push_str(&format!(
            "//   (more with inputs = [\"{}\"] in the .shader.toml)\n",
            missing.join("\", \"")
        ));
    }
    if !tries.is_empty() {
        tries = format!("\n    // The inputs this style asked for:\n{tries}");
    }
    format!(
        r#"// The `{name}` style, written by `flint style new {name}`: a passthrough
// with one example parameter, `amount`, that does nothing at its default.
//
// This file holds one function, `style`, called once for each pixel. The
// engine writes everything around it (you never declare a binding or an
// entry point); `flint style prelude {name}.shader.toml` prints it.
//
//   p.uv     where the pixel is, 0..1 from the top left
//   p.pos    where the pixel is, in pixels (centres at .5)
//   p.color  the picture at this pixel: {color_note}
//
//   params.amount    each [params.<name>] of the .shader.toml, an f32
//   flint.resolution, flint.texel (1 / resolution), flint.time (seconds),
//   flint.frame, flint.near, flint.far, flint.pixels_per_point,
//   flint.camera_pos, flint.inv_view_proj
//
{helpers}//
// Return the colour to draw. A mistake never crashes the game: the style
// draws its last good build (or the picture unchanged) and the error shows
// in the corner of the window, with its line in this file. Check it first
// with `flint style check {name}.shader.toml`.
//
// Loops are allowed, but keep their bounds constant or a clamped param: a
// loop that never ends hangs the GPU until the driver resets it.

fn style(p: StylePixel) -> vec4<f32> {{
    let c = p.color;
{tries}
{example}
}}
"#
    )
}
