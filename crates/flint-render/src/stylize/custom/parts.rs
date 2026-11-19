//! A check's error text taken apart again (#567): the file, line and
//! column, the message, the quoted line and the hints, for
//! `flint style check --json`.
//!
//! Errors stay `String`s everywhere a host shows them; this reads the
//! shapes [`locate`](super::locate) and the loaders write, and the tests
//! below hold the two to each other.

use serde::Serialize;

/// One error of a style's check, in parts.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct ErrorParts {
    /// The file it is in, as the message spells it (`/` on every
    /// platform): the `.shader.toml` or one of its WGSL files.
    pub file: String,
    /// 1-based, in that file, when the error has a place.
    pub line: Option<u32>,
    /// 1-based, on that line.
    pub col: Option<u32>,
    /// What is wrong. Several lines when the error is (a TOML parse
    /// error quotes the description itself).
    pub message: String,
    /// The user's line the error is on, as `locate` quotes it.
    pub source: Option<String>,
    /// What to do about it, one a line (`hint: ` dropped).
    pub hints: Vec<String>,
}

/// Every error in `error` (several passes' are one after another, a line
/// apart). An error starts on a line that leads with a style's file
/// (`<path>.wgsl:` or `<path>.shader.toml:`); a text that doesn't is one
/// error in `fallback_file`.
pub fn error_parts(error: &str, fallback_file: &str) -> Vec<ErrorParts> {
    let mut out: Vec<ErrorParts> = Vec::new();
    for line in error.lines() {
        if let Some(parts) = lead(line) {
            out.push(parts);
            continue;
        }
        let Some(last) = out.last_mut() else {
            out.push(ErrorParts {
                file: fallback_file.to_string(),
                message: line.trim_end().to_string(),
                ..ErrorParts::default()
            });
            continue;
        };
        let indented = line.trim_start();
        if let Some(hint) = line.strip_prefix("    hint: ") {
            last.hints.push(hint.trim_end().to_string());
        } else if let Some(quoted) = quote(line, last.line) {
            last.source = Some(quoted);
        } else if !indented.is_empty() || !last.message.is_empty() {
            if !last.message.is_empty() {
                last.message.push('\n');
            }
            last.message.push_str(line.trim_end());
        }
    }
    for e in &mut out {
        e.message = e.message.trim_end().to_string();
        // A TOML parse error has its place in its message
        if e.line.is_none() {
            if let Some((line, col)) = toml_place(&e.message) {
                e.line = Some(line);
                e.col = Some(col);
            }
        }
    }
    out
}

/// `<file>[:line[:col]]: message`, when `line` leads with a style's file.
fn lead(line: &str) -> Option<ErrorParts> {
    let end = [".shader.toml:", ".wgsl:"]
        .iter()
        .filter_map(|ext| line.find(ext).map(|at| at + ext.len() - 1))
        .min()?;
    let file = &line[..end];
    // `styles/a.wgsl` leads; `the x.wgsl: ...` inside a message doesn't
    if file.contains(": ") || file.starts_with(char::is_whitespace) {
        return None;
    }
    let mut rest = &line[end + 1..];
    let mut numbers = Vec::new();
    while numbers.len() < 2 {
        let digits = rest.chars().take_while(char::is_ascii_digit).count();
        if digits == 0 || !rest[digits..].starts_with(':') {
            break;
        }
        numbers.push(rest[..digits].parse::<u32>().ok()?);
        rest = &rest[digits + 1..];
    }
    Some(ErrorParts {
        file: file.to_string(),
        line: numbers.first().copied(),
        col: numbers.get(1).copied(),
        message: rest.trim().to_string(),
        ..ErrorParts::default()
    })
}

/// `    <line> | <text>`: the user's line `locate` quotes under its error.
fn quote(line: &str, at: Option<u32>) -> Option<String> {
    let body = line.strip_prefix("    ")?;
    let (number, text) = body.split_once(" | ")?;
    let number = number.trim().parse::<u32>().ok()?;
    (at.is_none() || at == Some(number)).then(|| text.to_string())
}

/// `TOML parse error at line 3, column 9`.
fn toml_place(message: &str) -> Option<(u32, u32)> {
    let (_, after) = message.split_once("at line ")?;
    let (line, after) = after.split_once(", column ")?;
    let col: String = after.chars().take_while(char::is_ascii_digit).collect();
    Some((line.trim().parse().ok()?, col.parse().ok()?))
}
