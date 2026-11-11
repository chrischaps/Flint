//! A [`Report`] as text for a person, grouped by file, or as one JSON
//! document for an agent. Both hold every diagnostic; neither prints.

use super::Report;
use flint_core::diagnostic::{self, Diagnostic};

/// How many of a missing file's tried paths the text shows. The JSON has
/// them all.
const TRIED_SHOWN: usize = 6;

/// The diagnostics of one file, in the order found, under the file's name.
fn by_file(diagnostics: &[Diagnostic]) -> Vec<(String, Vec<&Diagnostic>)> {
    let mut groups: Vec<(String, Vec<&Diagnostic>)> = Vec::new();
    for d in diagnostics {
        let file = match &d.file {
            Some(file) => file.to_string_lossy().into_owned(),
            None => "(project)".to_string(),
        };
        match groups.iter_mut().find(|(name, _)| *name == file) {
            Some((_, list)) => list.push(d),
            None => groups.push((file, vec![d])),
        }
    }
    groups
}

/// One diagnostic's lines, indented under its file: what it is, the hint,
/// and where the engine looked.
pub fn diagnostic_lines(d: &Diagnostic) -> String {
    // A parse error's message runs over several lines
    let mut out = format!("  {}\n", d.to_string().replace('\n', "\n    "));
    if let Some(hint) = &d.hint {
        out.push_str(&format!("      hint: {hint}\n"));
    }
    if !d.tried.is_empty() {
        out.push_str("      tried:\n");
        for path in d.tried.iter().take(TRIED_SHOWN) {
            out.push_str(&format!("        {}\n", path.to_string_lossy()));
        }
        if d.tried.len() > TRIED_SHOWN {
            out.push_str(&format!(
                "        ... and {} more (--json lists them all)\n",
                d.tried.len() - TRIED_SHOWN
            ));
        }
    }
    out
}

/// The report for a person: each file with what is wrong in it, errors
/// first, then one line that counts.
pub fn text(report: &Report) -> String {
    let mut out = String::new();
    for (file, mut list) in by_file(&report.diagnostics) {
        list.sort_by_key(|d| d.severity);
        let owned: Vec<Diagnostic> = list.iter().map(|d| (*d).clone()).collect();
        out.push_str(&format!("{file}: {}\n", diagnostic::counts_line(&owned)));
        for d in list {
            out.push_str(&diagnostic_lines(d));
        }
        out.push('\n');
    }
    out.push_str(&format!("{}\n", summary(report)));
    out
}

/// `checked 3 scenes and 12 files: 2 errors, 1 warning`.
pub fn summary(report: &Report) -> String {
    let plural = |n: usize, word: &str| format!("{n} {word}{}", if n == 1 { "" } else { "s" });
    format!(
        "checked {} and {}: {}",
        plural(report.scenes.len(), "scene"),
        plural(report.files, "file"),
        report.counts()
    )
}

/// The report as one JSON document, in the shape `flint validate --format
/// json` has (`valid`, `summary`, `errors`, `warnings`, `diagnostics`), with
/// the scenes that were checked.
pub fn json(report: &Report) -> serde_json::Value {
    serde_json::json!({
        "valid": !report.has_errors(),
        "summary": summary(report),
        "errors": diagnostic::count(&report.diagnostics, diagnostic::Severity::Error),
        "warnings": diagnostic::count(&report.diagnostics, diagnostic::Severity::Warning),
        "scenes": report.scenes,
        "files": report.files,
        "scripts": report.scripts,
        "diagnostics": report.diagnostics,
    })
}
