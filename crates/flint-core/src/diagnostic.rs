//! One shape for everything that is wrong with a scene (ADR 0115).
//!
//! A [`Diagnostic`] says how bad, which rule ([`codes`], stable kebab-case
//! strings an agent can act on), and where: file, entity, component and the
//! dotted path of the field. Schema validation (`flint-schema`) and the scene
//! pass (`flint_scene::diagnose`) both produce it, `flint validate` prints the
//! whole list as text or JSON, and the player prints a summary at load.
//!
//! ```
//! use flint_core::diagnostic::{codes, Diagnostic, Severity};
//!
//! let d = Diagnostic::error(codes::BAD_TYPE, "expected f32, got string")
//!     .entity("crate")
//!     .component("collider")
//!     .field("friction");
//! assert_eq!(d.severity, Severity::Error);
//! assert_eq!(
//!     d.to_string(),
//!     "error[bad-type] entity 'crate' collider.friction: expected f32, got string"
//! );
//! ```

use serde::Serialize;
use std::fmt;
use std::path::{Path, PathBuf};

/// How bad a [`Diagnostic`] is. `flint validate` fails on any `Error`; the
/// player runs either way.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Severity {
    Error,
    Warning,
}

impl Severity {
    pub fn as_str(self) -> &'static str {
        match self {
            Severity::Error => "error",
            Severity::Warning => "warning",
        }
    }
}

impl fmt::Display for Severity {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// The stable codes. A code never changes its meaning; a new problem gets a
/// new code and a row in [`ALL`](codes::ALL) (which the book's table and
/// `every_code_is_listed_once` follow).
pub mod codes {
    use super::Severity;

    /// A field the component's schema doesn't declare. A warning (games add
    /// fields that scripts read); `--strict` makes it an error.
    pub const UNKNOWN_FIELD: &str = "unknown-field";
    /// A value of the wrong TOML type for its field.
    pub const BAD_TYPE: &str = "bad-type";
    /// A string that is none of an enum field's values.
    pub const BAD_ENUM_VALUE: &str = "bad-enum-value";
    /// A number outside the field's `min`/`max`.
    pub const OUT_OF_RANGE: &str = "out-of-range";
    /// A `required` field that is not written.
    pub const MISSING_FIELD: &str = "missing-field";
    /// A component table no loaded schema directory declares.
    pub const UNKNOWN_COMPONENT: &str = "unknown-component";
    /// An `archetype = "..."` no loaded schema directory declares.
    pub const UNKNOWN_ARCHETYPE: &str = "unknown-archetype";
    /// The file is not TOML, or not a scene.
    pub const TOML_PARSE: &str = "toml-parse";
    /// The file could not be read.
    pub const FILE_UNREADABLE: &str = "file-unreadable";
    /// A `[prefabs.<name>]` that could not be expanded.
    pub const BAD_PREFAB: &str = "bad-prefab";
    /// No schema was loaded, so nothing could be checked.
    pub const NO_SCHEMAS: &str = "no-schemas";
    /// A component that does nothing without another the entity lacks: a
    /// `collider` with no `rigidbody`. Found by the system that reads it,
    /// while the game runs (#685).
    pub const NEEDS_COMPONENT: &str = "needs-component";

    /// Every code, the severity it is raised with, and what it means.
    pub const ALL: &[(&str, Severity, &str)] = &[
        (
            UNKNOWN_FIELD,
            Severity::Warning,
            "a field the component's schema does not declare",
        ),
        (
            BAD_TYPE,
            Severity::Error,
            "a value of the wrong type for its field",
        ),
        (
            BAD_ENUM_VALUE,
            Severity::Error,
            "a string that is none of the field's allowed values",
        ),
        (
            OUT_OF_RANGE,
            Severity::Error,
            "a number outside the field's min/max",
        ),
        (
            MISSING_FIELD,
            Severity::Error,
            "a required field that is not written",
        ),
        (
            UNKNOWN_COMPONENT,
            Severity::Error,
            "a component no loaded schema directory declares",
        ),
        (
            UNKNOWN_ARCHETYPE,
            Severity::Error,
            "an archetype no loaded schema directory declares",
        ),
        (
            TOML_PARSE,
            Severity::Error,
            "the file is not valid TOML, or not a scene",
        ),
        (
            FILE_UNREADABLE,
            Severity::Error,
            "the file could not be read",
        ),
        (
            BAD_PREFAB,
            Severity::Error,
            "a prefab instance that could not be expanded",
        ),
        (
            NO_SCHEMAS,
            Severity::Warning,
            "no schema was loaded, so components were not checked",
        ),
        (
            NEEDS_COMPONENT,
            Severity::Warning,
            "a component that does nothing without another the entity lacks",
        ),
    ];
}

/// One thing wrong with a scene: see the module docs.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Diagnostic {
    pub severity: Severity,
    /// One of [`codes`].
    pub code: &'static str,
    pub file: Option<PathBuf>,
    pub entity: Option<String>,
    pub component: Option<String>,
    /// Dotted path inside the component: `friction`, `points[2]`, `shear.xz`.
    pub field: Option<String>,
    pub message: String,
    /// What to do about it: "did you mean `light`?", the allowed values.
    pub hint: Option<String>,
}

impl Diagnostic {
    pub fn new(severity: Severity, code: &'static str, message: impl Into<String>) -> Self {
        Self {
            severity,
            code,
            file: None,
            entity: None,
            component: None,
            field: None,
            message: message.into(),
            hint: None,
        }
    }

    pub fn error(code: &'static str, message: impl Into<String>) -> Self {
        Self::new(Severity::Error, code, message)
    }

    pub fn warning(code: &'static str, message: impl Into<String>) -> Self {
        Self::new(Severity::Warning, code, message)
    }

    pub fn file(mut self, file: impl Into<PathBuf>) -> Self {
        self.file = Some(file.into());
        self
    }

    pub fn entity(mut self, entity: impl Into<String>) -> Self {
        self.entity = Some(entity.into());
        self
    }

    pub fn component(mut self, component: impl Into<String>) -> Self {
        self.component = Some(component.into());
        self
    }

    pub fn field(mut self, field: impl Into<String>) -> Self {
        self.field = Some(field.into());
        self
    }

    pub fn hint(mut self, hint: impl Into<String>) -> Self {
        self.hint = Some(hint.into());
        self
    }

    /// The hint, if there is something to suggest.
    pub fn hint_opt(mut self, hint: Option<String>) -> Self {
        self.hint = hint;
        self
    }

    pub fn is_error(&self) -> bool {
        self.severity == Severity::Error
    }

    /// Where it is, without the file: `entity 'crate' collider.friction`.
    /// Empty when the diagnostic is about the file as a whole.
    pub fn location(&self) -> String {
        let mut parts = Vec::new();
        if let Some(entity) = &self.entity {
            parts.push(format!("entity '{entity}'"));
        }
        match (&self.component, &self.field) {
            (Some(component), Some(field)) => parts.push(format!("{component}.{field}")),
            (Some(component), None) => parts.push(format!("component '{component}'")),
            (None, Some(field)) => parts.push(format!("field '{field}'")),
            (None, None) => {}
        }
        parts.join(" ")
    }

    /// The whole thing on one line, file first, hint last: what a log prints.
    pub fn one_line(&self) -> String {
        let mut line = String::new();
        if let Some(file) = &self.file {
            line.push_str(&shown(file));
            line.push_str(": ");
        }
        line.push_str(&self.to_string());
        if let Some(hint) = &self.hint {
            line.push_str(&format!(" ({hint})"));
        }
        line
    }
}

/// `error[bad-type] entity 'crate' collider.friction: expected f32, got
/// string`: severity, code, place and message. The file and the hint are the
/// printer's to place ([`Diagnostic::one_line`], `flint validate`).
impl fmt::Display for Diagnostic {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}[{}]", self.severity, self.code)?;
        let location = self.location();
        if !location.is_empty() {
            write!(f, " {location}")?;
        }
        write!(f, ": {}", self.message)
    }
}

fn shown(path: &Path) -> String {
    path.to_string_lossy().replace('\\', "/")
}

/// How many of `diagnostics` are of `severity`.
pub fn count(diagnostics: &[Diagnostic], severity: Severity) -> usize {
    diagnostics
        .iter()
        .filter(|d| d.severity == severity)
        .count()
}

/// Whether any of `diagnostics` is an error: what `flint validate` fails on.
pub fn has_errors(diagnostics: &[Diagnostic]) -> bool {
    diagnostics.iter().any(Diagnostic::is_error)
}

/// `--strict` (and `[project] strict = true`): an unknown field is an error.
pub fn promote_strict(diagnostics: &mut [Diagnostic]) {
    for d in diagnostics {
        if d.code == codes::UNKNOWN_FIELD {
            d.severity = Severity::Error;
        }
    }
}

/// Errors first, each group in the order it was found.
pub fn sort_errors_first(diagnostics: &mut [Diagnostic]) {
    diagnostics.sort_by_key(|d| d.severity);
}

/// `2 errors, 1 warning`, or `no problems`.
pub fn counts_line(diagnostics: &[Diagnostic]) -> String {
    let plural = |n: usize, word: &str| format!("{n} {word}{}", if n == 1 { "" } else { "s" });
    let errors = count(diagnostics, Severity::Error);
    let warnings = count(diagnostics, Severity::Warning);
    match (errors, warnings) {
        (0, 0) => "no problems".to_string(),
        (e, 0) => plural(e, "error"),
        (0, w) => plural(w, "warning"),
        (e, w) => format!("{}, {}", plural(e, "error"), plural(w, "warning")),
    }
}

/// What a loader prints once per load (ADR 0115 decision 2): one summary
/// line, then the first `limit` diagnostics, errors first, and how many were
/// left out. Empty when there is nothing to say. `scene` names what was
/// loaded.
pub fn summary_lines(scene: &str, diagnostics: &[Diagnostic], limit: usize) -> Vec<String> {
    if diagnostics.is_empty() {
        return Vec::new();
    }
    let mut ordered = diagnostics.to_vec();
    sort_errors_first(&mut ordered);
    let mut lines = vec![format!(
        "{scene}: {} (the full list: flint validate {scene})",
        counts_line(diagnostics)
    )];
    for d in ordered.iter().take(limit) {
        let mut line = format!("  {d}");
        if let Some(hint) = &d.hint {
            line.push_str(&format!(" ({hint})"));
        }
        lines.push(line);
    }
    if ordered.len() > limit {
        lines.push(format!("  ... and {} more", ordered.len() - limit));
    }
    lines
}

/// How many diagnostics a load prints after its summary line.
pub const SUMMARY_LIMIT: usize = 5;

/// [`summary_lines`] through `tracing::warn!`, [`SUMMARY_LIMIT`] of them:
/// what every scene load does with what it found. The scene still loads.
pub fn log_summary(scene: &str, diagnostics: &[Diagnostic]) {
    for line in summary_lines(scene, diagnostics, SUMMARY_LIMIT) {
        tracing::warn!("{line}");
    }
}

/// The candidate closest to `name`, if one is close enough to be what was
/// meant: the same letters in another case or with `-`/`_` moved wins, then
/// the smallest edit distance, within a third of the name's length (1 to 3).
pub fn closest<'a>(name: &str, candidates: impl IntoIterator<Item = &'a str>) -> Option<&'a str> {
    let fold = |s: &str| -> String {
        s.chars()
            .filter(|c| *c != '_' && *c != '-')
            .flat_map(char::to_lowercase)
            .collect()
    };
    let folded = fold(name);
    let limit = (name.chars().count() / 3).clamp(1, 3);
    let mut best: Option<(usize, &str)> = None;
    for candidate in candidates {
        if candidate == name {
            continue;
        }
        let distance = if fold(candidate) == folded {
            0
        } else {
            edit_distance(name, candidate)
        };
        if distance > limit {
            continue;
        }
        // Ties go to the name that sorts first, so the answer never depends
        // on a map's iteration order
        let better = match best {
            None => true,
            Some((d, held)) => distance < d || (distance == d && candidate < held),
        };
        if better {
            best = Some((distance, candidate));
        }
    }
    best.map(|(_, candidate)| candidate)
}

/// "did you mean `light`?" for the closest candidate, if any.
pub fn did_you_mean<'a>(
    name: &str,
    candidates: impl IntoIterator<Item = &'a str>,
) -> Option<String> {
    closest(name, candidates).map(|c| format!("did you mean `{c}`?"))
}

/// Edit distance where swapping two neighbouring letters counts once
/// (`lihgt` is one step from `light`), small and good enough for names.
pub fn edit_distance(a: &str, b: &str) -> usize {
    let a: Vec<char> = a.chars().collect();
    let b: Vec<char> = b.chars().collect();
    let mut before: Vec<usize> = vec![0; b.len() + 1];
    let mut prev: Vec<usize> = (0..=b.len()).collect();
    for i in 1..=a.len() {
        let mut row = vec![i; b.len() + 1];
        for j in 1..=b.len() {
            let cost = usize::from(a[i - 1] != b[j - 1]);
            row[j] = (prev[j] + 1).min(row[j - 1] + 1).min(prev[j - 1] + cost);
            if i > 1 && j > 1 && a[i - 1] == b[j - 2] && a[i - 2] == b[j - 1] {
                row[j] = row[j].min(before[j - 2] + 1);
            }
        }
        before = std::mem::replace(&mut prev, row);
    }
    prev[b.len()]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn display_names_entity_component_and_field() {
        let d = Diagnostic::warning(codes::UNKNOWN_FIELD, "no field `frction`")
            .file("scenes/a.scene.toml")
            .entity("crate")
            .component("collider")
            .field("frction")
            .hint("did you mean `friction`?");
        assert_eq!(
            d.to_string(),
            "warning[unknown-field] entity 'crate' collider.frction: no field `frction`"
        );
        assert_eq!(
            d.one_line(),
            "scenes/a.scene.toml: warning[unknown-field] entity 'crate' collider.frction: \
             no field `frction` (did you mean `friction`?)"
        );
        let whole_file = Diagnostic::error(codes::TOML_PARSE, "expected `=`");
        assert_eq!(whole_file.to_string(), "error[toml-parse]: expected `=`");
        let component = Diagnostic::error(codes::UNKNOWN_COMPONENT, "no such component")
            .entity("lamp")
            .component("lihgt");
        assert_eq!(
            component.to_string(),
            "error[unknown-component] entity 'lamp' component 'lihgt': no such component"
        );
    }

    #[test]
    fn json_is_flat_and_lowercase() {
        let d = Diagnostic::error(codes::BAD_TYPE, "expected f32, got string")
            .entity("crate")
            .component("collider")
            .field("friction");
        let text = toml::to_string(&d).unwrap();
        assert!(text.contains("severity = \"error\""), "{text}");
        assert!(text.contains("code = \"bad-type\""), "{text}");
        assert!(text.contains("field = \"friction\""), "{text}");
    }

    #[test]
    fn every_code_is_listed_once() {
        let mut seen = std::collections::HashSet::new();
        for (code, _, what) in codes::ALL {
            assert!(seen.insert(*code), "{code} is listed twice");
            assert!(!what.is_empty());
            assert!(
                code.chars().all(|c| c.is_ascii_lowercase() || c == '-'),
                "{code} is not kebab-case"
            );
        }
        assert_eq!(seen.len(), 12);
    }

    #[test]
    fn strict_promotes_only_unknown_fields() {
        let mut list = vec![
            Diagnostic::warning(codes::UNKNOWN_FIELD, "a"),
            Diagnostic::warning(codes::NO_SCHEMAS, "b"),
            Diagnostic::error(codes::BAD_TYPE, "c"),
        ];
        assert_eq!(counts_line(&list), "1 error, 2 warnings");
        promote_strict(&mut list);
        assert_eq!(count(&list, Severity::Error), 2);
        assert_eq!(list[1].severity, Severity::Warning);
        assert!(has_errors(&list));
        assert_eq!(counts_line(&[]), "no problems");
    }

    #[test]
    fn the_summary_is_one_line_then_the_first_few_errors_first() {
        let mut list: Vec<Diagnostic> = (0..7)
            .map(|i| Diagnostic::warning(codes::UNKNOWN_FIELD, format!("w{i}")))
            .collect();
        list.push(Diagnostic::error(codes::BAD_TYPE, "the error").hint("fix it"));
        let lines = summary_lines("a.scene.toml", &list, 3);
        assert_eq!(lines.len(), 5);
        assert_eq!(
            lines[0],
            "a.scene.toml: 1 error, 7 warnings (the full list: flint validate a.scene.toml)"
        );
        assert_eq!(lines[1], "  error[bad-type]: the error (fix it)");
        assert_eq!(lines[2], "  warning[unknown-field]: w0");
        assert_eq!(lines[4], "  ... and 5 more");
        assert!(summary_lines("a", &[], 3).is_empty());
    }

    #[test]
    fn closest_finds_typos_and_leaves_strangers() {
        let names = ["light", "collider", "rigidbody", "body_type", "transform"];
        let near = |name: &str| closest(name, names);
        assert_eq!(near("lihgt"), Some("light"));
        assert_eq!(near("colider"), Some("collider"));
        assert_eq!(near("bodyType"), Some("body_type"));
        assert_eq!(near("rigid_body"), Some("rigidbody"));
        assert_eq!(near("health"), None);
        assert_eq!(near("light"), None, "never suggests the name itself");
        assert_eq!(
            did_you_mean("trasnform", names).as_deref(),
            Some("did you mean `transform`?")
        );
        // A tie goes to the first by name, whatever the order given
        assert_eq!(closest("bat", ["cat", "bar"]), Some("bar"));
        assert_eq!(closest("bat", ["bar", "cat"]), Some("bar"));
    }

    #[test]
    fn edit_distance_counts_a_swap_once() {
        assert_eq!(edit_distance("light", "light"), 0);
        assert_eq!(edit_distance("lihgt", "light"), 1);
        assert_eq!(edit_distance("", "abc"), 3);
        assert_eq!(edit_distance("kitten", "sitting"), 3);
    }
}
