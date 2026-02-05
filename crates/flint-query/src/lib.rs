//! Flint Query - Query language for filtering entities
//!
//! This crate provides a simple query language for filtering and
//! selecting entities based on their components and properties.

mod executor;
mod output;
mod parser;

pub use executor::execute_query;
pub use output::{format_json, format_toml, QueryResult};
pub use parser::{parse_query, Condition, Operator, Query, QueryError};
