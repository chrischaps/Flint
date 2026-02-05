//! Query execution against FlintWorld

use crate::output::QueryResult;
use crate::parser::{Condition, Operator, Query, QueryValue};
use flint_ecs::{EntityInfo, FlintWorld};

/// Execute a query against a world
pub fn execute_query(world: &FlintWorld, query: &Query) -> QueryResult {
    match query.resource.as_str() {
        "entities" => {
            let entities = world.all_entities();
            let filtered = filter_entities(entities, &query.condition, world);
            QueryResult::Entities(filtered)
        }
        "components" => {
            // List all unique component names across all entities
            let mut components: Vec<String> = world
                .all_entities()
                .iter()
                .flat_map(|e| e.components.clone())
                .collect();
            components.sort();
            components.dedup();
            QueryResult::Components(components)
        }
        _ => QueryResult::Entities(vec![]),
    }
}

fn filter_entities(
    entities: Vec<EntityInfo>,
    condition: &Option<Condition>,
    world: &FlintWorld,
) -> Vec<EntityInfo> {
    match condition {
        None => entities,
        Some(cond) => entities
            .into_iter()
            .filter(|e| matches_condition(e, cond, world))
            .collect(),
    }
}

fn matches_condition(entity: &EntityInfo, condition: &Condition, world: &FlintWorld) -> bool {
    let field_value = get_field_value(entity, &condition.field, world);

    match &field_value {
        None => false,
        Some(value) => compare_values(value, &condition.operator, &condition.value),
    }
}

fn get_field_value(entity: &EntityInfo, field: &str, world: &FlintWorld) -> Option<FieldValue> {
    // Handle special fields
    match field {
        "archetype" => {
            return entity.archetype.clone().map(FieldValue::String);
        }
        "name" => {
            return Some(FieldValue::String(entity.name.clone()));
        }
        "parent" => {
            return entity.parent.clone().map(FieldValue::String);
        }
        _ => {}
    }

    // Handle nested component fields (e.g., "door.locked")
    let parts: Vec<&str> = field.split('.').collect();
    if parts.len() >= 2 {
        let component_name = parts[0];
        let field_path = &parts[1..];

        if let Some(components) = world.get_components(entity.id) {
            if let Some(comp_data) = components.get(component_name) {
                return extract_toml_value(comp_data, field_path);
            }
        }
    }

    // Check if field matches a component name (for existence check)
    if entity.components.contains(&field.to_string()) {
        return Some(FieldValue::Bool(true));
    }

    None
}

fn extract_toml_value(value: &toml::Value, path: &[&str]) -> Option<FieldValue> {
    if path.is_empty() {
        return toml_to_field_value(value);
    }

    match value {
        toml::Value::Table(table) => {
            table.get(path[0]).and_then(|v| extract_toml_value(v, &path[1..]))
        }
        _ => None,
    }
}

fn toml_to_field_value(value: &toml::Value) -> Option<FieldValue> {
    match value {
        toml::Value::String(s) => Some(FieldValue::String(s.clone())),
        toml::Value::Integer(i) => Some(FieldValue::Number(*i as f64)),
        toml::Value::Float(f) => Some(FieldValue::Number(*f)),
        toml::Value::Boolean(b) => Some(FieldValue::Bool(*b)),
        _ => None,
    }
}

#[derive(Debug, Clone)]
enum FieldValue {
    String(String),
    Number(f64),
    Bool(bool),
}

fn compare_values(field: &FieldValue, op: &Operator, query: &QueryValue) -> bool {
    match (field, query) {
        (FieldValue::String(f), QueryValue::String(q)) => match op {
            Operator::Equal => f == q,
            Operator::NotEqual => f != q,
            Operator::Contains => f.contains(q),
            _ => false,
        },
        (FieldValue::Number(f), QueryValue::Number(q)) => match op {
            Operator::Equal => (f - q).abs() < f64::EPSILON,
            Operator::NotEqual => (f - q).abs() >= f64::EPSILON,
            Operator::GreaterThan => f > q,
            Operator::LessThan => f < q,
            Operator::GreaterThanOrEqual => f >= q,
            Operator::LessThanOrEqual => f <= q,
            Operator::Contains => false,
        },
        (FieldValue::Bool(f), QueryValue::Boolean(q)) => match op {
            Operator::Equal => f == q,
            Operator::NotEqual => f != q,
            _ => false,
        },
        // Type mismatch
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parser::parse_query;

    fn setup_test_world() -> FlintWorld {
        let mut world = FlintWorld::new();

        let id1 = world.spawn("door1").unwrap();
        let components = world.get_components_mut(id1).unwrap();
        components.archetype = Some("door".to_string());
        components.set(
            "door",
            toml::toml! {
                locked = true
                style = "hinged"
            }.into(),
        );

        let id2 = world.spawn("door2").unwrap();
        let components = world.get_components_mut(id2).unwrap();
        components.archetype = Some("door".to_string());
        components.set(
            "door",
            toml::toml! {
                locked = false
                style = "sliding"
            }.into(),
        );

        let id3 = world.spawn("room1").unwrap();
        let components = world.get_components_mut(id3).unwrap();
        components.archetype = Some("room".to_string());

        world
    }

    #[test]
    fn test_query_all_entities() {
        let world = setup_test_world();
        let query = parse_query("entities").unwrap();
        let result = execute_query(&world, &query);

        if let QueryResult::Entities(entities) = result {
            assert_eq!(entities.len(), 3);
        } else {
            panic!("Expected Entities result");
        }
    }

    #[test]
    fn test_query_by_archetype() {
        let world = setup_test_world();
        let query = parse_query("entities where archetype == 'door'").unwrap();
        let result = execute_query(&world, &query);

        if let QueryResult::Entities(entities) = result {
            assert_eq!(entities.len(), 2);
            assert!(entities.iter().all(|e| e.archetype.as_deref() == Some("door")));
        } else {
            panic!("Expected Entities result");
        }
    }

    #[test]
    fn test_query_nested_field() {
        let world = setup_test_world();
        let query = parse_query("entities where door.locked == true").unwrap();
        let result = execute_query(&world, &query);

        if let QueryResult::Entities(entities) = result {
            assert_eq!(entities.len(), 1);
            assert_eq!(entities[0].name, "door1");
        } else {
            panic!("Expected Entities result");
        }
    }
}
