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

/// Whether one entity meets a condition, as a query's `where` asks of each
/// (#714: `flint playtest`'s `expect <entity> <condition>`).
pub fn entity_matches(world: &FlintWorld, entity: &EntityInfo, condition: &Condition) -> bool {
    matches_condition(entity, condition, world)
}

/// The value an entity's field holds, as a condition reads it (a string, a
/// number or a boolean), or `None` when it has none: what a failed
/// `expect` reports it found.
pub fn field_value(world: &FlintWorld, entity: &EntityInfo, field: &str) -> Option<QueryValue> {
    get_field_value(entity, field, world).map(|v| match v {
        FieldValue::String(s) => QueryValue::String(s),
        FieldValue::Number(n) => QueryValue::Number(n),
        FieldValue::Bool(b) => QueryValue::Boolean(b),
    })
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

    // Handle nested component fields (e.g., "door.locked",
    // "transform.position[2]")
    let steps = steps(field)?;
    if let [Step::Key(component_name), field_path @ ..] = steps.as_slice() {
        if !field_path.is_empty() {
            let comp_data = world.get_components(entity.id)?.get(component_name)?;
            return extract_toml_value(comp_data, field_path);
        }
    }

    // Check if field matches a component name (for existence check)
    if entity.components.contains(&field.to_string()) {
        return Some(FieldValue::Bool(true));
    }

    None
}

/// One step of a field path: `.name` or `[n]`.
#[derive(Debug, Clone, Copy, PartialEq)]
enum Step<'a> {
    Key(&'a str),
    Index(usize),
}

/// The names a vector's first four elements answer to (#714).
const AXES: [&str; 4] = ["x", "y", "z", "w"];

/// A field's steps, `path.points[3].x` as `path`, `points`, `[3]`, `x`; `None`
/// for a spelling the grammar would refuse (a field handed to
/// [`field_value`] unparsed).
fn steps(field: &str) -> Option<Vec<Step<'_>>> {
    let mut out = Vec::new();
    for part in field.split('.') {
        let (key, mut rest) = part.split_at(part.find('[').unwrap_or(part.len()));
        if key.is_empty() {
            return None; // `[0].x`, `a.` or `a.[0]`
        }
        out.push(Step::Key(key));
        while let Some(r) = rest.strip_prefix('[') {
            let (digits, after) = r.split_once(']')?;
            if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) {
                return None;
            }
            out.push(Step::Index(digits.parse().ok()?));
            rest = after;
        }
        if !rest.is_empty() {
            return None;
        }
    }
    Some(out)
}

/// The value at `path` under `value`. A table steps by key; an array by
/// index or by axis (`.z` is `[2]`). A step that finds nothing, past an
/// array's end, into a scalar, `[n]` of a table, is no value: no match, as
/// a missing field is (#1185).
fn extract_toml_value(value: &toml::Value, path: &[Step]) -> Option<FieldValue> {
    let Some((step, rest)) = path.split_first() else {
        return toml_to_field_value(value);
    };
    let next = match (value, *step) {
        (toml::Value::Table(table), Step::Key(key)) => table.get(key),
        (toml::Value::Array(items), Step::Key(axis)) => {
            items.get(AXES.iter().position(|a| *a == axis)?)
        }
        (toml::Value::Array(items), Step::Index(i)) => items.get(i),
        _ => None,
    }?;
    extract_toml_value(next, rest)
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

/// Whether `found` stands in `op` to `want`, as a condition compares a
/// field's value: strings by `==`, `!=` and `contains`, numbers by all six
/// comparisons, booleans by `==` and `!=`; two kinds never match.
pub fn compare(found: &QueryValue, op: &Operator, want: &QueryValue) -> bool {
    let field = match found {
        QueryValue::String(s) => FieldValue::String(s.clone()),
        QueryValue::Number(n) => FieldValue::Number(*n),
        QueryValue::Boolean(b) => FieldValue::Bool(*b),
    };
    compare_values(&field, op, want)
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
        let mut components = world.get_components_mut(id1).unwrap();
        components.set_archetype(Some("door".to_string()));
        components.set(
            "door",
            toml::toml! {
                locked = true
                style = "hinged"
            }
            .into(),
        );

        let id2 = world.spawn("door2").unwrap();
        let mut components = world.get_components_mut(id2).unwrap();
        components.set_archetype(Some("door".to_string()));
        components.set(
            "door",
            toml::toml! {
                locked = false
                style = "sliding"
            }
            .into(),
        );

        let id3 = world.spawn("room1").unwrap();
        let mut components = world.get_components_mut(id3).unwrap();
        components.set_archetype(Some("room".to_string()));

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

    /// #714: one condition asked of one entity, a vector's axis read by
    /// name, and the value found for a report
    #[test]
    fn one_entity_one_condition_and_a_vector_axis() {
        let mut world = setup_test_world();
        let id = world.get_id("door1").unwrap();
        world.get_components_mut(id).unwrap().set(
            "transform",
            toml::toml! { position = [1.0, 2.5, -3.0] }.into(),
        );
        let door = world
            .all_entities()
            .into_iter()
            .find(|e| e.name == "door1")
            .unwrap();
        let cond = |s: &str| crate::parse_condition(s).unwrap();
        assert!(entity_matches(&world, &door, &cond("door.locked == true")));
        assert!(!entity_matches(
            &world,
            &door,
            &cond("door.locked == false")
        ));
        assert!(entity_matches(
            &world,
            &door,
            &cond("transform.position.y > 2")
        ));
        assert!(entity_matches(
            &world,
            &door,
            &cond("transform.position.z < -2.5")
        ));
        assert!(!entity_matches(
            &world,
            &door,
            &cond("transform.position.w == 0")
        ));
        assert_eq!(
            field_value(&world, &door, "transform.position.y").map(|v| v.to_string()),
            Some("2.5".into())
        );
        assert_eq!(
            cond("door.style == 'hinged'").to_string(),
            "door.style == 'hinged'"
        );
        assert!(crate::parse_condition("door.locked == true and x == 1").is_err());
    }

    /// #1185: a field steps into a vector by axis or by `[n]`, into a list
    /// of vectors and back into a table; a step that finds nothing is no
    /// match, never an error.
    #[test]
    fn a_field_reaches_into_arrays_and_a_miss_is_no_match() {
        let mut world = setup_test_world();
        let id = world.get_id("door1").unwrap();
        {
            let mut c = world.get_components_mut(id).unwrap();
            c.set("transform", toml::toml! { position = [1, 2.5, 7.0] }.into());
            c.set(
                "path",
                toml::toml! {
                    points = [[0.0, 0.0, 0.0], [4.0, 1.0, -6.0]]
                    stops = [{ name = "gate", wait = 2.0 }, { name = "well", wait = 0.5 }]
                }
                .into(),
            );
        }
        let door = world
            .all_entities()
            .into_iter()
            .find(|e| e.name == "door1")
            .unwrap();
        let holds = |s: &str| entity_matches(&world, &door, &crate::parse_condition(s).unwrap());
        let found = |s: &str| field_value(&world, &door, s).map(|v| v.to_string());

        // The issue's own query, both spellings; an integer element too.
        assert!(holds("transform.position.z > 5"));
        assert!(holds("transform.position[2] > 5"));
        assert!(holds("transform.position[0] == 1"));
        assert_eq!(
            found("transform.position[1]"),
            found("transform.position.y")
        );
        // A list of vectors, and a list of tables.
        assert!(holds("path.points[1].z < -5"));
        assert!(holds("path.points[1][0] == 4"));
        assert!(holds("path.stops[1].name == 'well'"));
        assert!(holds("path.stops[0].wait >= 2"));

        for miss in [
            "transform.position[3] == 0",  // past the end
            "transform.position.w == 0",   // past the end, by axis
            "transform.position[99] == 0", // far past it
            "transform.position.q == 0",   // not an axis
            "door.locked.x == true",       // into a boolean
            "door.locked[0] == true",      // into a boolean, by index
            "door.style[0] == 'h'",        // a string is not indexed
            "path.stops[0][0] == 'gate'",  // `[n]` of a table
            "path.points == 0",            // a whole array is no value
            "missing.thing[0] == 0",       // no such component
        ] {
            assert!(!holds(miss), "{miss} matched");
            assert!(!holds(&miss.replace("==", "!=")), "{miss} as != matched");
        }
        // `.x` is an axis wherever the value is an array, whatever it holds.
        assert!(holds("path.stops.x.name == 'gate'"));

        // Through a whole query too: one door is past z = 5.
        let q = parse_query("entities where transform.position.z > 5").unwrap();
        let QueryResult::Entities(picked) = execute_query(&world, &q) else {
            panic!("entities");
        };
        assert_eq!(
            picked.iter().map(|e| e.name.as_str()).collect::<Vec<_>>(),
            ["door1"]
        );
    }

    #[test]
    fn a_field_spelling_the_grammar_refuses_has_no_steps() {
        assert_eq!(
            steps("path.points[3].x"),
            Some(vec![
                Step::Key("path"),
                Step::Key("points"),
                Step::Index(3),
                Step::Key("x")
            ])
        );
        for bad in [
            "[0].x", "a.", "a.[0]", "a[", "a[x]", "a[]", "a[1]b", "a[-1]",
        ] {
            assert_eq!(steps(bad), None, "{bad}");
        }
    }

    #[test]
    fn test_query_by_archetype() {
        let world = setup_test_world();
        let query = parse_query("entities where archetype == 'door'").unwrap();
        let result = execute_query(&world, &query);

        if let QueryResult::Entities(entities) = result {
            assert_eq!(entities.len(), 2);
            assert!(entities
                .iter()
                .all(|e| e.archetype.as_deref() == Some("door")));
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
