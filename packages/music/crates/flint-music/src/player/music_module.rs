//! The `music` module: where the scene's music is (#745), for any game with
//! a suite playing, chart or no chart. The music subsystem sets the clock
//! before each frame's scripts: the audio clock in play (what the ear
//! hears, latency-compensated), the game clock in a stepped capture
//! (`--frame-step`, `flint render --scripts`), so a beat lands on the same
//! frame every run. With no music each getter says so (`-1`, `""`), and a
//! script asks the same way in any scene of a run with the package, which
//! enables it everywhere (`Registry::every_scene_modules`). ADR 0116; the
//! music package's since #1619, moved from flint-script unchanged.

use flint_script::api::builder::kind::{Int, Num, Str};
use flint_script::api::Module;
use flint_script::ScriptModule;

/// The module's name.
pub const MODULE: &str = "music";

pub struct MusicModule;

/// The `music` module, every scene's while the package is in.
pub static MUSIC_MODULE: MusicModule = MusicModule;

impl ScriptModule for MusicModule {
    fn name(&self) -> &'static str {
        MODULE
    }

    fn enabled_when(&self) -> &'static str {
        "the build has music (the music package)"
    }

    fn declare(&self, m: &mut Module<'_>) {
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
}

#[cfg(test)]
mod tests {
    use super::*;
    use flint_script::{MusicClock, ScriptSystem};

    fn system() -> ScriptSystem {
        let mut s = ScriptSystem::new();
        s.declare_module(&MUSIC_MODULE);
        s.set_enabled_modules(vec![MODULE]);
        s
    }

    fn eval<T: Clone + Send + Sync + 'static>(s: &mut ScriptSystem, src: &str) -> T {
        s.rhai_engine_mut().eval::<T>(src).expect(src)
    }

    #[test]
    fn with_no_music_the_clock_says_so() {
        let mut s = system();
        assert_eq!(eval::<f64>(&mut s, "music_beat()"), -1.0);
        assert_eq!(eval::<i64>(&mut s, "music_bar()"), -1);
        assert_eq!(eval::<rhai::ImmutableString>(&mut s, "music_section()"), "");
    }

    #[test]
    fn scripts_read_the_clock_the_host_set() {
        let mut s = system();
        s.set_music(MusicClock {
            playing: true,
            beat: 9.5,
            bar: 2,
            section: "verse".into(),
        });
        assert_eq!(eval::<f64>(&mut s, "music_beat()"), 9.5);
        assert_eq!(eval::<i64>(&mut s, "music_bar()"), 2);
        assert_eq!(
            eval::<rhai::ImmutableString>(&mut s, "music_section()"),
            "verse"
        );
        assert_eq!(eval::<f64>(&mut s, "music_beat() % 1.0"), 0.5);
    }
}
