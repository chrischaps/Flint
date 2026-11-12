//! The `music` module: where the scene's music is (#745), for any game with
//! a suite playing, chart or no chart. The host's music subsystem sets the
//! clock before each frame's scripts: the audio clock in play (what the ear
//! hears, latency-compensated), the game clock in a stepped capture
//! (`--frame-step`, `flint render --scripts`), so a beat lands on the same
//! frame every run. With no music each getter says so (`-1`, `""`), and a
//! script asks the same way in any scene. ADR 0116.

use super::builder::kind::{Int, Num, Str};
use super::builder::ScriptApi;

pub(crate) fn register(api: &mut ScriptApi) {
    let mut m = api.module("music");

    m.func(
        "music_beat",
        "Beats since the music's first beat, with the fraction, across tempo changes.",
    )
    .returns(
        Num,
        "beats; negative in the count-in, -1.0 with no music playing",
    )
    .run(|call, ()| call.ctx_ref().music.beat);

    m.func("music_bar", "The bar the music is in, counted from 0.")
        .returns(Int, "the bar; -1 with no music playing")
        .run(|call, ()| call.ctx_ref().music.bar);

    m.func("music_section", "The name of the section the music is in.")
        .returns(
            Str,
            "the section's name; \"\" before the first or with no music",
        )
        .run(|call, ()| rhai::ImmutableString::from(call.ctx_ref().music.section.as_str()));
}

#[cfg(test)]
mod tests {
    use crate::context::MusicClock;
    use crate::engine::ScriptEngine;

    fn eval<T: Clone + Send + Sync + 'static>(engine: &ScriptEngine, src: &str) -> T {
        engine.rhai_engine().eval::<T>(src).expect(src)
    }

    #[test]
    fn with_no_music_the_clock_says_so() {
        let engine = ScriptEngine::new();
        assert_eq!(eval::<f64>(&engine, "music_beat()"), -1.0);
        assert_eq!(eval::<i64>(&engine, "music_bar()"), -1);
        assert_eq!(
            eval::<rhai::ImmutableString>(&engine, "music_section()"),
            ""
        );
    }

    #[test]
    fn scripts_read_the_clock_the_host_set() {
        let engine = ScriptEngine::new();
        engine.ctx.lock().unwrap().music = MusicClock {
            playing: true,
            beat: 9.5,
            bar: 2,
            section: "verse".into(),
        };
        assert_eq!(eval::<f64>(&engine, "music_beat()"), 9.5);
        assert_eq!(eval::<i64>(&engine, "music_bar()"), 2);
        assert_eq!(
            eval::<rhai::ImmutableString>(&engine, "music_section()"),
            "verse"
        );
        assert_eq!(eval::<f64>(&engine, "music_beat() % 1.0"), 0.5);
    }
}
