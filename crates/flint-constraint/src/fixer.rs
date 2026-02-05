//! Constraint auto-fix engine with cascade detection

use crate::evaluator::ConstraintEvaluator;
use crate::registry::ConstraintRegistry;
use crate::types::{AutoFixStrategy, ConstraintDef, ConstraintKind};
use flint_core::Result;
use flint_ecs::FlintWorld;
use flint_schema::SchemaRegistry;
use std::collections::HashSet;

const MAX_ITERATIONS: usize = 10;

/// A single fix action that was or would be applied
#[derive(Debug, Clone)]
pub struct FixAction {
    pub constraint_name: String,
    pub entity_name: String,
    pub description: String,
    pub strategy: String,
}

/// Report of fix operations
#[derive(Debug)]
pub struct FixReport {
    pub actions: Vec<FixAction>,
    pub remaining_violations: usize,
    pub cycle_detected: bool,
    pub iterations: usize,
}

/// Applies auto-fixes for constraint violations
pub struct ConstraintFixer<'a> {
    schema_registry: &'a SchemaRegistry,
    constraint_registry: &'a ConstraintRegistry,
}

impl<'a> ConstraintFixer<'a> {
    /// Create a new fixer
    pub fn new(
        schema_registry: &'a SchemaRegistry,
        constraint_registry: &'a ConstraintRegistry,
    ) -> Self {
        Self {
            schema_registry,
            constraint_registry,
        }
    }

    /// Apply fixes to the world, iterating until stable or max iterations
    pub fn fix(&self, world: &mut FlintWorld) -> Result<FixReport> {
        let mut all_actions = Vec::new();
        let mut seen_pairs: HashSet<(String, String)> = HashSet::new();
        let mut cycle_detected = false;

        for iteration in 0..MAX_ITERATIONS {
            let evaluator =
                ConstraintEvaluator::new(world, self.schema_registry, self.constraint_registry);
            let report = evaluator.validate();

            if report.violations.is_empty() {
                return Ok(FixReport {
                    actions: all_actions,
                    remaining_violations: 0,
                    cycle_detected: false,
                    iterations: iteration + 1,
                });
            }

            let fixable_violations: Vec<_> = report
                .violations
                .iter()
                .filter(|v| v.has_auto_fix)
                .collect();

            if fixable_violations.is_empty() {
                return Ok(FixReport {
                    actions: all_actions,
                    remaining_violations: report.violations.len(),
                    cycle_detected: false,
                    iterations: iteration + 1,
                });
            }

            let mut made_progress = false;

            for violation in &fixable_violations {
                let pair = (
                    violation.constraint_name.clone(),
                    violation.entity_name.clone(),
                );

                if seen_pairs.contains(&pair) {
                    cycle_detected = true;
                    continue;
                }
                seen_pairs.insert(pair);

                // Find the constraint definition
                if let Some(constraint) = self
                    .constraint_registry
                    .all()
                    .iter()
                    .find(|c| c.name == violation.constraint_name)
                {
                    if let Some(ref auto_fix) = constraint.auto_fix {
                        if auto_fix.enabled {
                            if let Some(action) =
                                self.apply_fix(world, constraint, &violation.entity_name, &auto_fix.strategy)?
                            {
                                all_actions.push(action);
                                made_progress = true;
                            }
                        }
                    }
                }
            }

            if cycle_detected || !made_progress {
                let evaluator =
                    ConstraintEvaluator::new(world, self.schema_registry, self.constraint_registry);
                let final_report = evaluator.validate();
                return Ok(FixReport {
                    actions: all_actions,
                    remaining_violations: final_report.violations.len(),
                    cycle_detected,
                    iterations: iteration + 1,
                });
            }
        }

        // Max iterations reached
        let evaluator =
            ConstraintEvaluator::new(world, self.schema_registry, self.constraint_registry);
        let final_report = evaluator.validate();
        Ok(FixReport {
            actions: all_actions,
            remaining_violations: final_report.violations.len(),
            cycle_detected,
            iterations: MAX_ITERATIONS,
        })
    }

    /// Dry run: clone the world, apply fixes, return report without mutation
    pub fn dry_run(&self, world: &FlintWorld) -> Result<FixReport> {
        let mut cloned = clone_world(world);
        self.fix(&mut cloned)
    }

    fn apply_fix(
        &self,
        world: &mut FlintWorld,
        constraint: &ConstraintDef,
        entity_name: &str,
        strategy: &AutoFixStrategy,
    ) -> Result<Option<FixAction>> {
        match strategy {
            AutoFixStrategy::AddChild {
                archetype,
                defaults,
            } => {
                let parent_id = match world.get_id(entity_name) {
                    Some(id) => id,
                    None => return Ok(None),
                };

                let child_name = format!("{}_{}", entity_name, archetype);
                // Avoid duplicate names
                if world.contains_name(&child_name) {
                    return Ok(None);
                }

                let child_id = world.spawn(&child_name)?;
                let components = world.get_components_mut(child_id).unwrap();
                components.archetype = Some(archetype.clone());

                // Apply archetype defaults from schema
                if let Some(arch_schema) = self.schema_registry.get_archetype(archetype) {
                    for (comp_name, arch_defaults) in &arch_schema.defaults {
                        components.set(comp_name.clone(), arch_defaults.clone());
                    }
                }

                // Apply any override defaults from the fix strategy
                for (comp_name, value) in defaults {
                    components.set(comp_name.clone(), value.clone());
                }

                world.set_parent(child_id, parent_id)?;

                Ok(Some(FixAction {
                    constraint_name: constraint.name.clone(),
                    entity_name: entity_name.to_string(),
                    description: format!("Added child '{}' with archetype '{}'", child_name, archetype),
                    strategy: "add_child".to_string(),
                }))
            }

            AutoFixStrategy::SetDefault { field, value } => {
                let id = match world.get_id(entity_name) {
                    Some(id) => id,
                    None => return Ok(None),
                };

                let parts: Vec<&str> = field.split('.').collect();
                if parts.len() < 2 {
                    return Ok(None);
                }

                let components = match world.get_components_mut(id) {
                    Some(c) => c,
                    None => return Ok(None),
                };

                components.set_field(parts[0], parts[1], value.clone());

                Ok(Some(FixAction {
                    constraint_name: constraint.name.clone(),
                    entity_name: entity_name.to_string(),
                    description: format!("Set {} = {}", field, value),
                    strategy: "set_default".to_string(),
                }))
            }

            AutoFixStrategy::RemoveInvalid => {
                let id = match world.get_id(entity_name) {
                    Some(id) => id,
                    None => return Ok(None),
                };

                // For value range violations, extract the field from the constraint kind
                if let ConstraintKind::ValueRange { field, .. } = &constraint.kind {
                    let parts: Vec<&str> = field.split('.').collect();
                    if parts.len() >= 2 {
                        if let Some(components) = world.get_components_mut(id) {
                            if let Some(comp_data) = components.get_mut(parts[0]) {
                                if let Some(table) = comp_data.as_table_mut() {
                                    table.remove(parts[1]);
                                }
                            }
                        }
                    }
                }

                Ok(Some(FixAction {
                    constraint_name: constraint.name.clone(),
                    entity_name: entity_name.to_string(),
                    description: "Removed invalid field".to_string(),
                    strategy: "remove_invalid".to_string(),
                }))
            }

            AutoFixStrategy::AssignFromParent {
                field,
                source_field,
            } => {
                let id = match world.get_id(entity_name) {
                    Some(id) => id,
                    None => return Ok(None),
                };

                let parent_id = match world.get_parent(id) {
                    Some(pid) => pid,
                    None => return Ok(None),
                };

                // Read the source field from parent
                let source_parts: Vec<&str> = source_field.split('.').collect();
                let value = if source_parts.len() >= 2 {
                    world
                        .get_components(parent_id)
                        .and_then(|c| c.get_field(source_parts[0], source_parts[1]))
                        .cloned()
                } else {
                    None
                };

                if let Some(value) = value {
                    let target_parts: Vec<&str> = field.split('.').collect();
                    if target_parts.len() >= 2 {
                        if let Some(components) = world.get_components_mut(id) {
                            components.set_field(target_parts[0], target_parts[1], value);
                        }
                    }

                    Ok(Some(FixAction {
                        constraint_name: constraint.name.clone(),
                        entity_name: entity_name.to_string(),
                        description: format!("Assigned {} from parent's {}", field, source_field),
                        strategy: "assign_from_parent".to_string(),
                    }))
                } else {
                    Ok(None)
                }
            }
        }
    }
}

/// Clone a FlintWorld by re-creating all entities and their data
fn clone_world(source: &FlintWorld) -> FlintWorld {
    let mut dest = FlintWorld::new();

    // First pass: create entities
    for info in source.all_entities() {
        dest.spawn(info.name.clone()).unwrap();
    }

    // Second pass: copy components and set parents
    for info in source.all_entities() {
        let dest_id = dest.get_id(&info.name).unwrap();

        if let Some(components) = source.get_components(info.id) {
            let dest_components = dest.get_components_mut(dest_id).unwrap();
            dest_components.archetype = components.archetype.clone();
            for (name, value) in &components.data {
                dest_components.set(name.clone(), value.clone());
            }
        }

        if let Some(parent_name) = &info.parent {
            if let Some(parent_id) = dest.get_id(parent_name) {
                dest.set_parent(dest_id, parent_id).unwrap();
            }
        }
    }

    dest
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{AutoFix, Severity};

    fn setup_world() -> FlintWorld {
        let mut world = FlintWorld::new();

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

        world
    }

    fn constraints_with_add_child_fix() -> ConstraintRegistry {
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
            auto_fix: Some(AutoFix {
                enabled: true,
                strategy: AutoFixStrategy::AddChild {
                    archetype: "handle".to_string(),
                    defaults: std::collections::HashMap::new(),
                },
            }),
        });
        registry
    }

    #[test]
    fn test_add_child_fix() {
        let mut world = setup_world();
        let schemas = SchemaRegistry::new();
        let constraints = constraints_with_add_child_fix();

        let fixer = ConstraintFixer::new(&schemas, &constraints);
        let report = fixer.fix(&mut world).unwrap();

        assert_eq!(report.actions.len(), 1);
        assert!(report.actions[0].description.contains("handle"));
        assert!(world.contains_name("front_door_handle"));
        assert_eq!(report.remaining_violations, 0);
    }

    #[test]
    fn test_set_default_fix() {
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
            auto_fix: Some(AutoFix {
                enabled: true,
                strategy: AutoFixStrategy::SetDefault {
                    field: "door.open_angle".to_string(),
                    value: toml::Value::Float(90.0),
                },
            }),
        });

        let fixer = ConstraintFixer::new(&schemas, &constraints);
        let report = fixer.fix(&mut world).unwrap();

        assert_eq!(report.actions.len(), 1);
        assert_eq!(report.remaining_violations, 0);

        // Verify the value was fixed
        let components = world.get_components(id).unwrap();
        let angle = components.get_field("door", "open_angle").unwrap();
        assert_eq!(angle.as_float(), Some(90.0));
    }

    #[test]
    fn test_remove_invalid_fix() {
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
            auto_fix: Some(AutoFix {
                enabled: true,
                strategy: AutoFixStrategy::RemoveInvalid,
            }),
        });

        let fixer = ConstraintFixer::new(&schemas, &constraints);
        let report = fixer.fix(&mut world).unwrap();

        assert_eq!(report.actions.len(), 1);
        let components = world.get_components(id).unwrap();
        assert!(components.get_field("door", "open_angle").is_none());
    }

    #[test]
    fn test_dry_run_immutability() {
        let world = setup_world();
        let schemas = SchemaRegistry::new();
        let constraints = constraints_with_add_child_fix();

        let entity_count_before = world.entity_count();

        let fixer = ConstraintFixer::new(&schemas, &constraints);
        let report = fixer.dry_run(&world).unwrap();

        assert_eq!(report.actions.len(), 1);
        // World should not have been mutated
        assert_eq!(world.entity_count(), entity_count_before);
        assert!(!world.contains_name("front_door_handle"));
    }

    #[test]
    fn test_no_fixable_violations() {
        let mut world = setup_world();
        let schemas = SchemaRegistry::new();
        let mut constraints = ConstraintRegistry::new();
        constraints.register(ConstraintDef {
            name: "doors_have_handles".to_string(),
            description: None,
            query: "entities where archetype == 'door'".to_string(),
            kind: ConstraintKind::RequiredChild {
                child_archetype: "handle".to_string(),
            },
            severity: Severity::Warning,
            message: "Door '{name}' is missing a handle".to_string(),
            auto_fix: None, // No auto-fix
        });

        let fixer = ConstraintFixer::new(&schemas, &constraints);
        let report = fixer.fix(&mut world).unwrap();

        assert_eq!(report.actions.len(), 0);
        assert_eq!(report.remaining_violations, 1);
    }

    #[test]
    fn test_fix_report_structure() {
        let mut world = setup_world();
        let schemas = SchemaRegistry::new();
        let constraints = constraints_with_add_child_fix();

        let fixer = ConstraintFixer::new(&schemas, &constraints);
        let report = fixer.fix(&mut world).unwrap();

        assert!(!report.cycle_detected);
        assert!(report.iterations >= 1);
        assert_eq!(report.actions[0].strategy, "add_child");
    }
}
