//! Constraint evaluation engine

use crate::registry::ConstraintRegistry;
use crate::report::{ValidationReport, Violation};
use crate::types::{ConstraintDef, ConstraintKind};
use flint_ecs::FlintWorld;
use flint_query::{execute_query, parse_query, QueryResult};
use flint_schema::SchemaRegistry;

/// Evaluates constraints against a world
#[allow(dead_code)]
pub struct ConstraintEvaluator<'a> {
    world: &'a FlintWorld,
    schema_registry: &'a SchemaRegistry,
    constraint_registry: &'a ConstraintRegistry,
}

impl<'a> ConstraintEvaluator<'a> {
    /// Create a new evaluator
    pub fn new(
        world: &'a FlintWorld,
        schema_registry: &'a SchemaRegistry,
        constraint_registry: &'a ConstraintRegistry,
    ) -> Self {
        Self {
            world,
            schema_registry,
            constraint_registry,
        }
    }

    /// Run all constraints and return a validation report
    pub fn validate(&self) -> ValidationReport {
        let mut report = ValidationReport::new();

        for constraint in self.constraint_registry.all() {
            self.evaluate_constraint(constraint, &mut report);
        }

        report
    }

    fn evaluate_constraint(&self, constraint: &ConstraintDef, report: &mut ValidationReport) {
        // Parse and execute the query to get matching entities
        let query = match parse_query(&constraint.query) {
            Ok(q) => q,
            Err(_) => return, // Skip malformed queries silently
        };

        let result = execute_query(self.world, &query);
        let entities = match result {
            QueryResult::Entities(entities) => entities,
            _ => return, // Only entity queries make sense for constraints
        };

        // Check each matched entity against the constraint kind
        for entity in &entities {
            let violated = match &constraint.kind {
                ConstraintKind::RequiredComponent {
                    archetype,
                    component,
                } => self.check_required_component(&entity.name, archetype, component),

                ConstraintKind::RequiredChild { child_archetype } => {
                    self.check_required_child(&entity.name, child_archetype)
                }

                ConstraintKind::ValueRange { field, min, max } => {
                    self.check_value_range(&entity.name, field, *min, *max)
                }

                ConstraintKind::ReferenceValid { field } => {
                    self.check_reference_valid(&entity.name, field)
                }

                ConstraintKind::QueryRule { rule } => self.check_query_rule(rule),
            };

            if violated {
                let message = expand_message(
                    &constraint.message,
                    &entity.name,
                    entity.archetype.as_deref().unwrap_or(""),
                );

                report.violations.push(Violation {
                    constraint_name: constraint.name.clone(),
                    entity_name: entity.name.clone(),
                    entity_id: entity.id,
                    severity: constraint.severity,
                    message,
                    has_auto_fix: constraint
                        .auto_fix
                        .as_ref()
                        .map(|f| f.enabled)
                        .unwrap_or(false),
                });
            }
        }
    }

    fn check_required_component(
        &self,
        entity_name: &str,
        archetype: &str,
        component: &str,
    ) -> bool {
        let id = match self.world.get_id(entity_name) {
            Some(id) => id,
            None => return false,
        };

        let components = match self.world.get_components(id) {
            Some(c) => c,
            None => return true, // No components at all = violation
        };

        // Check if entity is the right archetype
        if let Some(ref entity_arch) = components.archetype {
            if entity_arch != archetype {
                return false; // Different archetype, not a violation
            }
        }

        // Component is missing = violation
        !components.has(component)
    }

    fn check_required_child(&self, entity_name: &str, child_archetype: &str) -> bool {
        let id = match self.world.get_id(entity_name) {
            Some(id) => id,
            None => return false,
        };

        let children = self.world.get_children(id);

        // Check if any child has the required archetype
        let has_required_child = children.iter().any(|child_id| {
            self.world
                .get_components(*child_id)
                .and_then(|c| c.archetype.as_deref())
                .map(|arch| arch == child_archetype)
                .unwrap_or(false)
        });

        !has_required_child
    }

    fn check_value_range(&self, entity_name: &str, field: &str, min: f64, max: f64) -> bool {
        let id = match self.world.get_id(entity_name) {
            Some(id) => id,
            None => return false,
        };

        let components = match self.world.get_components(id) {
            Some(c) => c,
            None => return false,
        };

        // Parse field path (e.g., "door.open_angle")
        let parts: Vec<&str> = field.split('.').collect();
        if parts.len() < 2 {
            return false;
        }

        let value = components.get_field(parts[0], parts[1]);
        match value {
            Some(v) => {
                let num = v
                    .as_float()
                    .or_else(|| v.as_integer().map(|i| i as f64));
                match num {
                    Some(n) => n < min || n > max,
                    None => false, // Non-numeric field, not a range violation
                }
            }
            None => false, // Field doesn't exist, not a range violation
        }
    }

    fn check_reference_valid(&self, entity_name: &str, field: &str) -> bool {
        let id = match self.world.get_id(entity_name) {
            Some(id) => id,
            None => return false,
        };

        let components = match self.world.get_components(id) {
            Some(c) => c,
            None => return false,
        };

        let parts: Vec<&str> = field.split('.').collect();
        if parts.len() < 2 {
            return false;
        }

        let value = components.get_field(parts[0], parts[1]);
        match value {
            Some(v) => {
                if let Some(ref_name) = v.as_str() {
                    // Check if the referenced entity exists
                    !self.world.contains_name(ref_name)
                } else {
                    false
                }
            }
            None => false,
        }
    }

    fn check_query_rule(&self, rule: &str) -> bool {
        // A query rule passes if it returns no results
        // Violation = the rule query returns entities (meaning something is wrong)
        match parse_query(rule) {
            Ok(q) => {
                let result = execute_query(self.world, &q);
                match result {
                    QueryResult::Entities(entities) => entities.is_empty(),
                    _ => false,
                }
            }
            Err(_) => false,
        }
    }
}

/// Expand message template with entity info
fn expand_message(template: &str, entity_name: &str, archetype: &str) -> String {
    template
        .replace("{name}", entity_name)
        .replace("{archetype}", archetype)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{ConstraintKind, Severity};

    fn setup_world() -> FlintWorld {
        let mut world = FlintWorld::new();

        // Create a door entity without a child handle
        let door_id = world.spawn("front_door").unwrap();
        let components = world.get_components_mut(door_id).unwrap();
        components.archetype = Some("door".to_string());
        components.set(
            "door",
            toml::Value::Table({
                let mut m = toml::map::Map::new();
                m.insert("style".to_string(), toml::Value::String("hinged".to_string()));
                m.insert("locked".to_string(), toml::Value::Boolean(false));
                m.insert("open_angle".to_string(), toml::Value::Float(90.0));
                m
            }),
        );
        components.set(
            "transform",
            toml::Value::Table({
                let mut m = toml::map::Map::new();
                m.insert(
                    "position".to_string(),
                    toml::Value::Array(vec![
                        toml::Value::Integer(5),
                        toml::Value::Integer(0),
                        toml::Value::Integer(0),
                    ]),
                );
                m
            }),
        );

        // Create a room with bounds
        let room_id = world.spawn("main_room").unwrap();
        let components = world.get_components_mut(room_id).unwrap();
        components.archetype = Some("room".to_string());
        components.set(
            "bounds",
            toml::Value::Table({
                let mut m = toml::map::Map::new();
                m.insert(
                    "min".to_string(),
                    toml::Value::Array(vec![
                        toml::Value::Integer(0),
                        toml::Value::Integer(0),
                        toml::Value::Integer(0),
                    ]),
                );
                m.insert(
                    "max".to_string(),
                    toml::Value::Array(vec![
                        toml::Value::Integer(10),
                        toml::Value::Integer(4),
                        toml::Value::Integer(10),
                    ]),
                );
                m
            }),
        );

        world.set_parent(door_id, room_id).unwrap();

        world
    }

    fn constraint_registry_with_required_child() -> ConstraintRegistry {
        let mut registry = ConstraintRegistry::new();
        registry.register(ConstraintDef {
            name: "doors_have_handles".to_string(),
            description: None,
            query: "entities where archetype == 'door'".to_string(),
            kind: ConstraintKind::RequiredChild {
                child_archetype: "handle".to_string(),
            },
            severity: Severity::Warning,
            message: "Door '{name}' is missing a handle".to_string(),
            auto_fix: None,
        });
        registry
    }

    #[test]
    fn test_required_child_violation() {
        let world = setup_world();
        let schemas = SchemaRegistry::new();
        let constraints = constraint_registry_with_required_child();

        let evaluator = ConstraintEvaluator::new(&world, &schemas, &constraints);
        let report = evaluator.validate();

        assert_eq!(report.violations.len(), 1);
        assert_eq!(report.violations[0].entity_name, "front_door");
        assert_eq!(report.violations[0].severity, Severity::Warning);
        assert!(report.violations[0].message.contains("front_door"));
    }

    #[test]
    fn test_required_child_passes() {
        let mut world = setup_world();
        let schemas = SchemaRegistry::new();
        let constraints = constraint_registry_with_required_child();

        // Add a handle child to the door
        let door_id = world.get_id("front_door").unwrap();
        let handle_id = world.spawn("door_handle").unwrap();
        let components = world.get_components_mut(handle_id).unwrap();
        components.archetype = Some("handle".to_string());
        world.set_parent(handle_id, door_id).unwrap();

        let evaluator = ConstraintEvaluator::new(&world, &schemas, &constraints);
        let report = evaluator.validate();

        assert_eq!(report.violations.len(), 0);
    }

    #[test]
    fn test_required_component_violation() {
        let mut world = FlintWorld::new();
        let id = world.spawn("bare_door").unwrap();
        let components = world.get_components_mut(id).unwrap();
        components.archetype = Some("door".to_string());
        // No transform component!

        let schemas = SchemaRegistry::new();
        let mut constraints = ConstraintRegistry::new();
        constraints.register(ConstraintDef {
            name: "doors_need_transform".to_string(),
            description: None,
            query: "entities where archetype == 'door'".to_string(),
            kind: ConstraintKind::RequiredComponent {
                archetype: "door".to_string(),
                component: "transform".to_string(),
            },
            severity: Severity::Error,
            message: "Door '{name}' is missing transform".to_string(),
            auto_fix: None,
        });

        let evaluator = ConstraintEvaluator::new(&world, &schemas, &constraints);
        let report = evaluator.validate();

        assert_eq!(report.error_count(), 1);
        assert!(!report.is_valid());
    }

    #[test]
    fn test_required_component_passes() {
        let world = setup_world(); // front_door has transform
        let schemas = SchemaRegistry::new();
        let mut constraints = ConstraintRegistry::new();
        constraints.register(ConstraintDef {
            name: "doors_need_transform".to_string(),
            description: None,
            query: "entities where archetype == 'door'".to_string(),
            kind: ConstraintKind::RequiredComponent {
                archetype: "door".to_string(),
                component: "transform".to_string(),
            },
            severity: Severity::Error,
            message: "Door '{name}' is missing transform".to_string(),
            auto_fix: None,
        });

        let evaluator = ConstraintEvaluator::new(&world, &schemas, &constraints);
        let report = evaluator.validate();

        assert_eq!(report.error_count(), 0);
        assert!(report.is_valid());
    }

    #[test]
    fn test_value_range_violation() {
        let mut world = FlintWorld::new();
        let id = world.spawn("bad_door").unwrap();
        let components = world.get_components_mut(id).unwrap();
        components.archetype = Some("door".to_string());
        components.set(
            "door",
            toml::Value::Table({
                let mut m = toml::map::Map::new();
                m.insert("open_angle".to_string(), toml::Value::Float(200.0));
                m
            }),
        );

        let schemas = SchemaRegistry::new();
        let mut constraints = ConstraintRegistry::new();
        constraints.register(ConstraintDef {
            name: "door_angle_valid".to_string(),
            description: None,
            query: "entities where archetype == 'door'".to_string(),
            kind: ConstraintKind::ValueRange {
                field: "door.open_angle".to_string(),
                min: 0.0,
                max: 180.0,
            },
            severity: Severity::Error,
            message: "Door '{name}' has invalid angle".to_string(),
            auto_fix: None,
        });

        let evaluator = ConstraintEvaluator::new(&world, &schemas, &constraints);
        let report = evaluator.validate();

        assert_eq!(report.error_count(), 1);
    }

    #[test]
    fn test_value_range_passes() {
        let world = setup_world(); // front_door has open_angle = 90.0
        let schemas = SchemaRegistry::new();
        let mut constraints = ConstraintRegistry::new();
        constraints.register(ConstraintDef {
            name: "door_angle_valid".to_string(),
            description: None,
            query: "entities where archetype == 'door'".to_string(),
            kind: ConstraintKind::ValueRange {
                field: "door.open_angle".to_string(),
                min: 0.0,
                max: 180.0,
            },
            severity: Severity::Error,
            message: "Door '{name}' has invalid angle".to_string(),
            auto_fix: None,
        });

        let evaluator = ConstraintEvaluator::new(&world, &schemas, &constraints);
        let report = evaluator.validate();

        assert_eq!(report.error_count(), 0);
    }

    #[test]
    fn test_reference_valid_violation() {
        let mut world = FlintWorld::new();
        let id = world.spawn("linked_entity").unwrap();
        let components = world.get_components_mut(id).unwrap();
        components.archetype = Some("linked".to_string());
        components.set(
            "link",
            toml::Value::Table({
                let mut m = toml::map::Map::new();
                m.insert(
                    "target".to_string(),
                    toml::Value::String("nonexistent_entity".to_string()),
                );
                m
            }),
        );

        let schemas = SchemaRegistry::new();
        let mut constraints = ConstraintRegistry::new();
        constraints.register(ConstraintDef {
            name: "valid_references".to_string(),
            description: None,
            query: "entities where archetype == 'linked'".to_string(),
            kind: ConstraintKind::ReferenceValid {
                field: "link.target".to_string(),
            },
            severity: Severity::Error,
            message: "'{name}' references nonexistent entity".to_string(),
            auto_fix: None,
        });

        let evaluator = ConstraintEvaluator::new(&world, &schemas, &constraints);
        let report = evaluator.validate();

        assert_eq!(report.error_count(), 1);
    }

    #[test]
    fn test_report_summary() {
        let world = setup_world();
        let schemas = SchemaRegistry::new();
        let constraints = constraint_registry_with_required_child();

        let evaluator = ConstraintEvaluator::new(&world, &schemas, &constraints);
        let report = evaluator.validate();

        assert!(report.summary().contains("1 violation"));
    }

    #[test]
    fn test_empty_world() {
        let world = FlintWorld::new();
        let schemas = SchemaRegistry::new();
        let constraints = constraint_registry_with_required_child();

        let evaluator = ConstraintEvaluator::new(&world, &schemas, &constraints);
        let report = evaluator.validate();

        assert_eq!(report.violations.len(), 0);
        assert!(report.is_valid());
    }

    #[test]
    fn test_message_expansion() {
        assert_eq!(
            expand_message("Door '{name}' ({archetype}) failed", "front_door", "door"),
            "Door 'front_door' (door) failed"
        );
    }

    #[test]
    fn test_multiple_constraints() {
        let mut world = FlintWorld::new();
        let id = world.spawn("bare_door").unwrap();
        let components = world.get_components_mut(id).unwrap();
        components.archetype = Some("door".to_string());

        let schemas = SchemaRegistry::new();
        let mut constraints = ConstraintRegistry::new();
        constraints.register(ConstraintDef {
            name: "doors_need_transform".to_string(),
            description: None,
            query: "entities where archetype == 'door'".to_string(),
            kind: ConstraintKind::RequiredComponent {
                archetype: "door".to_string(),
                component: "transform".to_string(),
            },
            severity: Severity::Error,
            message: "Door '{name}' is missing transform".to_string(),
            auto_fix: None,
        });
        constraints.register(ConstraintDef {
            name: "doors_have_handles".to_string(),
            description: None,
            query: "entities where archetype == 'door'".to_string(),
            kind: ConstraintKind::RequiredChild {
                child_archetype: "handle".to_string(),
            },
            severity: Severity::Warning,
            message: "Door '{name}' is missing a handle".to_string(),
            auto_fix: None,
        });

        let evaluator = ConstraintEvaluator::new(&world, &schemas, &constraints);
        let report = evaluator.validate();

        assert_eq!(report.violations.len(), 2);
        assert_eq!(report.error_count(), 1);
        assert_eq!(report.warning_count(), 1);
    }
}
