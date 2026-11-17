//! One way out of a command (#710).
//!
//! Every command answers in one of two forms: text for a person, or, under
//! the global `--json`, exactly one JSON document on stdout. Everything else
//! a command prints while it works goes through [`say!`] or [`progress!`],
//! which keep stdout for the answer, and a command that fails returns an
//! error, never `std::process::exit`:
//!
//! - [`emit`] prints a [`Report`] in the form asked for and, when the report
//!   is a failure (a check with errors, a lookup with no answer), returns
//!   [`Failed`], which `main` turns into exit 1 with nothing more said.
//! - Any other error leaves through [`finish`]: its message on stderr, exit
//!   1, and under `--json`, when the command had not printed its document,
//!   an error document on stdout (`{"error": {"message", "causes"}}`), so a
//!   caller always has one document to parse.
//!
//! ```text
//! say!(...)       a human line: stdout, or stderr when stdout is the document
//! progress!(...)  a line about work in flight: always stderr
//! emit(&report)   the answer; Err(Failed) when it says the run failed
//! document(&v)    a JSON document on stdout, for a command that builds one
//! ```

use serde_json::{json, Value};
use std::process::ExitCode;
use std::sync::atomic::{AtomicBool, Ordering};

/// `--json` was given: stdout is one JSON document.
static JSON: AtomicBool = AtomicBool::new(false);
/// Stdout holds a document (`--json`, or a command's own `--report json`),
/// so the human lines go to stderr.
static STDOUT_RESERVED: AtomicBool = AtomicBool::new(false);
/// A document has been printed: an error after it adds none.
static EMITTED: AtomicBool = AtomicBool::new(false);

/// Turns `--json` on for the process (main does, from the global flag).
pub fn set_json(on: bool) {
    JSON.store(on, Ordering::Relaxed);
    if on {
        reserve_stdout();
    }
}

/// Whether `--json` was given.
pub fn json() -> bool {
    JSON.load(Ordering::Relaxed)
}

/// A command's own `--format`, which `--json` overrides: `json` under
/// `--json`, else what was asked.
pub fn format_or_json(format: &str) -> &str {
    if json() {
        "json"
    } else {
        format
    }
}

/// Stdout belongs to a document from here on: [`say!`] writes to stderr.
/// `flint render --report json` reserves it without `--json`.
pub fn reserve_stdout() {
    STDOUT_RESERVED.store(true, Ordering::Relaxed);
}

/// Whether the human lines go to stderr.
pub fn stdout_reserved() -> bool {
    STDOUT_RESERVED.load(Ordering::Relaxed)
}

/// A human line: on stdout, unless stdout is a JSON document, then stderr.
macro_rules! say {
    () => {
        if $crate::output::stdout_reserved() { eprintln!() } else { println!() }
    };
    ($($t:tt)*) => {
        if $crate::output::stdout_reserved() {
            eprintln!($($t)*)
        } else {
            println!($($t)*)
        }
    };
}
pub(crate) use say;

/// A line about work in flight ("Generating ...", "Packing ..."): always
/// stderr, so it never mixes with an answer.
macro_rules! progress {
    ($($t:tt)*) => { eprintln!($($t)*) };
}
pub(crate) use progress;

/// A command's answer, in both forms.
pub trait Report {
    /// What a person reads, printed as is (a final newline is added if
    /// missing).
    fn text(&self) -> String;
    /// The same answer as one JSON document.
    fn json(&self) -> Value;
    /// The answer is a failure: the run exits 1 after printing it.
    fn failed(&self) -> bool {
        false
    }
}

/// A report built in place: the text and the document side by side.
pub struct Answer {
    pub text: String,
    pub json: Value,
    pub failed: bool,
}

impl Answer {
    pub fn new(text: impl Into<String>, json: Value) -> Self {
        Self {
            text: text.into(),
            json,
            failed: false,
        }
    }

    /// The same answer, exiting 1 when `failed`.
    pub fn failed_if(mut self, failed: bool) -> Self {
        self.failed = failed;
        self
    }
}

impl Report for Answer {
    fn text(&self) -> String {
        self.text.clone()
    }
    fn json(&self) -> Value {
        self.json.clone()
    }
    fn failed(&self) -> bool {
        self.failed
    }
}

/// Prints `report` in the form asked for. `Err(Failed)` when the report is
/// a failure: return it from the command.
pub fn emit(report: &impl Report) -> anyhow::Result<()> {
    if json() {
        document(&report.json());
    } else {
        let text = report.text();
        if text.ends_with('\n') || text.is_empty() {
            print!("{text}");
        } else {
            println!("{text}");
        }
    }
    if report.failed() {
        Err(Failed.into())
    } else {
        Ok(())
    }
}

/// Prints `value` as a JSON document on stdout. A command prints one.
pub fn document(value: &Value) {
    EMITTED.store(true, Ordering::Relaxed);
    // A Value always serialises.
    println!(
        "{}",
        serde_json::to_string_pretty(value).unwrap_or_else(|_| "null".into())
    );
}

/// Under `--json`, prints the document `f` builds; otherwise nothing (the
/// command said it in [`say!`] lines). For a command whose text is written
/// as it goes.
pub fn when_json(f: impl FnOnce() -> Value) {
    if json() {
        document(&f());
    }
}

/// The error to return when the command's answer, already printed, is a
/// failure: exit 1 and nothing more said.
pub fn failed() -> anyhow::Error {
    Failed.into()
}

/// Prints an already-serialised document (a command whose report is a
/// typed struct with its own serialiser).
pub fn document_text(text: &str) {
    EMITTED.store(true, Ordering::Relaxed);
    println!("{}", text.trim_end());
}

/// Whether a document has been printed this run.
pub fn emitted() -> bool {
    EMITTED.load(Ordering::Relaxed)
}

/// The command has said why it failed (its report is the answer); `main`
/// exits 1 and prints nothing more.
#[derive(Debug)]
pub struct Failed;

impl std::fmt::Display for Failed {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("the command reported a failure")
    }
}

impl std::error::Error for Failed {}

/// The document an error is under `--json`.
pub fn error_document(error: &anyhow::Error) -> Value {
    let causes: Vec<String> = error.chain().skip(1).map(|c| c.to_string()).collect();
    json!({
        "error": {
            "message": error.to_string(),
            "causes": causes,
        }
    })
}

/// The one way a run ends: exit 0, or exit 1 with the error on stderr (and
/// under `--json`, as a document on stdout if none was printed).
pub fn finish(result: anyhow::Result<()>) -> ExitCode {
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) if e.downcast_ref::<Failed>().is_some() => ExitCode::FAILURE,
        Err(e) => {
            if json() && !emitted() {
                document(&error_document(&e));
            }
            eprintln!("Error: {e:?}");
            ExitCode::FAILURE
        }
    }
}

/// `--json` given to a command that opens a window: refused, so a caller
/// gets an error document rather than a window and an empty stdout.
pub fn refuse_interactive(command: &str) -> anyhow::Result<()> {
    if json() {
        anyhow::bail!(
            "flint {command} opens a window and has no JSON form; drop --json \
             (`flint render` and `flint check` answer as JSON)"
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_error_document_names_the_message_and_its_causes() {
        let e = anyhow::anyhow!("inner").context("outer");
        let doc = error_document(&e);
        assert_eq!(doc["error"]["message"], "outer");
        assert_eq!(doc["error"]["causes"], json!(["inner"]));
    }

    #[test]
    fn a_failed_report_is_the_failed_error() {
        let answer = Answer::new("x", json!({})).failed_if(true);
        assert!(answer.failed());
        let e: anyhow::Error = Failed.into();
        assert!(e.downcast_ref::<Failed>().is_some());
    }
}
