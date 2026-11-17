//! What a query answers, as data: the same for a scene file and a running
//! game (#713).
//!
//! `flint query` and `flint entity show` answer from a scene they load; the
//! player answers the same questions about its live world over
//! `--inspect-listen`, and writes the same entities to `--dump-world`. All
//! of them come here, so the grammar, the entities a query picks and the
//! JSON each is written as are one thing whichever world is asked.

use crate::executor::execute_query;
use crate::output::QueryResult;
use crate::parser::{parse_query, Query};
use flint_ecs::{EntityInfo, FlintWorld};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;

/// One entity and its components' values: what `flint entity show` prints,
/// and each entity of a world dump.
#[derive(Debug, Clone, Serialize)]
pub struct EntityDetails {
    #[serde(flatten)]
    pub info: EntityInfo,
    /// Every component's fields as the world holds them now, by component
    /// name. `None` for an entity with no component table.
    pub data: Option<BTreeMap<String, toml::Value>>,
}

/// The entity named `name`, with its components' values.
pub fn entity_details(world: &FlintWorld, name: &str) -> Option<EntityDetails> {
    let id = world.get_id(name)?;
    let info = world.all_entities().into_iter().find(|e| e.id == id)?;
    Some(details_of(world, info))
}

fn details_of(world: &FlintWorld, info: EntityInfo) -> EntityDetails {
    let data = world.get_components(info.id).map(|c| c.data.clone());
    EntityDetails { info, data }
}

/// The entities `query` picks, each with its components' values, in the
/// world's order (by name, so the same every run). A `components` query
/// picks none.
pub fn details_matching(world: &FlintWorld, query: &Query) -> Vec<EntityDetails> {
    match execute_query(world, query) {
        QueryResult::Entities(entities) => entities
            .into_iter()
            .map(|info| details_of(world, info))
            .collect(),
        QueryResult::Components(_) => Vec::new(),
    }
}

/// The world at one moment of a run: a `--dump-world` snapshot.
#[derive(Debug, Clone, Serialize)]
pub struct Snapshot {
    /// The game time asked for (`--dump-world-at`).
    pub at: f64,
    /// The game time of the frame it was taken on, the first at or after
    /// `at` (the clock scripts read as `total_time()`).
    pub time: f64,
    /// Frames since the run started, this one counted from 1.
    pub frame: u64,
    /// The entities the dump's query picked, with their values.
    pub entities: Vec<EntityDetails>,
}

impl Snapshot {
    pub fn take(world: &FlintWorld, query: &Query, at: f64, time: f64, frame: u64) -> Self {
        Self {
            at,
            time,
            frame,
            entities: details_matching(world, query),
        }
    }
}

/// A question for a world, as one line of the inspect socket takes it:
/// a JSON object, or a bare query string.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Ask {
    /// `{"query": "entities where door.locked == true"}`: what `flint
    /// query` prints; with `"data": true`, each entity with its components'
    /// values, as a world dump writes them.
    Query {
        query: String,
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        data: bool,
    },
    /// `{"entity": "player"}`: what `flint entity show player` prints.
    Entity { entity: String },
}

impl Ask {
    /// One line of the socket: a JSON object of either form, or the text of
    /// a query (`entities where archetype == 'door'`), so a line typed into
    /// `nc` works as well as one a program sends.
    pub fn parse(line: &str) -> Result<Ask, String> {
        let line = line.trim();
        if line.starts_with('{') {
            return serde_json::from_str(line).map_err(|e| {
                format!(r#"expected {{"query": "..."}} or {{"entity": "..."}}: {e}"#)
            });
        }
        if line.is_empty() {
            return Err("an empty line: send a query, or {\"entity\": \"name\"}".into());
        }
        Ok(Ask::Query {
            query: line.to_owned(),
            data: false,
        })
    }
}

/// The answer to `ask` from `world`, as the JSON `flint query` or `flint
/// entity show` prints for it, or why there is none (a query that doesn't
/// parse, an entity that isn't there).
pub fn answer(world: &FlintWorld, ask: &Ask) -> Result<Value, String> {
    match ask {
        Ask::Query { query, data } => {
            let query = parse_query(query).map_err(|e| format!("Failed to parse query: {e}"))?;
            if *data {
                return to_value(&details_matching(world, &query));
            }
            match execute_query(world, &query) {
                QueryResult::Entities(entities) => to_value(&entities),
                QueryResult::Components(components) => to_value(&components),
            }
        }
        Ask::Entity { entity } => {
            let details = entity_details(world, entity)
                .ok_or_else(|| format!("Entity '{entity}' not found"))?;
            to_value(&details)
        }
    }
}

fn to_value(value: &impl Serialize) -> Result<Value, String> {
    serde_json::to_value(value).map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn world() -> FlintWorld {
        let mut world = FlintWorld::new();
        for (name, locked, x) in [("door_b", false, 2.0), ("door_a", true, 1.0)] {
            let id = world.spawn(name).unwrap();
            let mut c = world.get_components_mut(id).unwrap();
            c.set_archetype(Some("door".into()));
            c.set("door", toml::toml! { locked = locked }.into());
            c.set("transform", toml::toml! { position = [x, 0.0, 0.0] }.into());
        }
        world.spawn("empty").unwrap();
        world
    }

    #[test]
    fn a_line_is_json_or_a_bare_query() {
        assert_eq!(
            Ask::parse("entities where archetype == 'door'").unwrap(),
            Ask::Query {
                query: "entities where archetype == 'door'".into(),
                data: false
            }
        );
        assert_eq!(
            Ask::parse(r#" {"query": "entities", "data": true} "#).unwrap(),
            Ask::Query {
                query: "entities".into(),
                data: true
            }
        );
        assert_eq!(
            Ask::parse(r#"{"entity": "door_a"}"#).unwrap(),
            Ask::Entity {
                entity: "door_a".into()
            }
        );
        assert!(Ask::parse("").is_err());
        assert!(Ask::parse(r#"{"what": 1}"#).is_err());
    }

    #[test]
    fn a_query_answers_what_flint_query_prints() {
        let world = world();
        let ask = Ask::parse("entities where door.locked == true").unwrap();
        let got = answer(&world, &ask).unwrap();
        let names: Vec<&str> = got
            .as_array()
            .unwrap()
            .iter()
            .map(|e| e["name"].as_str().unwrap())
            .collect();
        assert_eq!(names, ["door_a"]);
        assert!(got[0].get("data").is_none(), "names, not values");
        let components = answer(&world, &Ask::parse("components").unwrap()).unwrap();
        assert_eq!(components, serde_json::json!(["door", "transform"]));
    }

    #[test]
    fn with_data_each_entity_carries_its_values_in_name_order() {
        let world = world();
        let ask = Ask::Query {
            query: "entities where archetype == 'door'".into(),
            data: true,
        };
        let got = answer(&world, &ask).unwrap();
        assert_eq!(
            got[0]["name"], "door_a",
            "by name, whatever the spawn order"
        );
        assert_eq!(got[0]["data"]["door"]["locked"], true);
        assert_eq!(got[1]["data"]["transform"]["position"][0], 2.0);
    }

    #[test]
    fn an_entity_answers_what_flint_entity_show_prints() {
        let world = world();
        let got = answer(
            &world,
            &Ask::Entity {
                entity: "door_b".into(),
            },
        )
        .unwrap();
        assert_eq!(got["name"], "door_b");
        assert_eq!(got["archetype"], "door");
        assert_eq!(got["data"]["door"]["locked"], false);
        let missing = answer(
            &world,
            &Ask::Entity {
                entity: "nope".into(),
            },
        );
        assert_eq!(missing.unwrap_err(), "Entity 'nope' not found");
    }

    #[test]
    fn a_bad_query_says_why() {
        let err = answer(&FlintWorld::new(), &Ask::parse("entities where").unwrap()).unwrap_err();
        assert!(err.starts_with("Failed to parse query"), "{err}");
    }
}
