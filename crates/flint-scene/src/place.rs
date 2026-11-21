//! Where in a scene's text a scene-table diagnostic stands (#1061).
//!
//! The scene pass checks the tables after the file has parsed, as values,
//! so what it finds has a table and a field but no place in the file. Here
//! the text is parsed once more, with spans (`toml_edit`), and each such
//! diagnostic is given the line and column of the key it names: the field's
//! key when the file writes it, else the nearest key above it that the file
//! writes (a missing field is at its table's header). Only when there is a
//! diagnostic to place: a clean scene pays nothing.

use flint_core::diagnostic::Diagnostic;
use flint_core::toml_util::line_column;
use std::ops::Range;
use toml_edit::{ImDocument, Item, Table, Value};

/// Give each diagnostic of `found` that is about a scene-level table (a
/// `table`, no entity) and has no line yet the line and column, in `text`,
/// of what it names. Text that doesn't parse, or a table the text doesn't
/// write, leaves a diagnostic as it was.
pub fn place_table_diagnostics(text: &str, found: &mut [Diagnostic]) {
    let unplaced = |d: &Diagnostic| d.table.is_some() && d.entity.is_none() && d.line.is_none();
    if !found.iter().any(unplaced) {
        return;
    }
    let Ok(doc) = ImDocument::parse(text) else {
        return;
    };
    for d in found.iter_mut().filter(|d| unplaced(d)) {
        let table = d.table.as_deref().unwrap_or_default();
        if let Some(span) = span_of(doc.as_table(), table, d.field.as_deref()) {
            let (line, column) = line_column(text, span.start);
            d.line = Some(line);
            d.column = Some(column);
        }
    }
}

/// One step down a diagnostic's dotted field: a key, or an array index.
#[derive(Debug, PartialEq)]
enum Step<'a> {
    Key(&'a str),
    Index(usize),
}

/// `zone[1].radius` -> `zone`, `1`, `radius`.
fn steps(field: &str) -> Vec<Step<'_>> {
    let mut out = Vec::new();
    for part in field.split('.') {
        let (key, mut rest) = part.split_once('[').map_or((part, ""), |(k, r)| (k, r));
        if !key.is_empty() {
            out.push(Step::Key(key));
        }
        while let Some((index, after)) = rest.split_once(']') {
            match index.parse() {
                Ok(i) => out.push(Step::Index(i)),
                Err(_) => return out,
            }
            rest = after.strip_prefix('[').unwrap_or(after);
        }
    }
    out
}

/// What a step lands on: a table's item, or a value inside one.
#[derive(Clone, Copy)]
enum Node<'a> {
    Item(&'a Item),
    Value(&'a Value),
    Table(&'a Table),
}

/// The span of the deepest key of `table` / `field` the document writes.
fn span_of(root: &Table, table: &str, field: Option<&str>) -> Option<Range<usize>> {
    let (key, item) = root.get_key_value(table)?;
    let mut best = key.span().or_else(|| item.span());
    let mut node = Node::Item(item);
    for step in field.map(steps).unwrap_or_default() {
        let next = match (node, step) {
            (Node::Item(item), Step::Key(k)) => match item {
                Item::Table(t) => t.get_key_value(k).map(|(k, i)| (k.span(), Node::Item(i))),
                Item::Value(v) => key_in_value(v, k),
                _ => None,
            },
            (Node::Table(t), Step::Key(k)) => {
                t.get_key_value(k).map(|(k, i)| (k.span(), Node::Item(i)))
            }
            (Node::Value(v), Step::Key(k)) => key_in_value(v, k),
            (Node::Item(item), Step::Index(i)) => match item {
                Item::ArrayOfTables(a) => a.get(i).map(|t| (t.span(), Node::Table(t))),
                Item::Value(v) => index_in_value(v, i),
                _ => None,
            },
            (Node::Value(v), Step::Index(i)) => index_in_value(v, i),
            (Node::Table(_), Step::Index(_)) => None,
        };
        let Some((span, landed)) = next else {
            break;
        };
        best = span.or(best);
        node = landed;
    }
    best
}

fn key_in_value<'a>(value: &'a Value, key: &str) -> Option<(Option<Range<usize>>, Node<'a>)> {
    let (k, item) = value.as_inline_table()?.get_key_value(key)?;
    Some((k.span().or_else(|| item.span()), Node::Item(item)))
}

fn index_in_value(value: &Value, index: usize) -> Option<(Option<Range<usize>>, Node<'_>)> {
    let v = value.as_array()?.get(index)?;
    Some((v.span(), Node::Value(v)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use flint_core::diagnostic::codes;

    const TEXT: &str = "[scene]\nname = \"t\"\n\n[living]\nshadowz = \"drift\"\n\n\
                        [camera]\nposition = [0, 1, \"two\"]\nlook = { at = 3 }\n\n\
                        [[wooze.zone]]\nradius = 1\n\n[[wooze.zone]]\n  radius = \"far\"\n";

    fn placed(table: &str, field: Option<&str>) -> (Option<usize>, Option<usize>) {
        let mut d = Diagnostic::warning(codes::UNKNOWN_FIELD, "x").table(table);
        if let Some(field) = field {
            d = d.field(field);
        }
        let mut found = [d];
        place_table_diagnostics(TEXT, &mut found);
        (found[0].line, found[0].column)
    }

    #[test]
    fn a_field_is_at_its_key() {
        assert_eq!(placed("living", Some("shadowz")), (Some(5), Some(1)));
        assert_eq!(placed("wooze", Some("zone[1].radius")), (Some(15), Some(3)));
        assert_eq!(placed("camera", Some("look.at")), (Some(9), Some(10)));
    }

    #[test]
    fn an_array_element_is_at_its_value() {
        assert_eq!(placed("camera", Some("position[2]")), (Some(8), Some(19)));
    }

    #[test]
    fn what_the_file_lacks_is_at_the_nearest_key_it_writes() {
        // A missing field is at its table's header
        assert_eq!(placed("living", Some("stagger")), (Some(4), Some(2)));
        assert_eq!(placed("camera", None), (Some(7), Some(2)));
        assert_eq!(placed("weather", Some("rain")), (None, None));
    }

    #[test]
    fn an_entity_or_a_placed_diagnostic_is_left_alone() {
        let mut found = [
            Diagnostic::warning(codes::UNKNOWN_FIELD, "x")
                .table("living")
                .at(1, 1),
            Diagnostic::warning(codes::UNKNOWN_FIELD, "x").entity("crate"),
        ];
        place_table_diagnostics(TEXT, &mut found);
        assert_eq!((found[0].line, found[0].column), (Some(1), Some(1)));
        assert_eq!(found[1].line, None);
    }

    #[test]
    fn steps_read_dotted_fields_and_indices() {
        assert_eq!(
            steps("zone[1].radius"),
            [Step::Key("zone"), Step::Index(1), Step::Key("radius")]
        );
        assert_eq!(
            steps("m[0][2]"),
            [Step::Key("m"), Step::Index(0), Step::Index(2)]
        );
    }
}
