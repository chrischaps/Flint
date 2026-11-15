//! Constraint registry for loading and managing constraint definitions

use crate::types::{ConstraintDef, ConstraintFile, ConstraintKind, Severity};
use flint_core::vfs;
use flint_core::{FlintError, Result};
use flint_query::parse_query;
use std::fmt;
use std::path::{Path, PathBuf};

/// Registry that holds all loaded constraint definitions
#[derive(Debug, Default)]
pub struct ConstraintRegistry {
    constraints: Vec<ConstraintDef>,
    /// The file each constraint was read from, by index; `None` for one
    /// registered in code.
    sources: Vec<Option<PathBuf>>,
}

/// A constraint that can't be evaluated (#687): its query, or its
/// `query_rule`'s rule, doesn't parse, or selects something other than
/// entities. It used to be skipped in silence, so it passed forever.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BadConstraint {
    /// The constraint's `name`.
    pub name: String,
    /// The file it was read from, when it came from one.
    pub file: Option<PathBuf>,
    /// `query`, or `rule` (a `query_rule`'s): the key of the text at fault.
    pub field: &'static str,
    /// The text that doesn't parse.
    pub query: String,
    /// What the parser said, or what is wrong with the selection.
    pub error: String,
}

impl fmt::Display for BadConstraint {
    /// ``constraint 'doors' (schemas/constraints/a.toml): its query `...`
    /// can't be evaluated: ...``
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "constraint '{}'", self.name)?;
        if let Some(file) = &self.file {
            write!(f, " ({})", file.to_string_lossy().replace('\\', "/"))?;
        }
        write!(
            f,
            ": its {} `{}` can't be evaluated: {}",
            self.field, self.query, self.error
        )
    }
}

/// What is wrong with `text` as a constraint's query, if anything: it must
/// parse (the whole of it) and select entities.
fn query_problem(text: &str) -> Option<String> {
    match parse_query(text) {
        Err(e) => Some(e.to_string()),
        Ok(query) if query.resource != "entities" => Some(format!(
            "a constraint selects entities, and this selects {}",
            query.resource
        )),
        Ok(_) => None,
    }
}

impl ConstraintDef {
    /// Why this constraint can't be evaluated, if it can't: `(key, text,
    /// error)` for its `query`, then for a `query_rule`'s `rule`.
    pub fn problems(&self) -> Vec<(&'static str, String, String)> {
        let mut found = Vec::new();
        if let Some(error) = query_problem(&self.query) {
            found.push(("query", self.query.clone(), error));
        }
        if let ConstraintKind::QueryRule { rule } = &self.kind {
            if let Some(error) = query_problem(rule) {
                found.push(("rule", rule.clone(), error));
            }
        }
        found
    }
}

impl ConstraintRegistry {
    /// Create a new empty registry
    pub fn new() -> Self {
        Self::default()
    }

    /// Load constraints from a directory of TOML files
    ///
    /// Expects `path/constraints/*.toml` files, read in name order. A file
    /// that doesn't read is the error, and the error names the file.
    pub fn load_from_directory<P: AsRef<Path>>(path: P) -> Result<Self> {
        let mut registry = Self::new();
        let constraints_path = path.as_ref().join("constraints");

        if vfs::exists(&constraints_path) {
            let mut files = vfs::read_dir(&constraints_path)?;
            files.sort();
            for file_path in files {
                if file_path.extension().map(|e| e == "toml").unwrap_or(false) {
                    registry.load_file(&file_path)?;
                }
            }
        }

        Ok(registry)
    }

    /// Load constraints from a TOML file. The error names the file.
    pub fn load_file<P: AsRef<Path>>(&mut self, path: P) -> Result<()> {
        let path = path.as_ref();
        let shown = path.to_string_lossy().replace('\\', "/");
        let content = vfs::read_to_string(path)
            .map_err(|e| FlintError::ConstraintLoadError(format!("{shown}: {e}")))?;
        let before = self.constraints.len();
        self.load_string(&content).map_err(|e| match e {
            FlintError::ConstraintLoadError(message) => {
                FlintError::ConstraintLoadError(format!("{shown}: {message}"))
            }
            other => other,
        })?;
        for source in &mut self.sources[before..] {
            *source = Some(path.to_path_buf());
        }
        Ok(())
    }

    /// Load constraints from a TOML string
    pub fn load_string(&mut self, content: &str) -> Result<()> {
        let file: ConstraintFile = toml::from_str(content).map_err(|e| {
            FlintError::ConstraintLoadError(format!("Failed to parse constraint TOML: {}", e))
        })?;

        for constraint in file.constraint {
            self.register(constraint);
        }

        Ok(())
    }

    /// Register a constraint directly
    pub fn register(&mut self, constraint: ConstraintDef) {
        self.constraints.push(constraint);
        self.sources.push(None);
    }

    /// Add every constraint of `other`, keeping the file each came from.
    pub fn extend(&mut self, other: ConstraintRegistry) {
        self.constraints.extend(other.constraints);
        self.sources.extend(other.sources);
    }

    /// The file the constraint at `index` of [`all`](Self::all) was read
    /// from, when it came from one.
    pub fn source(&self, index: usize) -> Option<&Path> {
        self.sources.get(index)?.as_deref()
    }

    /// Every constraint that can't be evaluated, with its file: what
    /// `flint check` and `flint validate` report, and what the evaluator
    /// turns into an error violation.
    pub fn problems(&self) -> Vec<BadConstraint> {
        let mut found = Vec::new();
        for (index, constraint) in self.constraints.iter().enumerate() {
            for (field, query, error) in constraint.problems() {
                found.push(BadConstraint {
                    name: constraint.name.clone(),
                    file: self.source(index).map(Path::to_path_buf),
                    field,
                    query,
                    error,
                });
            }
        }
        found
    }

    /// Get all constraints
    pub fn all(&self) -> &[ConstraintDef] {
        &self.constraints
    }

    /// Get constraints filtered by severity
    pub fn by_severity(&self, severity: Severity) -> Vec<&ConstraintDef> {
        self.constraints
            .iter()
            .filter(|c| c.severity == severity)
            .collect()
    }

    /// Get constraints that apply to a specific archetype
    ///
    /// This checks the query string for archetype references — a simple heuristic
    /// that works for the common pattern `"entities where archetype == '<name>'"`.
    pub fn for_archetype(&self, archetype: &str) -> Vec<&ConstraintDef> {
        self.constraints
            .iter()
            .filter(|c| c.query.contains(archetype))
            .collect()
    }

    /// Get the number of loaded constraints
    pub fn len(&self) -> usize {
        self.constraints.len()
    }

    /// Check if the registry is empty
    pub fn is_empty(&self) -> bool {
        self.constraints.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// #687: a constraint whose query doesn't parse is named, with its file,
    /// rather than skipped; so is one whose rule doesn't, and one that
    /// selects something other than entities.
    #[test]
    fn a_bad_query_is_a_problem_naming_the_constraint_and_its_file() {
        use flint_core::vfs::MemorySource;
        let rule = |name: &str, query: &str, kind: &str| {
            format!(
                "[[constraint]]\nname = \"{name}\"\nquery = \"{query}\"\nseverity = \"error\"\n\
                 message = \"m\"\n[constraint.kind]\n{kind}\n\n"
            )
        };
        let required = "type = \"required_component\"\narchetype = \"door\"\ncomponent = \"t\"";
        let rules = [
            rule("fine", "entities where archetype == 'door'", required),
            rule("typo", "entities were archetype == 'door'", required),
            rule(
                "two_conditions",
                "entities where archetype == 'door' and door.locked == true",
                "type = \"query_rule\"\nrule = \"components\"",
            ),
        ]
        .concat();
        let mut source = MemorySource::new();
        source.insert("game/schemas/constraints/rules.toml", rules.as_str());
        source.insert("game/broken/constraints/broken.toml", "[[constraint]\n");
        vfs::scoped(std::sync::Arc::new(source), || {
            let registry = ConstraintRegistry::load_from_directory("game/schemas").unwrap();
            let problems = registry.problems();
            let named: Vec<(&str, &str)> = problems
                .iter()
                .map(|p| (p.name.as_str(), p.field))
                .collect();
            assert_eq!(
                named,
                [
                    ("typo", "query"),
                    ("two_conditions", "query"),
                    ("two_conditions", "rule"),
                ]
            );
            let said = problems[0].to_string();
            assert!(
                said.starts_with(
                    "constraint 'typo' (game/schemas/constraints/rules.toml): its query \
                     `entities were archetype == 'door'` can't be evaluated: Parse error"
                ),
                "{said}"
            );
            assert!(problems[2].error.contains("selects components"));

            // Merged into another registry, a constraint keeps its file
            let mut merged = ConstraintRegistry::new();
            merged.extend(registry);
            assert_eq!(merged.problems().len(), 3);
            assert!(merged.source(0).is_some());

            // A file that is not TOML is an error naming the file
            let err = ConstraintRegistry::load_from_directory("game/broken")
                .unwrap_err()
                .to_string();
            assert!(err.contains("game/broken/constraints/broken.toml"), "{err}");
        });
    }

    fn sample_toml() -> &'static str {
        r#"
[[constraint]]
name = "doors_have_handles"
query = "entities where archetype == 'door'"
severity = "warning"
message = "Door '{name}' is missing a handle"

[constraint.kind]
type = "required_child"
child_archetype = "handle"

[[constraint]]
name = "rooms_have_bounds"
query = "entities where archetype == 'room'"
severity = "error"
message = "Room '{name}' is missing bounds"

[constraint.kind]
type = "required_component"
archetype = "room"
component = "bounds"
"#
    }

    #[test]
    fn test_load_from_string() {
        let mut registry = ConstraintRegistry::new();
        registry.load_string(sample_toml()).unwrap();
        assert_eq!(registry.len(), 2);
    }

    #[test]
    fn test_all_constraints() {
        let mut registry = ConstraintRegistry::new();
        registry.load_string(sample_toml()).unwrap();
        assert_eq!(registry.all().len(), 2);
    }

    #[test]
    fn test_by_severity() {
        let mut registry = ConstraintRegistry::new();
        registry.load_string(sample_toml()).unwrap();

        let warnings = registry.by_severity(Severity::Warning);
        assert_eq!(warnings.len(), 1);
        assert_eq!(warnings[0].name, "doors_have_handles");

        let errors = registry.by_severity(Severity::Error);
        assert_eq!(errors.len(), 1);
        assert_eq!(errors[0].name, "rooms_have_bounds");
    }

    #[test]
    fn test_for_archetype() {
        let mut registry = ConstraintRegistry::new();
        registry.load_string(sample_toml()).unwrap();

        let door_constraints = registry.for_archetype("door");
        assert_eq!(door_constraints.len(), 1);
        assert_eq!(door_constraints[0].name, "doors_have_handles");

        let room_constraints = registry.for_archetype("room");
        assert_eq!(room_constraints.len(), 1);
        assert_eq!(room_constraints[0].name, "rooms_have_bounds");
    }

    #[test]
    fn test_register_directly() {
        let mut registry = ConstraintRegistry::new();
        registry.register(ConstraintDef {
            name: "test_constraint".to_string(),
            description: None,
            query: "entities".to_string(),
            kind: ConstraintKind::RequiredChild {
                child_archetype: "child".to_string(),
            },
            severity: Severity::Info,
            message: "Test message".to_string(),
            auto_fix: None,
        });
        assert_eq!(registry.len(), 1);
    }

    #[test]
    fn test_empty_registry() {
        let registry = ConstraintRegistry::new();
        assert!(registry.is_empty());
        assert_eq!(registry.len(), 0);
    }
}
