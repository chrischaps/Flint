//! What a script says, and how long it takes (#703).
//!
//! **Speech.** Rhai's `print(x)` and `debug(x)` wrote to the process's
//! stdout, which a command such as `flint render --scripts --report json`
//! answers on, so one `print` in a script spoiled the report. They go to
//! `tracing` now, at info under the target `script`, with the file and the
//! entity of the script that is running ([`Speaker`]): the CLI writes its
//! log to stderr, and its default filter and the player's let `script=info`
//! through. `debug` adds the line it was called on.
//!
//! **Time.** With timing on (the perf readout, Flint#120), every callback
//! [`ScriptEngine::call`](super::ScriptEngine::call) runs is timed under
//! `<source>:<callback>` (`scripts/hud.rhai:on_draw_ui`), so the readout
//! names the callback, not only the file; the script system's own steps are
//! there too, in brackets, without the callbacks they ran ([`Times::step`]).

use crate::context::ScriptCallContext;
use std::sync::{Arc, Mutex};

/// The script running now, as its speech names it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Speaker {
    /// The script's file as found (`ScriptInstance::file`)
    pub file: String,
    /// The entity it is on, by name (or `entity <id>`)
    pub entity: String,
}

impl Speaker {
    /// No script: what the engine is evaluating outside any (a test's
    /// `eval`, a `flint script` probe).
    pub(crate) fn nobody() -> Arc<Self> {
        Arc::new(Self {
            file: String::new(),
            entity: String::new(),
        })
    }
}

/// Send `engine`'s `print` and `debug` to `tracing`, naming the script
/// `ctx` says is running.
pub(crate) fn route_speech(engine: &mut rhai::Engine, ctx: &Arc<Mutex<ScriptCallContext>>) {
    let printing = ctx.clone();
    engine.on_print(move |text| {
        let who = crate::lock_or_recover(&printing).speaker.clone();
        tracing::info!(target: "script", file = %who.file, entity = %who.entity, "{text}");
    });
    let debugging = ctx.clone();
    engine.on_debug(move |text, source, pos| {
        let who = crate::lock_or_recover(&debugging).speaker.clone();
        // A module's own source when the call is in an imported one
        let file = source.unwrap_or(&who.file);
        match pos.line() {
            Some(line) => tracing::info!(
                target: "script", file = %file, line, entity = %who.entity, "{text}"
            ),
            None => tracing::info!(target: "script", file = %file, entity = %who.entity, "{text}"),
        }
    });
}

/// The frame's script times, ms, while timing is on: each callback's under
/// `<source>:<callback>`, scripts sharing a file adding up, and the script
/// system's own steps under `[name]`.
#[derive(Debug, Default)]
pub(crate) struct Times {
    entries: Vec<(String, f32)>,
    /// Every callback's time so far, so a step can leave it out.
    in_callbacks: f32,
    /// What the callbacks' functions took (#1100), by callback and function.
    functions: Vec<FnTime>,
}

/// A function's time inside one callback (#1100), since the last take:
/// an engine function the callback's script called (`raycast()`) or one of
/// the script's own (`scripts/hud.rhai:draw_bar()`). Times are inclusive: a
/// script function's holds the engine calls it made.
#[derive(Debug, Clone, PartialEq)]
pub struct FnTime {
    /// The callback it was called under, `<source>:<callback>`, as
    /// [`ScriptSystem::take_times`](crate::ScriptSystem::take_times) names it
    pub callback: String,
    /// The function, as the trace names its scope
    pub function: &'static str,
    /// Every call, recursive ones too
    pub calls: u32,
    /// The outermost calls' time together, ms
    pub total_ms: f32,
    /// The longest outermost call, ms
    pub worst_ms: f32,
}

/// When a step of the script system began ([`Times::step`]).
#[derive(Debug, Clone, Copy)]
pub struct StepStart {
    at: web_time::Instant,
    in_callbacks: f32,
}

impl Times {
    /// `callback` on the script from `source` ran for `ms`.
    pub(crate) fn callback(&mut self, source: &str, callback: &str, ms: f32) {
        self.in_callbacks += ms;
        let label_is = |label: &str| {
            label
                .strip_prefix(source)
                .and_then(|rest| rest.strip_prefix(':'))
                == Some(callback)
        };
        match self.entries.iter_mut().find(|(l, _)| label_is(l)) {
            Some((_, t)) => *t += ms,
            None => self.entries.push((format!("{source}:{callback}"), ms)),
        }
    }

    /// What `callback` on the script from `source` spent in each function
    /// (`took`, drained).
    pub(crate) fn functions(
        &mut self,
        source: &str,
        callback: &str,
        took: &mut Vec<crate::fn_times::Took>,
    ) {
        for t in took.drain(..) {
            let label_is = |label: &str| {
                label
                    .strip_prefix(source)
                    .and_then(|rest| rest.strip_prefix(':'))
                    == Some(callback)
            };
            let found = self
                .functions
                .iter_mut()
                .find(|f| std::ptr::eq(f.function, t.name) && label_is(&f.callback));
            match found {
                Some(f) => {
                    f.calls += t.calls;
                    f.total_ms += t.total_ms;
                    f.worst_ms = f.worst_ms.max(t.worst_ms);
                }
                None => self.functions.push(FnTime {
                    callback: format!("{source}:{callback}"),
                    function: t.name,
                    calls: t.calls,
                    total_ms: t.total_ms,
                    worst_ms: t.worst_ms,
                }),
            }
        }
    }

    /// The functions' times since the last take.
    pub(crate) fn take_functions(&mut self) -> Vec<FnTime> {
        std::mem::take(&mut self.functions)
    }

    pub(crate) fn begin(&self) -> StepStart {
        StepStart {
            at: web_time::Instant::now(),
            in_callbacks: self.in_callbacks,
        }
    }

    /// The step `label` that began at `start` is done: its time, less the
    /// callbacks it ran (they have their own).
    pub(crate) fn step(&mut self, label: &str, start: StepStart) {
        let all = start.at.elapsed().as_secs_f32() * 1000.0;
        let own = (all - (self.in_callbacks - start.in_callbacks)).max(0.0);
        match self.entries.iter_mut().find(|(l, _)| l == label) {
            Some((_, t)) => *t += own,
            None => self.entries.push((label.to_string(), own)),
        }
    }

    /// What was timed since the last take.
    pub(crate) fn take(&mut self) -> Vec<(String, f32)> {
        std::mem::take(&mut self.entries)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_callback_is_keyed_by_file_and_callback_and_adds_up() {
        let mut t = Times::default();
        t.callback("scripts/hud.rhai", "on_update", 1.0);
        t.callback("scripts/hud.rhai", "on_draw_ui", 2.0);
        t.callback("scripts/hud.rhai", "on_draw_ui", 0.5);
        // A file whose name begins like another's is its own
        t.callback("scripts/hud.rhai.old", "on_draw_ui", 4.0);
        assert_eq!(
            t.take(),
            [
                ("scripts/hud.rhai:on_update".to_string(), 1.0),
                ("scripts/hud.rhai:on_draw_ui".to_string(), 2.5),
                ("scripts/hud.rhai.old:on_draw_ui".to_string(), 4.0),
            ]
        );
        assert!(t.take().is_empty(), "a take empties it");
    }

    #[test]
    fn a_step_leaves_out_the_callbacks_it_ran() {
        let mut t = Times::default();
        let start = t.begin();
        std::thread::sleep(std::time::Duration::from_millis(5));
        // As though the step's callbacks took far longer than it did
        t.callback("a.rhai", "on_init", 1000.0);
        t.step("[load]", start);
        let times = t.take();
        let load = times.iter().find(|(l, _)| l == "[load]").unwrap().1;
        assert_eq!(load, 0.0, "never below nothing");
    }
}
