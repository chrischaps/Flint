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

use crate::commands::absent::MissingPackage;
use serde_json::{json, Value};
use std::process::ExitCode;

// The state and the answer's forms are `flint_package::cli`'s (#1553), so a
// package's commands answer through the same switches as the engine's.
pub use flint_package::cli::{
    document, document_text, emit, emitted, failed, format_or_json, json, set_json,
    stdout_reserved, when_json, Answer, Failed, Report,
};

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
        // A command this build left out (#1333): exit 2, as a command line
        // clap can't take, and the document names the package
        Err(mut e) if e.downcast_ref::<MissingPackage>().is_some() => {
            // A package's own `missing` can't tell a folder the launchers
            // don't know yet (#1587); the manifests compiled in can
            crate::commands::absent::judge(&mut e);
            if json() && !emitted() {
                let mut doc = error_document(&e);
                if let Some(m) = e.downcast_ref::<MissingPackage>() {
                    doc["error"]["package"] = json!(m.package);
                    doc["error"]["hint"] = json!(m.hint());
                    doc["error"]["unsynced"] = json!(m.unsynced);
                }
                document(&doc);
            }
            eprintln!("Error: {e}");
            ExitCode::from(crate::commands::absent::EXIT)
        }
        // A package the project lists that this binary lacks (#1335):
        // exit 2 too, naming `flint build`
        Err(e)
            if e.downcast_ref::<flint_player::packages::Unbuilt>()
                .is_some() =>
        {
            if json() && !emitted() {
                let mut doc = error_document(&e);
                if let Some(u) = e.downcast_ref::<flint_player::packages::Unbuilt>() {
                    doc["error"]["packages"] = json!(u.packages);
                    doc["error"]["hint"] = json!(u.hint());
                    doc["error"]["unsynced"] = json!(u.unsynced);
                }
                document(&doc);
            }
            eprintln!("Error: {e}");
            ExitCode::from(crate::commands::absent::EXIT)
        }
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
