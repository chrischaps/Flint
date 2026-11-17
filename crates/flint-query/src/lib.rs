//! Flint Query - Query language for filtering entities
//!
//! This crate provides a simple query language for filtering and
//! selecting entities based on their components and properties.

mod answer;
mod executor;
mod output;
mod parser;

pub use answer::{answer, details_matching, entity_details, Ask, EntityDetails, Snapshot};
pub use executor::{compare, entity_matches, execute_query, field_value};
pub use output::{format_json, format_toml, QueryResult};
pub use parser::{
    parse_condition, parse_query, Condition, Operator, Query, QueryError, QueryValue,
};
