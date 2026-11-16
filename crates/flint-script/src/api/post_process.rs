//! The `post_process` module: overrides on the scene's `[post_process]`
//! settings, the frame's tear, the deprecated render modes and the
//! renderer's distance levers (ADR 0116). Each is a request the host
//! applies after the script batch; all are held until changed but the
//! render mode, which lasts only while a script keeps calling it.

use super::builder::kind::{Bool, Int, Num};
use super::builder::ScriptApi;

pub(crate) fn register(api: &mut ScriptApi) {
    let mut m = api.module("post_process");

    m.func("set_vignette", "Vignette intensity (0 none, 1 heavy).")
        .arg("intensity", Num)
        .run(|call, (v,)| {
            call.ctx().requests.post.vignette = Some(v as f32);
        });

    m.func("set_bloom_intensity", "Bloom strength (0 none).")
        .arg("intensity", Num)
        .run(|call, (v,)| {
            call.ctx().requests.post.bloom = Some(v as f32);
        });

    m.func("set_exposure", "Exposure multiplier (1 as authored).")
        .arg("value", Num)
        .run(|call, (v,)| {
            call.ctx().requests.post.exposure = Some(v as f32);
        });

    m.func("set_chromatic_aberration", "Chromatic aberration strength.")
        .arg("intensity", Num)
        .run(|call, (v,)| {
            call.ctx().requests.post.chromatic_aberration = Some(v as f32);
        });

    m.func("set_radial_blur", "Radial blur strength.")
        .arg("intensity", Num)
        .run(|call, (v,)| {
            call.ctx().requests.post.radial_blur = Some(v as f32);
        });

    m.func("set_ssao_intensity", "SSAO intensity.")
        .arg("intensity", Num)
        .run(|call, (v,)| {
            call.ctx().requests.post.ssao_intensity = Some(v as f32);
        });

    m.func("set_fog_density", "Fog density.")
        .arg("density", Num)
        .run(|call, (v,)| {
            call.ctx().requests.post.fog_density = Some(v as f32);
        });

    m.func("set_fog_color", "Fog colour, linear 0..1.")
        .arg("r", Num)
        .arg("g", Num)
        .arg("b", Num)
        .run(|call, (r, g, b)| {
            call.ctx().requests.fog_color = Some([r as f32, g as f32, b as f32]);
        });

    m.func(
        "set_desaturation",
        "Desaturate toward ash grey (0 full colour, 1 grey).",
    )
    .arg("amount", Num)
    .run(|call, (v,)| {
        call.ctx().requests.post.desaturation = Some(v as f32);
    });

    m.func(
        "set_dof",
        "Depth-of-field defocus strength (0 sharp, 1 full blur).",
    )
    .arg("strength", Num)
    .run(|call, (v,)| {
        call.ctx().requests.post.dof_strength = Some(v as f32);
    });

    m.func(
        "set_dof_focus",
        "The depth-of-field focus plane's distance and half-width, view metres.",
    )
    .arg("distance", Num)
    .arg("range", Num)
    .run(|call, (distance, range)| {
        let c = call.ctx();
        c.requests.post.dof_focus_distance = Some(distance as f32);
        c.requests.post.dof_focus_range = Some(range as f32);
    });

    m.func(
        "set_tear",
        "How much of the frame tears open, 0..1 ([post_process] tear): a style with blend_source = \"tear\" (set_post_style_param) or a prop's style that follows it shows only in the holes.",
    )
    .arg("amount", Num)
    .run(|call, (amount,)| {
        call.ctx().requests.post.tear = Some(amount.clamp(0.0, 1.0) as f32);
    });

    m.func(
        "set_tear_shape",
        "The tear's shape: patches across the frame's height (3 by default), or an iris from the centre.",
    )
    .arg("scale", Num)
    .arg("iris", Bool)
    .run(|call, (scale, iris)| {
        let c = call.ctx();
        c.requests.post.tear_scale = Some(scale as f32);
        c.requests.post.tear_iris = Some(iris);
    });

    m.func(
        "set_render_mode",
        "A render mode (1 Matrix, 2 blood, 3 drunk, 4 Tron, 5 underwater, 6 the tear alone) at a mix of 0..1, shown the frames it is called; each is a style now.",
    )
    .deprecated(
        "the modes are styles (#746): set_post_style(\"matrix\", true) with set_post_style_param(\"matrix\", \"blend_source\", \"tear\") and set_tear(mix); drunk and underwater take their blend",
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
        "name them (#746): set_tear_shape(scale, iris), set_post_style_param(\"matrix\", \"rate\", r), set_post_style_param(\"underwater\", \"depth\", d)...",
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
