//! The `post_process` module: overrides on the scene's `[post_process]`
//! settings (`set_post`, #853), the deprecated setters it replaced and the
//! deprecated render modes, and the renderer's distance levers (ADR 0116).
//! Each is a request the host applies after the script batch; all are held
//! until changed but the render mode, which lasts only while a script keeps
//! calling it.

use super::builder::kind::{Any, Bool, Int, Num, Str};
use super::builder::{CallResult, ScriptApi};
use super::convert::dynamic_to_toml;
use flint_schema::post_effects::PostValue;

/// The deprecated setters that were one number into one key (#853): name,
/// key, the parameter's name, doc and what to write instead.
const ONE_NUMBER: &[(&str, &str, &str, &str, &str)] = &[
    (
        "set_bloom_intensity",
        "bloom_intensity",
        "intensity",
        "Bloom strength (0 none).",
        "set_post(\"bloom_intensity\", v)",
    ),
    (
        "set_exposure",
        "exposure",
        "value",
        "Exposure multiplier (1 as authored).",
        "set_post(\"exposure\", v)",
    ),
    (
        "set_chromatic_aberration",
        "chromatic_aberration",
        "intensity",
        "Chromatic aberration strength.",
        "set_post(\"chromatic_aberration\", v)",
    ),
    (
        "set_radial_blur",
        "radial_blur",
        "intensity",
        "Radial blur strength.",
        "set_post(\"radial_blur\", v)",
    ),
    (
        "set_ssao_intensity",
        "ssao_intensity",
        "intensity",
        "SSAO intensity.",
        "set_post(\"ssao_intensity\", v)",
    ),
    (
        "set_fog_density",
        "fog_density",
        "density",
        "Fog density.",
        "set_post(\"fog_density\", v)",
    ),
    (
        "set_desaturation",
        "desaturate",
        "amount",
        "Desaturate toward ash grey (0 full colour, 1 grey).",
        "set_post(\"desaturate\", v)",
    ),
    (
        "set_dof",
        "dof_strength",
        "strength",
        "Depth-of-field defocus strength (0 sharp, 1 full blur).",
        "set_post(\"dof_strength\", v)",
    ),
];

pub(crate) fn register(api: &mut ScriptApi) {
    let mut m = api.module("post_process");

    m.func(
        "set_post",
        "Set one of the scene's [post_process] keys (vignette_intensity, bloom_intensity, fog_color, tear...; `flint render --list-effects` lists them) until set again: a number, a bool, or [r, g, b] for a colour.",
    )
    .arg("param", Str)
    .arg("value", Any)
    .run(|call, (param, value)| -> CallResult<()> {
        let set = match dynamic_to_toml(&value) {
            Some(toml) => call.ctx().requests.post.set(&param, &toml),
            None => Err(match flint_schema::post_effects::param(&param) {
                Some(_) => format!("`{param}` can't take a {}", value.type_name()),
                None => flint_schema::post_effects::unknown_key(&param),
            }),
        };
        set.map_err(|why| call.error(why))
    });

    // ── The setters `set_post` replaced (#853), each for one release ──
    for &(name, key, arg, doc, instead) in ONE_NUMBER {
        m.func(name, doc)
            .deprecated(instead)
            .arg(arg, Num)
            .run(move |call, (v,)| {
                call.ctx().requests.post.put(key, PostValue::F32(v as f32));
            });
    }

    m.func("set_vignette", "Vignette intensity (0 none, 1 heavy).")
        .deprecated("set_post(\"vignette_intensity\", v) and set_post(\"vignette_enabled\", true), which this did with v > 0.001")
        .arg("intensity", Num)
        .run(|call, (v,)| {
            let post = &mut call.ctx().requests.post;
            post.put("vignette_enabled", PostValue::Bool(v as f32 > 0.001));
            post.put("vignette_intensity", PostValue::F32(v as f32));
        });

    m.func("set_fog_color", "Fog colour, linear 0..1.")
        .deprecated("set_post(\"fog_color\", [r, g, b])")
        .arg("r", Num)
        .arg("g", Num)
        .arg("b", Num)
        .run(|call, (r, g, b)| {
            call.ctx()
                .requests
                .post
                .put("fog_color", PostValue::Rgb([r as f32, g as f32, b as f32]));
        });

    m.func(
        "set_dof_focus",
        "The depth-of-field focus plane's distance and half-width, view metres.",
    )
    .deprecated("set_post(\"dof_focus_distance\", d) and set_post(\"dof_focus_range\", r)")
    .arg("distance", Num)
    .arg("range", Num)
    .run(|call, (distance, range)| {
        let post = &mut call.ctx().requests.post;
        post.put("dof_focus_distance", PostValue::F32(distance as f32));
        post.put("dof_focus_range", PostValue::F32(range as f32));
    });

    m.func(
        "set_tear",
        "How much of the frame tears open, 0..1 ([post_process] tear): a style with blend_source = \"tear\" (set_post_style_param) or a prop's style that follows it shows only in the holes.",
    )
    .deprecated("set_post(\"tear\", amount)")
    .arg("amount", Num)
    .run(|call, (amount,)| {
        call.ctx()
            .requests
            .post
            .put("tear", PostValue::F32(amount.clamp(0.0, 1.0) as f32));
    });

    m.func(
        "set_tear_shape",
        "The tear's shape: patches across the frame's height (3 by default), or an iris from the centre.",
    )
    .deprecated("set_post(\"tear_scale\", scale) and set_post(\"tear_iris\", iris)")
    .arg("scale", Num)
    .arg("iris", Bool)
    .run(|call, (scale, iris)| {
        let post = &mut call.ctx().requests.post;
        post.put("tear_scale", PostValue::F32(scale as f32));
        post.put("tear_iris", PostValue::Bool(iris));
    });

    m.func(
        "set_render_mode",
        "A render mode (1 Matrix, 2 blood, 3 drunk, 4 Tron, 5 underwater, 6 the tear alone) at a mix of 0..1, shown the frames it is called; each is a style now.",
    )
    .deprecated(
        "the modes are styles (#746): set_post_style(\"matrix\", true) with set_post_style_param(\"matrix\", \"blend_source\", \"tear\") and set_post(\"tear\", mix); drunk and underwater take their blend",
    )
    .arg("mode", Int)
    .arg("mix", Num)
    .run(|call, (mode, mix)| {
        call.ctx().requests.render_mode =
            Some((mode.max(0) as u32, mix.clamp(0.0, 1.0) as f32));
    });

    m.func(
        "set_render_mode_params",
        "The render mode's four values: tear scale, iris, Matrix rate; underwater's depth, churn, daylight, glow.",
    )
    .deprecated(
        "name them (#746): set_post(\"tear_scale\", scale), set_post(\"tear_iris\", iris), set_post_style_param(\"matrix\", \"rate\", r), set_post_style_param(\"underwater\", \"depth\", d)...",
    )
    .arg("x", Num)
    .arg("y", Num)
    .arg("z", Num)
    .arg("w", Num)
    .run(|call, (x, y, z, w)| {
        call.ctx().requests.mode_params = Some([x as f32, y as f32, z as f32, w as f32]);
    });

    // ── Distance levers (#56) ──
    // Applied after this frame's scripts and kept across scene changes;
    // clamped to 0.05 and up by the host.
    m.func(
        "set_lod_scale",
        "Multiply every level-of-detail switch distance (1 as authored, 0.5 simpler levels at half the distance).",
    )
    .arg("scale", Num)
    .run(|call, (scale,)| {
        call.ctx().requests.lod_scale = Some(scale as f32);
    });

    m.func(
        "set_draw_distance_scale",
        "Multiply every model's draw distance (1 as authored).",
    )
    .arg("scale", Num)
    .run(|call, (scale,)| {
        call.ctx().requests.draw_distance_scale = Some(scale as f32);
    });

    // Each under its name and, for one release, its old one (#702)
    for (name, old) in [
        ("get_lod_scale", None),
        ("lod_scale", Some("use get_lod_scale")),
    ] {
        m.func(
            name,
            "The level-of-detail scale in force, or the one just asked for.",
        )
        .returns(Num, "")
        .deprecated(old)
        .run(|call, ()| {
            let c = call.ctx_ref();
            c.requests.lod_scale.unwrap_or(c.lod_scale) as f64
        });
    }

    for (name, old) in [
        ("get_draw_distance_scale", None),
        ("draw_distance_scale", Some("use get_draw_distance_scale")),
    ] {
        m.func(
            name,
            "The draw-distance scale in force, or the one just asked for.",
        )
        .returns(Num, "")
        .deprecated(old)
        .run(|call, ()| {
            let c = call.ctx_ref();
            c.requests
                .draw_distance_scale
                .unwrap_or(c.draw_distance_scale) as f64
        });
    }
}
