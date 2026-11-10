//! Checking the keys of a fixed-shape TOML file (#282).
//!
//! Most of Flint's small file formats (`.font.toml`, `.atlas.toml`,
//! `fonts.toml`) are read by walking a `toml::Value` and asking for the keys
//! a loader knows. Anything else is never looked at, so a misspelled key, or
//! a key written after the wrong table header, quietly changes nothing and
//! the default wins. That is the worst kind of mistake for an agent, which
//! gets no error and a different result.
//!
//! A loader describes its file once as a [`Shape`] and calls [`check_keys`]
//! on the parsed document. Every key the shape doesn't know comes back as a
//! [`KeyIssue`] naming the key, the table it was found in, and the likeliest
//! fix:
//!
//! - a key some *enclosing* table knows was written after a header, the
//!   classic TOML trap: every key after `[font]` belongs to `[font]`, so a
//!   top-level key has to go above the first header;
//! - a key another table knows is in the wrong table;
//! - otherwise the nearest known key by edit distance, as a typo.
//!
//! A known key holding the wrong kind of thing (a single `[font]` where the
//! format wants `[[font]]`) is reported too, since the loader can only skip it.
//!
//! The checker reports; it never fails a load. Callers log the issues with
//! the file's path (`tracing::warn!("{}: {issue}", path.display())`).

use std::fmt;

/// What a key holds, as far as checking its keys goes.
#[derive(Clone, Copy, Debug)]
pub enum Shape {
    /// Any value; nothing inside it is checked.
    Value,
    /// A table whose keys are exactly these.
    Table(&'static [(&'static str, Shape)]),
    /// A table of free names (an atlas's regions), each holding this shape.
    Map(&'static Shape),
    /// An array of tables (`[[font]]`), each of this shape.
    Tables(&'static Shape),
}

impl Shape {
    fn kind(&self) -> &'static str {
        match self {
            Shape::Value => "a value",
            Shape::Table(_) | Shape::Map(_) => "a table",
            Shape::Tables(_) => "an array of tables",
        }
    }
}

/// One step of a path into a document: a table key, or an entry of an
/// array of tables.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Step {
    Key(String),
    Entry(usize),
}

/// Where a key was found: a path of table names, with the entry number
/// for arrays of tables. Displays as TOML would write its header.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TablePath(Vec<Step>);

impl TablePath {
    /// The top level of the document.
    pub fn is_root(&self) -> bool {
        self.0.is_empty()
    }

    fn names(&self) -> Vec<&str> {
        self.0
            .iter()
            .filter_map(|s| match s {
                Step::Key(k) => Some(k.as_str()),
                Step::Entry(_) => None,
            })
            .collect()
    }

    fn child(&self, step: Step) -> Self {
        let mut steps = self.0.clone();
        steps.push(step);
        Self(steps)
    }
}

impl fmt::Display for TablePath {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.is_root() {
            return write!(f, "the top level");
        }
        let name = self.names().join(".");
        match self.0.last() {
            Some(Step::Entry(i)) => write!(f, "[[{name}]] #{}", i + 1),
            _ => write!(f, "[{name}]"),
        }
    }
}

/// The likeliest fix for an unknown key.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Hint {
    /// Nothing close.
    None,
    /// A known key of the same table within a few edits.
    Typo(&'static str),
    /// A key of this enclosing table, written after a header below it.
    AfterHeader(TablePath),
    /// A key of this other table.
    Belongs(TablePath),
}

/// What is wrong with a key.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum IssueKind {
    /// The table doesn't know the key.
    Unknown {
        hint: Hint,
        /// The keys the table does know, in the shape's order.
        expected: Vec<&'static str>,
    },
    /// A known key holding the wrong kind of thing.
    WrongKind { expected: &'static str },
}

/// A key a loader will not read.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct KeyIssue {
    pub key: String,
    pub table: TablePath,
    pub kind: IssueKind,
}

impl KeyIssue {
    /// The fix a reader is pointed to, if there is one.
    pub fn hint(&self) -> Option<&Hint> {
        match &self.kind {
            IssueKind::Unknown { hint, .. } if *hint != Hint::None => Some(hint),
            _ => None,
        }
    }
}

impl fmt::Display for KeyIssue {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let (key, table) = (&self.key, &self.table);
        match &self.kind {
            IssueKind::WrongKind { expected } => {
                let example = if *expected == "an array of tables" {
                    format!(
                        " (`[[{}]]`)",
                        table.child(Step::Key(key.clone())).names().join(".")
                    )
                } else {
                    String::new()
                };
                write!(
                    f,
                    "`{key}` in {table} should be {expected}{example}; it is ignored"
                )
            }
            IssueKind::Unknown { hint, expected } => {
                write!(f, "unknown key `{key}` in {table}, ignored")?;
                match hint {
                    Hint::Typo(near) => write!(f, "; did you mean `{near}`?")?,
                    Hint::AfterHeader(owner) => {
                        let header = table.names().join(".");
                        let whose = if owner.is_root() {
                            "a top-level key".to_string()
                        } else {
                            format!("a key of {owner}")
                        };
                        write!(
                            f,
                            "; it is {whose}, but TOML puts every key after a `[{header}]` \
                             header into that table, so move it above the header"
                        )?
                    }
                    Hint::Belongs(owner) => write!(f, "; it is a key of {owner}")?,
                    Hint::None => {}
                }
                if !expected.is_empty() {
                    write!(f, " (expected one of: {})", expected.join(", "))?;
                }
                Ok(())
            }
        }
    }
}

/// Check `doc` against `shape` (normally a [`Shape::Table`] for the whole
/// file) and return every key the shape doesn't know, depth first,
/// each table's keys in name order (`toml` keeps no document
/// order).
pub fn check_keys(doc: &toml::Value, shape: &Shape) -> Vec<KeyIssue> {
    let mut owners = Vec::new();
    known_keys(shape, &[], &mut owners);
    let mut issues = Vec::new();
    walk(doc, shape, &TablePath::default(), &owners, &mut issues);
    issues
}

/// Every (table names, key) the shape knows, for spotting a key in the
/// wrong table. Free-name maps contribute nothing.
fn known_keys(
    shape: &Shape,
    at: &[&'static str],
    out: &mut Vec<(Vec<&'static str>, &'static str)>,
) {
    match shape {
        Shape::Table(fields) => {
            for (key, inner) in fields.iter() {
                out.push((at.to_vec(), *key));
                let mut deeper = at.to_vec();
                deeper.push(key);
                known_keys(inner, &deeper, out);
            }
        }
        Shape::Tables(inner) => known_keys(inner, at, out),
        Shape::Map(_) | Shape::Value => {}
    }
}

fn walk(
    value: &toml::Value,
    shape: &Shape,
    at: &TablePath,
    owners: &[(Vec<&'static str>, &'static str)],
    issues: &mut Vec<KeyIssue>,
) {
    match shape {
        Shape::Value => {}
        Shape::Table(fields) => {
            let Some(table) = value.as_table() else {
                return;
            };
            for (key, inner) in table {
                match fields.iter().find(|(k, _)| *k == key.as_str()) {
                    Some((_, field)) => {
                        let child = at.child(Step::Key(key.clone()));
                        if fits(inner, field) {
                            walk(inner, field, &child, owners, issues);
                        } else {
                            issues.push(KeyIssue {
                                key: key.clone(),
                                table: at.clone(),
                                kind: IssueKind::WrongKind {
                                    expected: field.kind(),
                                },
                            });
                        }
                    }
                    None => issues.push(KeyIssue {
                        key: key.clone(),
                        table: at.clone(),
                        kind: IssueKind::Unknown {
                            hint: hint_for(key, at, fields, owners),
                            expected: fields.iter().map(|(k, _)| *k).collect(),
                        },
                    }),
                }
            }
        }
        Shape::Map(inner) => {
            let Some(table) = value.as_table() else {
                return;
            };
            for (key, v) in table {
                walk(v, inner, &at.child(Step::Key(key.clone())), owners, issues);
            }
        }
        Shape::Tables(inner) => {
            let Some(entries) = value.as_array() else {
                return;
            };
            for (i, entry) in entries.iter().enumerate() {
                walk(entry, inner, &at.child(Step::Entry(i)), owners, issues);
            }
        }
    }
}

/// Whether a value is the kind of thing a shape describes.
fn fits(value: &toml::Value, shape: &Shape) -> bool {
    match shape {
        Shape::Value => true,
        Shape::Table(_) | Shape::Map(_) => value.is_table(),
        Shape::Tables(_) => value
            .as_array()
            .is_some_and(|a| a.iter().all(toml::Value::is_table)),
    }
}

fn hint_for(
    key: &str,
    at: &TablePath,
    fields: &[(&'static str, Shape)],
    owners: &[(Vec<&'static str>, &'static str)],
) -> Hint {
    let here = at.names();
    let elsewhere: Vec<&Vec<&'static str>> = owners
        .iter()
        .filter(|(path, k)| *k == key && *path != here)
        .map(|(path, _)| path)
        .collect();
    // An enclosing table first, nearest one: the key after a header
    let enclosing = elsewhere
        .iter()
        .filter(|path| path.len() < here.len() && here.starts_with(path))
        .max_by_key(|path| path.len());
    if let Some(path) = enclosing {
        return Hint::AfterHeader(path_of(path));
    }
    if let Some(path) = elsewhere.first() {
        return Hint::Belongs(path_of(path));
    }
    match closest_key(key, fields.iter().map(|(k, _)| *k)) {
        Some(near) => Hint::Typo(near),
        None => Hint::None,
    }
}

fn path_of(names: &[&'static str]) -> TablePath {
    TablePath(names.iter().map(|n| Step::Key(n.to_string())).collect())
}

/// The candidate nearest `key` by edit distance, within 2 edits or a third
/// of the key's length, whichever is more. Ties go to the earlier candidate.
pub fn closest_key<'a>(
    key: &str,
    candidates: impl IntoIterator<Item = &'a str>,
) -> Option<&'a str> {
    let limit = (key.chars().count() / 3).max(2);
    candidates
        .into_iter()
        .map(|c| (edit_distance(key, c), c))
        .filter(|(d, _)| *d <= limit)
        .min_by_key(|(d, _)| *d)
        .map(|(_, c)| c)
}

/// Levenshtein distance between two strings, by `char`.
pub fn edit_distance(a: &str, b: &str) -> usize {
    let b: Vec<char> = b.chars().collect();
    let mut prev: Vec<usize> = (0..=b.len()).collect();
    for (i, ca) in a.chars().enumerate() {
        let mut cur = vec![i + 1; b.len() + 1];
        for (j, cb) in b.iter().enumerate() {
            let sub = prev[j] + usize::from(ca != *cb);
            cur[j + 1] = sub.min(prev[j + 1] + 1).min(cur[j] + 1);
        }
        prev = cur;
    }
    prev[b.len()]
}

#[cfg(test)]
mod tests {
    use super::*;

    const FONT: Shape = Shape::Table(&[
        ("default_ramp", Shape::Value),
        (
            "font",
            Shape::Table(&[
                ("file", Shape::Value),
                ("size", Shape::Value),
                ("glyph_width", Shape::Value),
            ]),
        ),
    ]);

    const MANIFEST: Shape = Shape::Table(&[
        (
            "font",
            Shape::Tables(&Shape::Table(&[
                ("name", Shape::Value),
                ("file", Shape::Value),
            ])),
        ),
        (
            "egui",
            Shape::Table(&[("proportional", Shape::Value), ("monospace", Shape::Value)]),
        ),
        ("regions", Shape::Map(&Shape::Value)),
    ]);

    fn issues(text: &str, shape: &Shape) -> Vec<KeyIssue> {
        check_keys(&text.parse::<toml::Value>().unwrap(), shape)
    }

    #[test]
    fn known_keys_pass() {
        let found = issues(
            "default_ramp = \"a\"\n[font]\nfile = \"x\"\nsize = 8\n",
            &FONT,
        );
        assert!(found.is_empty(), "{found:?}");
    }

    #[test]
    fn top_level_key_after_a_header_says_move_it_above() {
        // #264's mistake: the key after [font] lands inside it
        let found = issues(
            "[font]\nfile = \"x\"\n\ndefault_ramp = \"classic\"\n",
            &FONT,
        );
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].key, "default_ramp");
        assert_eq!(found[0].table.to_string(), "[font]");
        assert_eq!(
            found[0].hint(),
            Some(&Hint::AfterHeader(TablePath::default()))
        );
        let text = found[0].to_string();
        assert!(text.contains("top-level key"), "{text}");
        assert!(text.contains("move it above"), "{text}");
    }

    #[test]
    fn misspelled_key_suggests_the_nearest() {
        let found = issues("[font]\nglyph_widht = 8\n", &FONT);
        assert_eq!(found[0].hint(), Some(&Hint::Typo("glyph_width")));
        let text = found[0].to_string();
        assert!(text.contains("did you mean `glyph_width`?"), "{text}");
        assert!(
            text.contains("expected one of: file, size, glyph_width"),
            "{text}"
        );
    }

    #[test]
    fn far_off_key_has_no_hint_but_lists_the_keys() {
        let found = issues("[font]\ncolour = 1\n", &FONT);
        assert_eq!(found[0].hint(), None);
        assert!(found[0].to_string().contains("expected one of"));
    }

    #[test]
    fn key_of_another_table_names_it() {
        let found = issues("proportional = \"body\"\n", &MANIFEST);
        assert_eq!(found[0].table.to_string(), "the top level");
        assert_eq!(found[0].hint(), Some(&Hint::Belongs(path_of(&["egui"]))));
        assert!(found[0].to_string().contains("it is a key of [egui]"));
    }

    #[test]
    fn array_entries_are_numbered() {
        let found = issues(
            "[[font]]\nname = \"a\"\nfile = \"a.ttf\"\n[[font]]\nname = \"b\"\nfiel = \"b.ttf\"\n",
            &MANIFEST,
        );
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].table.to_string(), "[[font]] #2");
        assert_eq!(found[0].hint(), Some(&Hint::Typo("file")));
    }

    #[test]
    fn single_table_where_an_array_is_wanted() {
        let found = issues("[font]\nname = \"a\"\nfile = \"a.ttf\"\n", &MANIFEST);
        assert_eq!(found.len(), 1);
        assert_eq!(
            found[0].kind,
            IssueKind::WrongKind {
                expected: "an array of tables"
            }
        );
        assert!(found[0].to_string().contains("`[[font]]`"), "{}", found[0]);
    }

    #[test]
    fn free_name_maps_take_any_name() {
        assert!(issues("[regions]\nidle_0 = 1\nanything = 2\n", &MANIFEST).is_empty());
    }

    #[test]
    fn edit_distance_counts_edits() {
        assert_eq!(edit_distance("", "abc"), 3);
        assert_eq!(edit_distance("kitten", "sitting"), 3);
        assert_eq!(edit_distance("same", "same"), 0);
        assert_eq!(
            closest_key("spacng", ["spacing", "columns"]),
            Some("spacing")
        );
        assert_eq!(closest_key("zzz", ["spacing"]), None);
    }
}
