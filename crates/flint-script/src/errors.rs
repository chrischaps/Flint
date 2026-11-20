//! Script errors, counted and kept (#693).
//!
//! A script that fails in `on_update` fails every frame. Each failure used to
//! be its own `warn!`, sixty identical lines a second, with no count, nothing
//! on screen and an exit code of 0. Every failure now goes through
//! [`ScriptErrors::record`]: the same error (file, callback, line, message)
//! is one [`ScriptError`] with a count. It is logged the first time, then
//! again at the 10th, 100th, 1000th... time with the count, and a host shows
//! [`ScriptErrors::on_screen`] (the player's error card), prints
//! [`ScriptErrors::summary`] at the end of a run and fails it under
//! `--strict-scripts`.
//!
//! Load and compile errors are recorded the same way, with
//! [`LOAD`] and [`COMPILE`] for the callback, so a script that never ran
//! counts as much as one that fails each frame.
//!
//! A call into a script module the scene hasn't enabled (#698) is an error
//! a shared script makes on purpose in every scene without the feature, so
//! it is logged once a scene instead (#856, [`ONCE_A_SCENE`]): the first
//! time, and again the first time after a transition or a reload of its
//! file. It is counted, shown on the card, summed and failed under
//! `--strict-scripts` as any other.

use std::fmt;

/// The tag on a run-time error's text that makes it logged once a scene
/// (#856): what a call into a module that isn't enabled returns. Any other
/// error is logged at 1, 10, 100... calls. An `i16`, since Rhai's tag is
/// one on 32-bit targets (wasm) and an `i32` on 64-bit ones.
pub const ONCE_A_SCENE: i16 = 856;

/// The callback of an error found loading a script: its file wasn't found.
pub const LOAD: &str = "load";
/// The callback of an error compiling a script, at load or hot reload.
pub const COMPILE: &str = "compile";
/// The callback of an error in a script's top-level statements, run once at
/// load (and on reload) to fill its scope.
pub const TOP_LEVEL: &str = "top level";

/// The most distinct errors kept. Past it an error is still counted in
/// [`ScriptErrors::total`] and logged once, but not kept.
pub const MAX_KEPT: usize = 64;

/// The most other entities named on one error.
const MAX_ALSO_ON: usize = 8;

/// One way a script failed, and how often.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScriptError {
    /// The script's file, as found (or as the scene names it, when it
    /// wasn't), or the imported module's when the error was in one.
    pub file: String,
    /// The entity the script is on: the first to fail this way.
    pub entity_name: String,
    /// Other entities whose scripts failed the same way (a shared file), at
    /// most eight.
    pub also_on: Vec<String>,
    /// The callback that was running (`on_update`), or [`LOAD`],
    /// [`COMPILE`] or [`TOP_LEVEL`].
    pub callback: String,
    /// The function the error was in, when it isn't the callback itself: a
    /// helper `fn`, or a module's.
    pub function: Option<String>,
    /// 1-based, when Rhai knows it.
    pub line: Option<usize>,
    pub column: Option<usize>,
    /// What went wrong, without its position.
    pub message: String,
    /// How many times.
    pub count: u64,
    /// The frame it first happened in (0 is the scene's load and `on_init`).
    pub first_frame: u64,
    /// The frame it last happened in.
    pub last_frame: u64,
    /// Whether the script it came from is still the one running: false once
    /// its file is reloaded or its scene is gone. The error card shows only
    /// these; the run's count keeps every one.
    pub current: bool,
    /// Logged once a scene rather than at 1, 10, 100... (#856): a call into
    /// a module the scene hasn't enabled.
    pub once_a_scene: bool,
}

impl ScriptError {
    /// `file:line:col`, as far as it is known.
    pub fn location(&self) -> String {
        match (self.line, self.column) {
            (Some(l), Some(c)) => format!("{}:{l}:{c}", self.file),
            (Some(l), None) => format!("{}:{l}", self.file),
            _ => self.file.clone(),
        }
    }

    /// Whether the script never ran: not found, or it didn't compile at
    /// load.
    pub fn is_load_error(&self) -> bool {
        self.callback == LOAD || self.callback == COMPILE
    }

    /// `hud_controller, on_update` (`> draw_bar` when in a helper), with
    /// `(+2 more)` for other entities.
    pub fn context(&self) -> String {
        let mut s = format!("{}, {}", self.entity_name, self.callback);
        if let Some(f) = &self.function {
            s.push_str(&format!(" > {f}"));
        }
        if !self.also_on.is_empty() {
            s.push_str(&format!(" (+{} more)", self.also_on.len()));
        }
        s
    }
}

impl fmt::Display for ScriptError {
    /// `scripts/hud.rhai:42:5: hud_controller, on_update: Function not found: foo ()`
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{}: {}: {}",
            self.location(),
            self.context(),
            self.message
        )
    }
}

/// A failure, before it is counted.
#[derive(Debug, Clone)]
pub struct Failure {
    pub file: String,
    pub entity_name: String,
    pub callback: String,
    pub function: Option<String>,
    pub line: Option<usize>,
    pub column: Option<usize>,
    pub message: String,
    /// Log it once a scene (#856): the error's text carried
    /// [`ONCE_A_SCENE`].
    pub once_a_scene: bool,
}

impl Failure {
    /// A run-time error from `call_fn` or `run_ast_with_scope`: the
    /// innermost error's message and position, and the function it was in
    /// (an imported module's file, when the function is one of its).
    pub fn eval(
        file: &str,
        entity_name: &str,
        callback: &str,
        error: &rhai::EvalAltResult,
    ) -> Self {
        let mut file = file.to_string();
        let mut function = None;
        let mut at = error;
        loop {
            match at {
                rhai::EvalAltResult::ErrorInFunctionCall(name, source, inner, _) => {
                    if !source.is_empty() {
                        file = source.clone();
                    }
                    // A timed function's original body (#1100) is its
                    // wrapper's call, which is named already
                    if !crate::fn_times::is_inner(name) {
                        function = (name != callback).then(|| name.clone());
                    }
                    at = inner;
                }
                rhai::EvalAltResult::ErrorInModule(_, inner, _) => at = inner,
                _ => break,
            }
        }
        let once_a_scene = matches!(
            at,
            rhai::EvalAltResult::ErrorRuntime(value, _) if i64::from(value.tag()) == i64::from(ONCE_A_SCENE)
        );
        let pos = at.position();
        // Rhai ends the text with the position, which is kept apart
        let text = at.to_string();
        let suffix = format!(" ({pos})");
        let message = match text.strip_suffix(&suffix) {
            Some(bare) if !pos.is_none() => bare.to_string(),
            _ => text,
        };
        Self {
            file,
            entity_name: entity_name.to_string(),
            callback: callback.to_string(),
            function,
            line: pos.line(),
            column: pos.position(),
            message,
            once_a_scene,
        }
    }

    /// A compile error as [`crate::engine::ScriptEngine::compile`] words it
    /// (Rhai's `ParseError`, which ends `(line 3, position 9)`).
    pub fn compile(file: &str, entity_name: &str, message: &str) -> Self {
        let (message, line, column) = split_position(message);
        Self {
            file: file.to_string(),
            entity_name: entity_name.to_string(),
            callback: COMPILE.to_string(),
            function: None,
            line,
            column,
            message,
            once_a_scene: false,
        }
    }

    /// A script the search didn't find: `message` names every path tried.
    pub fn load(file: &str, entity_name: &str, message: &str) -> Self {
        Self {
            file: file.to_string(),
            entity_name: entity_name.to_string(),
            callback: LOAD.to_string(),
            function: None,
            line: None,
            column: None,
            message: message.to_string(),
            once_a_scene: false,
        }
    }
}

/// How an error names a script's file: from the working directory when
/// it is under it (so a terminal can open it), else as found, with `/`.
pub fn shown_file(path: &std::path::Path) -> String {
    let relative = std::env::current_dir()
        .ok()
        .and_then(|cwd| path.strip_prefix(cwd).ok().map(|p| p.to_path_buf()));
    flint_core::project::shown(relative.as_deref().unwrap_or(path))
}

/// Take Rhai's ` (line 3, position 9)` off the end of a message.
fn split_position(message: &str) -> (String, Option<usize>, Option<usize>) {
    let trimmed = message.trim_end();
    if let Some(open) = trimmed.rfind(" (line ") {
        if let Some(inner) = trimmed[open + 7..].strip_suffix(')') {
            let mut parts = inner.split(", position ");
            let line = parts.next().and_then(|l| l.trim().parse().ok());
            let column = parts.next().and_then(|c| c.trim().parse().ok());
            if line.is_some() {
                return (trimmed[..open].to_string(), line, column);
            }
        }
    }
    (trimmed.to_string(), None, None)
}

/// Every script error of a run (#693).
#[derive(Debug, Default, Clone)]
pub struct ScriptErrors {
    kept: Vec<ScriptError>,
    /// Every failure, kept or not.
    total: u64,
    /// Distinct errors past [`MAX_KEPT`].
    dropped: u64,
    /// Frames since the run began; [`Self::next_frame`] counts them.
    frame: u64,
    /// Whether to log (tests of the counting turn it off).
    quiet: bool,
    /// Lines logged for failures, or that would have been when quiet.
    logged: u64,
}

impl ScriptErrors {
    pub fn new() -> Self {
        Self::default()
    }

    /// Count a failure. The first of its kind is logged whole; then at the
    /// 10th, 100th, 1000th... with the count. One marked `once_a_scene` is
    /// logged whole the first time and the first time again after
    /// [`Self::scene_cleared`] or its file's [`Self::file_reloaded`], and at
    /// no count (#856).
    pub fn record(&mut self, failure: Failure) {
        self.total += 1;
        let frame = self.frame;
        if let Some(err) = self.kept.iter_mut().find(|e| {
            e.file == failure.file
                && e.callback == failure.callback
                && e.line == failure.line
                && e.message == failure.message
        }) {
            err.count += 1;
            err.last_frame = frame;
            let new_here = !err.current;
            err.current = true;
            if err.entity_name != failure.entity_name
                && !err.also_on.contains(&failure.entity_name)
                && err.also_on.len() < MAX_ALSO_ON
            {
                err.also_on.push(failure.entity_name);
            }
            if err.once_a_scene {
                if new_here {
                    self.logged += 1;
                    if !self.quiet {
                        tracing::warn!(
                            "[script] {err} (logged once a scene; {} times in all since frame {})",
                            err.count,
                            err.first_frame
                        );
                    }
                }
            } else if is_milestone(err.count) {
                self.logged += 1;
                if !self.quiet {
                    let first = err.message.lines().next().unwrap_or("");
                    tracing::warn!(
                        "[script] {}: {}: the same error {} times since frame {}: {first}",
                        err.location(),
                        err.context(),
                        err.count,
                        err.first_frame
                    );
                }
            }
            return;
        }
        let err = ScriptError {
            file: failure.file,
            entity_name: failure.entity_name,
            also_on: Vec::new(),
            callback: failure.callback,
            function: failure.function,
            line: failure.line,
            column: failure.column,
            message: failure.message,
            count: 1,
            first_frame: frame,
            last_frame: frame,
            current: true,
            once_a_scene: failure.once_a_scene,
        };
        self.logged += 1;
        if !self.quiet {
            let more = if err.is_load_error() {
                ""
            } else if err.once_a_scene {
                " (logged once a scene)"
            } else {
                " (logged again at 10, 100, 1000... times)"
            };
            tracing::warn!("[script] {err}{more}");
        }
        if self.kept.len() < MAX_KEPT {
            self.kept.push(err);
        } else {
            self.dropped += 1;
        }
    }

    /// A frame begins.
    pub fn next_frame(&mut self) {
        self.frame += 1;
    }

    /// The frame the next failure is counted in.
    pub fn frame(&self) -> u64 {
        self.frame
    }

    /// `file` was reloaded: what failed in it before is no longer what runs.
    pub fn file_reloaded(&mut self, file: &str) {
        for err in self.kept.iter_mut().filter(|e| e.file == file) {
            err.current = false;
        }
    }

    /// The scene's scripts are gone (a transition): nothing kept is current.
    pub fn scene_cleared(&mut self) {
        for err in &mut self.kept {
            err.current = false;
        }
    }

    /// Every distinct error kept, in the order they first happened.
    pub fn all(&self) -> &[ScriptError] {
        &self.kept
    }

    /// The errors of the scripts running now, most recent first: what the
    /// error card shows.
    pub fn on_screen(&self) -> Vec<ScriptError> {
        let mut list: Vec<ScriptError> = self.kept.iter().filter(|e| e.current).cloned().collect();
        list.sort_by_key(|e| std::cmp::Reverse(e.last_frame));
        list
    }

    /// Distinct errors, kept or not.
    pub fn distinct(&self) -> u64 {
        self.kept.len() as u64 + self.dropped
    }

    /// Every failure counted.
    pub fn total(&self) -> u64 {
        self.total
    }

    pub fn is_empty(&self) -> bool {
        self.total == 0
    }

    /// The run's account: a `[script] N errors` line, then one line each.
    pub fn summary(&self) -> Vec<String> {
        let n = self.distinct();
        if n == 0 {
            return vec!["[script] no errors".to_string()];
        }
        let mut lines = vec![format!(
            "[script] {n} error{} ({} time{} in all)",
            if n == 1 { "" } else { "s" },
            self.total,
            if self.total == 1 { "" } else { "s" }
        )];
        for err in &self.kept {
            let first = err.message.lines().next().unwrap_or("");
            let times = if err.count == 1 {
                format!("once, frame {}", err.first_frame)
            } else {
                format!("{} times from frame {}", err.count, err.first_frame)
            };
            lines.push(format!(
                "[script]   {}: {}: {first} ({times})",
                err.location(),
                err.context()
            ));
        }
        if self.dropped > 0 {
            lines.push(format!(
                "[script]   ...and {} more, each logged when it first happened",
                self.dropped
            ));
        }
        lines
    }

    /// Why a run under `--strict-scripts` fails, or `None` with no error:
    /// how many, and the first.
    pub fn strict_failure(&self) -> Option<String> {
        let n = self.distinct();
        if n == 0 {
            return None;
        }
        let first = self
            .kept
            .first()
            .map(|e| format!("; the first: {e}"))
            .unwrap_or_default();
        Some(format!(
            "{n} script error{} under --strict-scripts{first}",
            if n == 1 { "" } else { "s" }
        ))
    }

    /// Fold another run's errors into these (`flint render`'s sweep cells,
    /// each a game of its own): the same error once, at the higher count.
    pub fn merge(&mut self, other: &ScriptErrors) {
        for err in &other.kept {
            let at = self.kept.iter().position(|e| {
                e.file == err.file
                    && e.callback == err.callback
                    && e.line == err.line
                    && e.message == err.message
            });
            match at {
                Some(i) => {
                    let mine = &mut self.kept[i];
                    if err.count > mine.count {
                        self.total += err.count - mine.count;
                        mine.count = err.count;
                    }
                }
                None if self.kept.len() < MAX_KEPT => {
                    self.total += err.count;
                    self.kept.push(err.clone());
                }
                None => {
                    self.total += err.count;
                    self.dropped += 1;
                }
            }
        }
        self.dropped = self.dropped.max(other.dropped);
    }

    /// Count without logging (tests of the counting).
    #[doc(hidden)]
    pub fn set_quiet(&mut self, quiet: bool) {
        self.quiet = quiet;
    }

    /// How many lines [`Self::record`] has logged (or would have, quiet):
    /// what a test of how often an error is said reads (#856).
    #[doc(hidden)]
    pub fn lines_logged(&self) -> u64 {
        self.logged
    }
}

/// 10, 100, 1000...
fn is_milestone(count: u64) -> bool {
    let mut n = count;
    if n < 10 {
        return false;
    }
    while n.is_multiple_of(10) {
        n /= 10;
    }
    n == 1
}

#[cfg(test)]
mod tests {
    use super::*;

    fn failure(message: &str, line: usize) -> Failure {
        Failure {
            file: "scripts/hud.rhai".into(),
            entity_name: "hud_controller".into(),
            callback: "on_update".into(),
            function: None,
            line: Some(line),
            column: Some(5),
            message: message.into(),
            once_a_scene: false,
        }
    }

    #[test]
    fn an_error_is_logged_at_one_ten_and_a_hundred() {
        let mut errors = ScriptErrors::new();
        errors.set_quiet(true);
        for _ in 0..600 {
            errors.next_frame();
            errors.record(failure("boom", 1));
        }
        assert_eq!(errors.lines_logged(), 3, "1, 10, 100");
        // A transition doesn't change that: it's counted on from 600
        errors.scene_cleared();
        for _ in 0..600 {
            errors.record(failure("boom", 1));
        }
        assert_eq!(errors.lines_logged(), 4, "1000");
    }

    /// A call into a module that isn't enabled (#856): once a scene, and
    /// once again after its file reloads.
    #[test]
    fn a_once_a_scene_error_is_logged_once_until_the_scene_changes() {
        let mut errors = ScriptErrors::new();
        errors.set_quiet(true);
        let gated = Failure {
            once_a_scene: true,
            ..failure("tune_living: the living module is not enabled here", 4)
        };
        for _ in 0..600 {
            errors.next_frame();
            errors.record(gated.clone());
        }
        assert_eq!(errors.lines_logged(), 1);
        assert_eq!(errors.all()[0].count, 600, "every call still counted");
        assert!(errors.all()[0].once_a_scene);

        errors.scene_cleared();
        assert!(errors.on_screen().is_empty());
        for _ in 0..600 {
            errors.record(gated.clone());
        }
        assert_eq!(errors.lines_logged(), 2, "once more in the next scene");
        assert_eq!(errors.on_screen().len(), 1);

        errors.file_reloaded("scripts/hud.rhai");
        errors.record(gated.clone());
        errors.record(gated);
        assert_eq!(errors.lines_logged(), 3, "and once after a reload");
        assert_eq!(errors.total(), 1202);
    }

    #[test]
    fn the_same_error_is_one_entry_with_a_count() {
        let mut errors = ScriptErrors::new();
        errors.set_quiet(true);
        for _ in 0..3 {
            errors.next_frame();
            errors.record(failure("Function not found: foo ()", 42));
        }
        errors.record(failure("Function not found: bar ()", 43));
        assert_eq!(errors.all().len(), 2);
        assert_eq!(errors.total(), 4);
        let first = &errors.all()[0];
        assert_eq!(first.count, 3);
        assert_eq!((first.first_frame, first.last_frame), (1, 3));
        assert_eq!(
            first.to_string(),
            "scripts/hud.rhai:42:5: hud_controller, on_update: Function not found: foo ()"
        );
    }

    #[test]
    fn another_entity_failing_alike_is_named_on_the_same_entry() {
        let mut errors = ScriptErrors::new();
        errors.set_quiet(true);
        errors.record(failure("boom", 1));
        let mut other = failure("boom", 1);
        other.entity_name = "hud_2".into();
        errors.record(other.clone());
        errors.record(other);
        assert_eq!(errors.all().len(), 1);
        assert_eq!(errors.all()[0].also_on, vec!["hud_2".to_string()]);
        assert!(errors.all()[0].context().ends_with("(+1 more)"));
    }

    #[test]
    fn milestones_are_the_powers_of_ten() {
        let at: Vec<u64> = (1..=10_000).filter(|n| is_milestone(*n)).collect();
        assert_eq!(at, vec![10, 100, 1000, 10_000]);
    }

    #[test]
    fn a_compile_message_gives_up_its_position() {
        let f = Failure::compile("a.rhai", "door", "Expecting ';' (line 3, position 9)");
        assert_eq!(f.message, "Expecting ';'");
        assert_eq!((f.line, f.column), (Some(3), Some(9)));
        let f = Failure::compile("a.rhai", "door", "Failed to read a.rhai: gone");
        assert_eq!((f.line, f.column), (None, None));
    }

    #[test]
    fn a_reload_takes_its_files_errors_off_the_screen_but_not_out_of_the_count() {
        let mut errors = ScriptErrors::new();
        errors.set_quiet(true);
        errors.record(failure("boom", 1));
        errors.file_reloaded("scripts/hud.rhai");
        assert!(errors.on_screen().is_empty());
        assert_eq!(errors.distinct(), 1);
        // Failing again after the reload puts it back
        errors.record(failure("boom", 1));
        assert_eq!(errors.on_screen().len(), 1);
    }

    #[test]
    fn the_summary_says_how_many() {
        let mut errors = ScriptErrors::new();
        assert_eq!(errors.summary(), vec!["[script] no errors"]);
        errors.set_quiet(true);
        errors.record(failure("boom", 1));
        errors.record(failure("boom", 1));
        let lines = errors.summary();
        assert_eq!(lines[0], "[script] 1 error (2 times in all)");
        assert!(lines[1].contains("hud.rhai:1:5: hud_controller, on_update: boom (2 times"));
    }

    #[test]
    fn past_the_cap_errors_are_counted_not_kept() {
        let mut errors = ScriptErrors::new();
        errors.set_quiet(true);
        for i in 0..(MAX_KEPT + 3) {
            errors.record(failure("boom", i + 1));
        }
        assert_eq!(errors.all().len(), MAX_KEPT);
        assert_eq!(errors.distinct(), MAX_KEPT as u64 + 3);
    }

    #[test]
    fn merging_keeps_the_higher_count() {
        let mut a = ScriptErrors::new();
        a.set_quiet(true);
        a.record(failure("boom", 1));
        let mut b = a.clone();
        b.record(failure("boom", 1));
        b.record(failure("bang", 2));
        a.merge(&b);
        assert_eq!(a.all().len(), 2);
        assert_eq!(a.all()[0].count, 2);
        assert_eq!(a.total(), 3);
    }
}
