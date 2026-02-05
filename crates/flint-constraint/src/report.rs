//! Validation report types

use crate::types::Severity;
use flint_core::EntityId;

/// A single constraint violation
#[derive(Debug, Clone)]
pub struct Violation {
    pub constraint_name: String,
    pub entity_name: String,
    pub entity_id: EntityId,
    pub severity: Severity,
    pub message: String,
    pub has_auto_fix: bool,
}

/// A complete validation report
#[derive(Debug, Default)]
pub struct ValidationReport {
    pub violations: Vec<Violation>,
}

impl ValidationReport {
    /// Create an empty report
    pub fn new() -> Self {
        Self::default()
    }

    /// Check if the scene is valid (no errors)
    pub fn is_valid(&self) -> bool {
        !self
            .violations
            .iter()
            .any(|v| v.severity == Severity::Error)
    }

    /// Count violations by severity
    pub fn error_count(&self) -> usize {
        self.violations
            .iter()
            .filter(|v| v.severity == Severity::Error)
            .count()
    }

    pub fn warning_count(&self) -> usize {
        self.violations
            .iter()
            .filter(|v| v.severity == Severity::Warning)
            .count()
    }

    pub fn info_count(&self) -> usize {
        self.violations
            .iter()
            .filter(|v| v.severity == Severity::Info)
            .count()
    }

    /// Get a human-readable summary
    pub fn summary(&self) -> String {
        let total = self.violations.len();
        if total == 0 {
            return "No violations found.".to_string();
        }

        format!(
            "{} violation(s): {} error(s), {} warning(s), {} info",
            total,
            self.error_count(),
            self.warning_count(),
            self.info_count(),
        )
    }
}
