//! The `touch` module: fingers down this frame, taps and swipes (ADR
//! 0116). A touch is named by its id while it lasts (the platform's
//! pointer id: not 0, 1, 2 in general, so `touch_id(i)` for `i` in
//! `0..touch_count()` gives each); taps and swipes are this frame's, by
//! index from 0. Positions are 0..1 of the screen's width and height
//! (multiply by screen_width() for points).

use super::builder::kind::{Bool, Int, Num, Str};
use super::builder::ScriptApi;

pub(crate) fn register(api: &mut ScriptApi) {
    let mut m = api.module("touch");

    m.func("touch_count", "How many fingers are down.")
        .returns(Int, "")
        .run(|call, ()| call.ctx_ref().input.touches.len() as i64);

    m.func(
        "touch_id",
        "The id of the index-th finger down (0 to touch_count() - 1, lowest id first), for touch_x and the rest.",
    )
    .arg("index", Int)
    .returns(Int, "-1 past the last touch")
    .run(|call, (index,)| {
        let touches = &call.ctx_ref().input.touches;
        usize::try_from(index)
            .ok()
            .and_then(|i| touches.get(i))
            .map_or(-1, |(id, _, _)| *id)
    });

    m.func(
        "touch_x",
        "The x of the touch with this id (touch_id gives them).",
    )
    .arg("id", Int)
    .returns(
        Num,
        "0..1 across the screen; -1.0 for no touch with that id",
    )
    .run(|call, (id,)| {
        call.ctx_ref()
            .input
            .touches
            .iter()
            .find(|(tid, _, _)| *tid == id)
            .map_or(-1.0, |(_, x, _)| *x)
    });

    m.func(
        "touch_y",
        "The y of the touch with this id (touch_id gives them).",
    )
    .arg("id", Int)
    .returns(
        Num,
        "0..1 across the screen; -1.0 for no touch with that id",
    )
    .run(|call, (id,)| {
        call.ctx_ref()
            .input
            .touches
            .iter()
            .find(|(tid, _, _)| *tid == id)
            .map_or(-1.0, |(_, _, y)| *y)
    });

    m.func("is_touching", "Whether a touch with this id is down.")
        .arg("id", Int)
        .returns(Bool, "")
        .run(|call, (id,)| {
            call.ctx_ref()
                .input
                .touches
                .iter()
                .any(|(tid, _, _)| *tid == id)
        });

    m.func(
        "touch_just_started",
        "Whether a touch with this id began this frame.",
    )
    .arg("id", Int)
    .returns(Bool, "")
    .run(|call, (id,)| {
        call.ctx_ref()
            .input
            .touch_just_started
            .iter()
            .any(|(tid, _, _)| *tid == id)
    });

    m.func(
        "touch_just_ended",
        "Whether a touch with this id lifted this frame.",
    )
    .arg("id", Int)
    .returns(Bool, "")
    .run(|call, (id,)| call.ctx_ref().input.touch_just_ended.contains(&id));

    m.func("tap_count", "How many taps this frame.")
        .returns(Int, "")
        .run(|call, ()| call.ctx_ref().input.touch_taps.len() as i64);

    m.func("tap_x", "A tap's x.")
        .arg("index", Int)
        .returns(Num, "0..1 across the screen; -1.0 past the last tap")
        .run(|call, (index,)| {
            call.ctx_ref()
                .input
                .touch_taps
                .get(index as usize)
                .map_or(-1.0, |(x, _)| *x)
        });

    m.func("tap_y", "A tap's y.")
        .arg("index", Int)
        .returns(Num, "0..1 across the screen; -1.0 past the last tap")
        .run(|call, (index,)| {
            call.ctx_ref()
                .input
                .touch_taps
                .get(index as usize)
                .map_or(-1.0, |(_, y)| *y)
        });

    m.func("swipe_count", "How many swipes this frame.")
        .returns(Int, "")
        .run(|call, ()| call.ctx_ref().input.touch_swipes.len() as i64);

    m.func("swipe_direction", "A swipe's direction.")
        .arg("index", Int)
        .returns(
            Str,
            "\"left\", \"right\", \"up\" or \"down\"; \"\" past the last swipe",
        )
        .run(|call, (index,)| {
            call.ctx_ref()
                .input
                .touch_swipes
                .get(index as usize)
                .map(|(dir, _, _)| dir.clone())
                .unwrap_or_default()
        });

    m.func("swipe_x", "Where a swipe started, x.")
        .arg("index", Int)
        .returns(Num, "0..1 across the screen; -1.0 past the last swipe")
        .run(|call, (index,)| {
            call.ctx_ref()
                .input
                .touch_swipes
                .get(index as usize)
                .map_or(-1.0, |(_, x, _)| *x)
        });

    m.func("swipe_y", "Where a swipe started, y.")
        .arg("index", Int)
        .returns(Num, "0..1 across the screen; -1.0 past the last swipe")
        .run(|call, (index,)| {
            call.ctx_ref()
                .input
                .touch_swipes
                .get(index as usize)
                .map_or(-1.0, |(_, _, y)| *y)
        });

    m.func(
        "is_swipe",
        "Whether there was a swipe in this direction this frame.",
    )
    .arg("direction", Str)
    .returns(Bool, "")
    .run(|call, (direction,)| {
        call.ctx_ref()
            .input
            .touch_swipes
            .iter()
            .any(|(dir, _, _)| *dir == direction.as_str())
    });
}
