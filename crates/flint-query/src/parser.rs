//! Query language parser

use pest::Parser;
use pest_derive::Parser;
use thiserror::Error;

#[derive(Parser)]
#[grammar = "grammar.pest"]
struct QueryParser;

#[derive(Debug, Error)]
pub enum QueryError {
    #[error("Parse error: {0}")]
    ParseError(String),
    #[error("Invalid operator: {0}")]
    InvalidOperator(String),
    #[error("Invalid value: {0}")]
    InvalidValue(String),
}

/// A parsed query
#[derive(Debug, Clone)]
pub struct Query {
    pub resource: String,
    pub condition: Option<Condition>,
}

/// A query condition
#[derive(Debug, Clone)]
pub struct Condition {
    pub field: String,
    pub operator: Operator,
    pub value: QueryValue,
}

/// Comparison operators
#[derive(Debug, Clone, PartialEq)]
pub enum Operator {
    Equal,
    NotEqual,
    Contains,
    GreaterThan,
    LessThan,
    GreaterThanOrEqual,
    LessThanOrEqual,
}

/// Query values
#[derive(Debug, Clone)]
pub enum QueryValue {
    String(String),
    Number(f64),
    Boolean(bool),
}

impl QueryValue {
    pub fn as_str(&self) -> Option<&str> {
        match self {
            QueryValue::String(s) => Some(s),
            _ => None,
        }
    }

    pub fn as_f64(&self) -> Option<f64> {
        match self {
            QueryValue::Number(n) => Some(*n),
            _ => None,
        }
    }

    pub fn as_bool(&self) -> Option<bool> {
        match self {
            QueryValue::Boolean(b) => Some(*b),
            _ => None,
        }
    }
}

/// Parse a query string
pub fn parse_query(input: &str) -> Result<Query, QueryError> {
    let pairs = QueryParser::parse(Rule::query, input)
        .map_err(|e| QueryError::ParseError(e.to_string()))?;

    let mut resource = String::new();
    let mut condition = None;

    for pair in pairs {
        for inner in pair.into_inner() {
            match inner.as_rule() {
                Rule::resource => {
                    resource = inner.as_str().to_string();
                }
                Rule::where_clause => {
                    for clause_inner in inner.into_inner() {
                        if clause_inner.as_rule() == Rule::condition {
                            condition = Some(parse_condition(clause_inner)?);
                        }
                    }
                }
                _ => {}
            }
        }
    }

    Ok(Query { resource, condition })
}

fn parse_condition(pair: pest::iterators::Pair<Rule>) -> Result<Condition, QueryError> {
    let mut field = String::new();
    let mut operator = Operator::Equal;
    let mut value = QueryValue::String(String::new());

    for inner in pair.into_inner() {
        match inner.as_rule() {
            Rule::field => {
                field = inner.as_str().to_string();
            }
            Rule::operator => {
                operator = parse_operator(inner.as_str())?;
            }
            Rule::value => {
                value = parse_value(inner)?;
            }
            _ => {}
        }
    }

    Ok(Condition {
        field,
        operator,
        value,
    })
}

fn parse_operator(op: &str) -> Result<Operator, QueryError> {
    match op {
        "==" => Ok(Operator::Equal),
        "!=" => Ok(Operator::NotEqual),
        "contains" => Ok(Operator::Contains),
        ">" => Ok(Operator::GreaterThan),
        "<" => Ok(Operator::LessThan),
        ">=" => Ok(Operator::GreaterThanOrEqual),
        "<=" => Ok(Operator::LessThanOrEqual),
        _ => Err(QueryError::InvalidOperator(op.to_string())),
    }
}

fn parse_value(pair: pest::iterators::Pair<Rule>) -> Result<QueryValue, QueryError> {
    for inner in pair.into_inner() {
        match inner.as_rule() {
            Rule::string => {
                // Extract the inner string content
                for string_inner in inner.into_inner() {
                    match string_inner.as_rule() {
                        Rule::string_inner | Rule::string_inner_dq => {
                            return Ok(QueryValue::String(string_inner.as_str().to_string()));
                        }
                        _ => {}
                    }
                }
            }
            Rule::number => {
                let n: f64 = inner
                    .as_str()
                    .parse()
                    .map_err(|_| QueryError::InvalidValue(inner.as_str().to_string()))?;
                return Ok(QueryValue::Number(n));
            }
            Rule::boolean => {
                let b = inner.as_str() == "true";
                return Ok(QueryValue::Boolean(b));
            }
            _ => {}
        }
    }

    Err(QueryError::InvalidValue("empty value".to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_simple_query() {
        let query = parse_query("entities").unwrap();
        assert_eq!(query.resource, "entities");
        assert!(query.condition.is_none());
    }

    #[test]
    fn test_query_with_condition() {
        let query = parse_query("entities where archetype == 'door'").unwrap();
        assert_eq!(query.resource, "entities");

        let cond = query.condition.unwrap();
        assert_eq!(cond.field, "archetype");
        assert_eq!(cond.operator, Operator::Equal);
        assert_eq!(cond.value.as_str(), Some("door"));
    }

    #[test]
    fn test_nested_field() {
        let query = parse_query("entities where door.locked == true").unwrap();

        let cond = query.condition.unwrap();
        assert_eq!(cond.field, "door.locked");
        assert_eq!(cond.value.as_bool(), Some(true));
    }

    #[test]
    fn test_numeric_comparison() {
        let query = parse_query("entities where health > 50").unwrap();

        let cond = query.condition.unwrap();
        assert_eq!(cond.operator, Operator::GreaterThan);
        assert_eq!(cond.value.as_f64(), Some(50.0));
    }
}
