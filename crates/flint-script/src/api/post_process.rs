//! The `post_process` module: overrides on the scene's `[post_process]`
//! settings, the stylized render modes and the renderer's distance levers
//! (ADR 0116). Each is a request the host applies after the script batch;
//! all are held until changed but the render mode, which lasts only while
//! a script keeps calling it.

use super::builder::kind::{Int, Num};
use super::builder::ScriptApi;

pub(crate) fn register(api: &mut ScriptApi) {
    let mut m = api.module("post_process");

    m.func("set_vignette", "Vignette intensity (0 none, 1 heavy).")
        .arg("intensity", Num)
        .run(|call, (v,)| {
            call.ctx().postprocess_vignette_override = Some(v as f32);
        });

    m.func("set_bloom_intensity", "Bloom strength (0 none).")
        .arg("intensity", Num)
        .run(|call, (v,)| {
            call.ctx().postprocess_bloom_override = Some(v as f32);
        });

    m.func("set_exposure", "Exposure multiplier (1 as authored).")
        .arg("value", Num)
        .run(|call, (v,)| {
            call.ctx().postprocess_exposure_override = Some(v as f32);
        });

    m.func("set_chromatic_aberration", "Chromatic aberration strength.")
        .arg("intensity", Num)
        .run(|call, (v,)| {
            call.ctx().postprocess_chromatic_aberration_override = Some(v as f32);
        });

    m.func("set_radial_blur", "Radial blur strength.")
        .arg("intensity", Num)
        .run(|call, (v,)| {
            call.ctx().postprocess_radial_blur_override = Some(v as f32);
        });

    m.func("set_ssao_intensity", "SSAO intensity.")
        .arg("intensity", Num)
        .run(|call, (v,)| {
            call.ctx().postprocess_ssao_intensity_override = Some(v as f32);
        });

    m.func("set_fog_density", "Fog density.")
        .arg("density", Num)
        .run(|call, (v,)| {
            call.ctx().postprocess_fog_density_override = Some(v as f32);
        });

    m.func("set_fog_color", "Fog colour, linear 0..1.")
        .arg("r", Num)
        .arg("g", Num)
        .arg("b", Num)
        .run(|call, (r, g, b)| {
            call.ctx().postprocess_fog_color_override = Some([r as f32, g as f32, b as f32]);
        });

    m.func(
        "set_desaturation",
        "Desaturate toward ash grey (0 full colour, 1 grey).",
    )
    .arg("amount", Num)
    .run(|call, (v,)| {
        call.ctx().postprocess_desaturation_override = Some(v as f32);
    });

    m.func(
        "set_dof",
        "Depth-of-field defocus strength (0 sharp, 1 full blur).",
    )
    .arg("strength", Num)
    .run(|call, (v,)| {
        call.ctx().postprocess_dof_strength_override = Some(v as f32);
    });

    m.func(
        "set_dof_focus",
        "The depth-of-field focus plane's distance and half-width, view metres.",
    )
    .arg("distance", Num)
    .arg("range", Num)
    .run(|call, (distance, range)| {
        let c = call.ctx();
        c.postprocess_dof_focus_distance_override = Some(distance as f32);
        c.postprocess_dof_focus_range_override = Some(range as f32);
    });

    m.func(
        "set_render_mode",
        "A stylized render mode (0 none, 1 Matrix, 2 blood, 3 drunk, 4 Tron, 5 underwater) at a mix of 0..1; call it every frame it shows.",
    )
    .arg("mode", Int)
    .arg("mix", Num)
    .run(|call, (mode, mix)| {
        call.ctx().postprocess_render_mode_override =
            Some((mode.max(0) as u32, mix.clamp(0.0, 1.0) as f32));
    });

    m.func(
        "set_render_mode_params",
        "The render mode's four tuning values (see Post-Processing: Render Modes).",
    )
    .arg("x", Num)
    .arg("y", Num)
    .arg("z", Num)
    .arg("w", Num)
    .run(|call, (x, y, z, w)| {
        call.ctx().postprocess_mode_params_override =
            Some([x as f32, y as f32, z as f32, w as f32]);
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
        call.ctx().lod_scale_request = Some(scale as f32);
    });

    m.func(
        "set_draw_distance_scale",
        "Multiply every model's draw distance (1 as authored).",
    )
    .arg("scale", Num)
    .run(|call, (scale,)| {
        call.ctx().draw_distance_scale_request = Some(scale as f32);
    });

    m.func(
        "lod_scale",
        "The level-of-detail scale in force, or the one just asked for.",
    )
    .returns(Num, "")
    .run(|call, ()| {
        let c = call.ctx_ref();
        c.lod_scale_request.unwrap_or(c.lod_scale) as f64
    });

    m.func(
        "draw_distance_scale",
        "The draw-distance scale in force, or the one just asked for.",
    )
    .returns(Num, "")
    .run(|call, ()| {
        let c = call.ctx_ref();
        c.draw_distance_scale_request
            .unwrap_or(c.draw_distance_scale) as f64
    });
}
