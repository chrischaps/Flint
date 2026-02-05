//! Query result formatting

use flint_ecs::EntityInfo;
use serde::Serialize;

/// Query result types
#[derive(Debug, Clone)]
pub enum QueryResult {
    Entities(Vec<EntityInfo>),
    Components(Vec<String>),
}

impl QueryResult {
    pub fn is_empty(&self) -> bool {
        match self {
            QueryResult::Entities(e) => e.is_empty(),
            QueryResult::Components(c) => c.is_empty(),
        }
    }

    pub fn len(&self) -> usize {
        match self {
            QueryResult::Entities(e) => e.len(),
            QueryResult::Components(c) => c.len(),
        }
    }
}

/// Format query result as JSON
pub fn format_json(result: &QueryResult) -> String {
    match result {
        QueryResult::Entities(entities) => {
            serde_json::to_string_pretty(entities).unwrap_or_else(|_| "[]".to_string())
        }
        QueryResult::Components(components) => {
            serde_json::to_string_pretty(components).unwrap_or_else(|_| "[]".to_string())
        }
    }
}

/// Format query result as TOML
pub fn format_toml(result: &QueryResult) -> String {
    match result {
        QueryResult::Entities(entities) => {
            let wrapper = EntityListWrapper { entities: entities.clone() };
            toml::to_string_pretty(&wrapper).unwrap_or_else(|_| "".to_string())
        }
        QueryResult::Components(components) => {
            let wrapper = ComponentListWrapper { components: components.clone() };
            toml::to_string_pretty(&wrapper).unwrap_or_else(|_| "".to_string())
        }
    }
}

#[derive(Serialize)]
struct EntityListWrapper {
    entities: Vec<EntityInfo>,
}

#[derive(Serialize)]
struct ComponentListWrapper {
    components: Vec<String>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use flint_core::EntityId;

    #[test]
    fn test_format_json() {
        let result = QueryResult::Entities(vec![EntityInfo::new(
            EntityId::from_raw(1),
            "test",
        )]);

        let json = format_json(&result);
        assert!(json.contains("test"));
    }

    #[test]
    fn test_format_components() {
        let result = QueryResult::Components(vec!["transform".to_string(), "door".to_string()]);

        let json = format_json(&result);
        assert!(json.contains("transform"));
        assert!(json.contains("door"));
    }
}
