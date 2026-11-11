//! The `time` module: the frame's step, the scene's clock and the time
//! scale (ADR 0116).

use super::builder::kind::Num;
use super::builder::ScriptApi;

pub(crate) fn register(api: &mut ScriptApi) {
    let mut m = api.module("time");

    m.func(
        "delta_time",
        "Seconds of game time since the last frame (the time scale applied).",
    )
    .returns(Num, "")
    .run(|call, ()| call.ctx_ref().delta_time);

    m.func(
        "total_time",
        "Seconds of game time since the scene started.",
    )
    .returns(Num, "")
    .run(|call, ()| call.ctx_ref().total_time);

    m.func(
        "set_time_scale",
        "Game seconds per real second from the next frame (1 normal, 0.25 slow motion), clamped to 0.05..4; a scene load resets it.",
    )
    .arg("scale", Num)
    .run(|call, (scale,)| {
        call.ctx().time_scale_request = Some(scale.clamp(0.05, 4.0));
    });
}
