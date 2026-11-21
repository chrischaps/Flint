//! A `.spline.toml` file, read with every mistake placed (#750).
//!
//! ```toml
//! [spline]
//! closed = true          # a loop; false for a path with two ends
//!
//! [sampling]
//! spacing = 2.0          # metres between samples (default 2)
//!
//! [[control_points]]
//! position = [0, 0, 0]
//! twist = 0.0            # banking in degrees (default 0)
//! gap_after = false      # no geometry from here to the next point (default false)
//! attrs = { surface = "ice", speed = 0.6 }   # what holds from here to the next point
//! ```
//!
//! A point's `attrs` are the game's own data along the path: each holds
//! from its point to the next, and neighbours with the same value are one
//! span ([`SplineFile::attr_spans`]), which scripts read with
//! `spline_point_attr` and `spline_point_attr_span`. `gap_after` reads as
//! the attribute `gap_after`, `true` over each gap, so a game's gap logic
//! goes through the same call (#855).
//!
//! [`SplineFile::parse`] returns a [`SplineFileError`] that names the file,
//! the line and the column of what is wrong: a TOML syntax error, a missing
//! `position`, a position of two numbers, a spacing of zero. Keys the format
//! doesn't use (`[spline] name`) are left alone.

use std::collections::BTreeMap;
use std::fmt;
use std::ops::Range;
use std::path::{Path, PathBuf};

use flint_core::toml_util::line_column;
use flint_core::{vfs, Vec3};
use serde::Deserialize;
use toml::Spanned;

use crate::spline::{self, Span, SplineControlPoint, SplineSample};

/// Metres between samples when `[sampling] spacing` is not written.
pub const DEFAULT_SPACING: f32 = 2.0;

/// A spline file as read: the control points and how to sample them.
#[derive(Debug, Clone)]
pub struct SplineFile {
    /// A loop (`[spline] closed`, default true) or a path with two ends.
    pub closed: bool,
    /// Metres between samples (`[sampling] spacing`).
    pub spacing: f32,
    /// At least two.
    pub control_points: Vec<SplineControlPoint>,
    /// Each control point's `attrs` (empty where it has none), in the
    /// same order.
    pub point_attrs: Vec<toml::Table>,
}

/// The attribute `gap_after` reads as: a point's own key, so no `attrs`
/// table may name it.
pub const GAP_ATTR: &str = "gap_after";

/// What is wrong with a spline file, and where.
#[derive(Debug, Clone, PartialEq)]
pub struct SplineFileError {
    /// The file, as it was named to [`SplineFile::read`] or
    /// [`SplineFile::parse`].
    pub file: PathBuf,
    /// The line, from 1. `None` when the file could not be read at all.
    pub line: Option<usize>,
    /// The column on `line`, from 1, counted in characters.
    pub column: Option<usize>,
    /// What is wrong, in one line.
    pub message: String,
}

impl fmt::Display for SplineFileError {
    /// `splines/oval.spline.toml:12:1: missing field `position``
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.file.display())?;
        if let Some(line) = self.line {
            write!(f, ":{line}")?;
            if let Some(column) = self.column {
                write!(f, ":{column}")?;
            }
        }
        write!(f, ": {}", self.message)
    }
}

impl std::error::Error for SplineFileError {}

// The file's shape. Each field the engine reads is typed, so serde names a
// wrong type or a missing key, and toml places it.

#[derive(Deserialize)]
struct Doc {
    spline: SplineTable,
    #[serde(default)]
    sampling: Option<Sampling>,
    control_points: Spanned<Vec<Spanned<PointDoc>>>,
}

#[derive(Deserialize)]
struct SplineTable {
    #[serde(default = "closed_by_default")]
    closed: bool,
}

fn closed_by_default() -> bool {
    true
}

#[derive(Deserialize)]
struct Sampling {
    #[serde(default)]
    spacing: Option<Spanned<f32>>,
}

#[derive(Deserialize)]
struct PointDoc {
    position: Spanned<Vec<f32>>,
    #[serde(default)]
    twist: f32,
    #[serde(default)]
    gap_after: bool,
    #[serde(default)]
    attrs: Option<Spanned<toml::Table>>,
}

impl SplineFile {
    /// Read and parse the file at `path` (through the vfs).
    pub fn read(path: &Path) -> Result<Self, SplineFileError> {
        let text = vfs::read_to_string(path).map_err(|e| SplineFileError {
            file: path.to_path_buf(),
            line: None,
            column: None,
            message: format!("cannot be read: {e}"),
        })?;
        Self::parse(&text, path)
    }

    /// Parse a spline file's text. `file` is only for the error.
    pub fn parse(text: &str, file: &Path) -> Result<Self, SplineFileError> {
        let at = |span: Option<Range<usize>>, message: String| {
            let (line, column) = match span {
                Some(span) => {
                    let (line, column) = line_column(text, span.start);
                    (Some(line), Some(column))
                }
                None => (None, None),
            };
            SplineFileError {
                file: file.to_path_buf(),
                line,
                column,
                message,
            }
        };

        let doc: Doc = toml::from_str(text).map_err(|e| at(e.span(), one_line(e.message())))?;

        let spacing = match doc.sampling.and_then(|s| s.spacing) {
            Some(spacing) => {
                let value = *spacing.get_ref();
                if !(value.is_finite() && value > 0.0) {
                    return Err(at(
                        Some(spacing.span()),
                        format!(
                            "`spacing` is {value}: it must be above 0 (metres between samples)"
                        ),
                    ));
                }
                value
            }
            None => DEFAULT_SPACING,
        };

        let points_span = doc.control_points.span();
        let mut control_points = Vec::new();
        let mut point_attrs = Vec::new();
        for point in doc.control_points.into_inner() {
            let point = point.into_inner();
            let position = point.position.get_ref();
            if position.len() != 3 {
                return Err(at(
                    Some(point.position.span()),
                    format!(
                        "`position` has {} number{}: it is [x, y, z]",
                        position.len(),
                        if position.len() == 1 { "" } else { "s" }
                    ),
                ));
            }
            control_points.push(SplineControlPoint {
                position: Vec3::new(position[0], position[1], position[2]),
                twist: point.twist,
                gap_after: point.gap_after,
            });
            let attrs = match point.attrs {
                Some(attrs) => {
                    if attrs.get_ref().contains_key(GAP_ATTR) {
                        return Err(at(
                            Some(attrs.span()),
                            format!(
                                "`attrs` names `{GAP_ATTR}`, which is the point's own key: \
                                 write `{GAP_ATTR} = true` beside `position`"
                            ),
                        ));
                    }
                    attrs.into_inner()
                }
                None => toml::Table::new(),
            };
            point_attrs.push(attrs);
        }
        if control_points.len() < 2 {
            return Err(at(
                Some(points_span),
                format!(
                    "a spline needs at least 2 control points, this has {}",
                    control_points.len()
                ),
            ));
        }

        Ok(SplineFile {
            closed: doc.spline.closed,
            spacing,
            control_points,
            point_attrs,
        })
    }

    /// The path sampled every `spacing` metres.
    pub fn samples(&self) -> Vec<SplineSample> {
        if self.closed {
            spline::sample_closed_spline(&self.control_points, self.spacing)
        } else {
            spline::sample_open_spline(&self.control_points, self.spacing)
        }
    }

    /// The `t` ranges `gap_after` leaves empty.
    pub fn gap_ranges(&self) -> Vec<(f32, f32)> {
        spline::compute_gap_ranges(&self.control_points, self.closed)
    }

    /// Every attribute's spans, by name: each point's `attrs`, and
    /// [`GAP_ATTR`] as `true` over the gaps (absent where there are none).
    /// A point that doesn't name an attribute leaves it unset from there
    /// to the next point; it doesn't carry the last value on.
    pub fn attr_spans(&self) -> BTreeMap<String, Vec<Span<toml::Value>>> {
        let mut names: Vec<&String> = self.point_attrs.iter().flat_map(|a| a.keys()).collect();
        names.sort();
        names.dedup();
        let mut out = BTreeMap::new();
        for name in names {
            let values: Vec<Option<toml::Value>> = self
                .point_attrs
                .iter()
                .map(|a| a.get(name).cloned())
                .collect();
            let spans = spline::segment_spans(&values, self.closed);
            if !spans.is_empty() {
                out.insert(name.clone(), spans);
            }
        }
        let gaps: Vec<Span<toml::Value>> = self
            .gap_ranges()
            .into_iter()
            .map(|(start, end)| Span {
                start,
                end,
                value: toml::Value::Boolean(true),
            })
            .collect();
        if !gaps.is_empty() {
            out.insert(GAP_ATTR.to_string(), gaps);
        }
        out
    }
}

/// A message on one line: toml's run over several only for its snippet,
/// which the line and column replace.
fn one_line(message: &str) -> String {
    message.split_whitespace().collect::<Vec<_>>().join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(text: &str) -> Result<SplineFile, SplineFileError> {
        SplineFile::parse(text, Path::new("splines/test.spline.toml"))
    }

    const GOOD: &str = r#"
[spline]
name = "Oval"
closed = false

[sampling]
spacing = 4

[[control_points]]
position = [0, 0, 0]

[[control_points]]
position = [0.0, 1.5, -50]
twist = 5
gap_after = true
"#;

    #[test]
    fn a_good_file_reads_with_integers_as_numbers() {
        let file = parse(GOOD).unwrap();
        assert!(!file.closed);
        assert_eq!(file.spacing, 4.0);
        assert_eq!(file.control_points.len(), 2);
        assert_eq!(file.control_points[1].position, Vec3::new(0.0, 1.5, -50.0));
        assert_eq!(file.control_points[1].twist, 5.0);
        assert!(file.control_points[1].gap_after);
        assert!(!file.samples().is_empty());
    }

    #[test]
    fn defaults_are_a_loop_sampled_every_two_metres() {
        let file = parse(
            "[spline]\n[[control_points]]\nposition = [0, 0, 0]\n\
             [[control_points]]\nposition = [10, 0, 0]\n[[control_points]]\nposition = [10, 0, 10]\n",
        )
        .unwrap();
        assert!(file.closed);
        assert_eq!(file.spacing, DEFAULT_SPACING);
        assert_eq!(file.control_points[0].twist, 0.0);
    }

    #[test]
    fn a_missing_position_names_the_line() {
        let text = "[spline]\n\n[[control_points]]\nposition = [0, 0, 0]\n\n[[control_points]]\ntwist = 3\n";
        let err = parse(text).unwrap_err();
        assert_eq!(err.line, Some(6), "{err}");
        assert!(err.message.contains("position"), "{err}");
        assert!(
            err.to_string().starts_with("splines/test.spline.toml:6:"),
            "{err}"
        );
    }

    #[test]
    fn a_syntax_error_names_line_and_column() {
        let text = "[spline]\nclosed = true\n\n[[control_points]]\nposition = [0, 0 0]\n";
        let err = parse(text).unwrap_err();
        assert_eq!((err.line, err.column), (Some(5), Some(18)), "{err}");
        assert!(!err.message.contains('\n'), "{err}");
    }

    #[test]
    fn a_short_position_is_refused_where_it_is() {
        let text = "[spline]\n[[control_points]]\nposition = [0, 0, 0]\n[[control_points]]\nposition =   [1, 2]\n";
        let err = parse(text).unwrap_err();
        assert_eq!((err.line, err.column), (Some(5), Some(14)), "{err}");
        assert!(err.message.contains("has 2 numbers"), "{err}");
    }

    #[test]
    fn a_wrong_type_is_placed() {
        let text = "[spline]\nclosed = \"yes\"\n[[control_points]]\nposition = [0, 0, 0]\n";
        let err = parse(text).unwrap_err();
        assert_eq!(err.line, Some(2), "{err}");
    }

    #[test]
    fn zero_spacing_and_one_point_are_refused() {
        let err = parse("[spline]\n[sampling]\nspacing = 0\n[[control_points]]\nposition = [0, 0, 0]\n[[control_points]]\nposition = [1, 0, 0]\n").unwrap_err();
        assert_eq!(err.line, Some(3), "{err}");
        let err = parse("[spline]\n[[control_points]]\nposition = [0, 0, 0]\n").unwrap_err();
        assert!(err.message.contains("at least 2"), "{err}");
        assert!(err.line.is_some(), "{err}");
    }

    /// A loop of four points: ice from the first to the third, a gap after
    /// the second, and a speed that changes at the third.
    const ATTRS: &str = r#"
[spline]
[[control_points]]
position = [0, 0, 0]
attrs = { surface = "ice", speed = 1 }
[[control_points]]
position = [10, 0, 0]
gap_after = true
attrs = { surface = "ice", speed = 1 }
[[control_points]]
position = [10, 0, 10]
[control_points.attrs]
speed = 0.5
[[control_points]]
position = [0, 0, 10]
attrs = { speed = 1 }
"#;

    #[test]
    fn attributes_hold_to_the_next_point_and_merge_into_spans() {
        let file = parse(ATTRS).unwrap();
        assert_eq!(file.point_attrs.len(), 4);
        let spans = file.attr_spans();
        let of = |name: &str| -> Vec<(f32, f32, toml::Value)> {
            spans[name]
                .iter()
                .map(|s| (s.start, s.end, s.value.clone()))
                .collect()
        };
        assert_eq!(
            of("surface"),
            vec![(0.0, 0.5, toml::Value::String("ice".into()))]
        );
        // speed 1 runs from the last point round through the first two
        assert_eq!(
            of("speed"),
            vec![
                (0.75, 0.5, toml::Value::Integer(1)),
                (0.5, 0.75, toml::Value::Float(0.5))
            ]
        );
        // gap_after is an attribute too, true over the gap only
        assert_eq!(of(GAP_ATTR), vec![(0.25, 0.5, toml::Value::Boolean(true))]);
        assert_eq!(
            file.gap_ranges(),
            vec![(0.25, 0.5)],
            "the same stretch the sweep leaves empty"
        );
        let names: Vec<_> = spans.keys().map(String::as_str).collect();
        assert_eq!(names, ["gap_after", "speed", "surface"]);
    }

    #[test]
    fn no_attributes_and_no_gaps_is_no_spans() {
        let file = parse(GOOD.replace("gap_after = true", "").as_str()).unwrap();
        assert!(file.attr_spans().is_empty());
        assert!(file.point_attrs.iter().all(toml::Table::is_empty));
    }

    #[test]
    fn attrs_naming_gap_after_or_not_a_table_are_placed() {
        let text = "[spline]\n[[control_points]]\nposition = [0, 0, 0]\nattrs = { gap_after = true }\n[[control_points]]\nposition = [1, 0, 0]\n";
        let err = parse(text).unwrap_err();
        assert_eq!((err.line, err.column), (Some(4), Some(9)), "{err}");
        assert!(err.message.contains("point's own key"), "{err}");
        let text = "[spline]\n[[control_points]]\nposition = [0, 0, 0]\nattrs = 3\n[[control_points]]\nposition = [1, 0, 0]\n";
        let err = parse(text).unwrap_err();
        assert_eq!(err.line, Some(4), "{err}");
    }

    #[test]
    fn a_missing_spline_table_is_named() {
        let err = parse("[[control_points]]\nposition = [0, 0, 0]\n").unwrap_err();
        assert!(err.message.contains("spline"), "{err}");
    }

    #[test]
    fn an_unreadable_file_has_no_line() {
        let err = SplineFile::read(Path::new("no/such/dir/x.spline.toml")).unwrap_err();
        assert_eq!(err.line, None);
        assert!(err
            .to_string()
            .starts_with("no/such/dir/x.spline.toml: cannot be read"));
    }
}
