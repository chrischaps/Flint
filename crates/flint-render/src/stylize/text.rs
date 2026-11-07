//! A glyph style's frame as text (#317).
//!
//! The ASCII style draws a real character grid. With text capture on
//! ([`StyleSet::set_text_capture`](super::StyleSet::set_text_capture)) it
//! also writes, a texel a cell, which glyph each cell of the screen grid
//! shows and in which colours, and copies that to the CPU with the frame.
//! [`GlyphText`] is the result: `rows` lines of `cols` characters, the
//! same cells as the picture, so an agent can read a render instead of
//! looking at it, two runs diff line by line, and a test can say "the box
//! is `#`" without pixel tolerances (`flint render --ascii-text`).

/// One cell of a [`GlyphText`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct GlyphCell {
    /// The glyph the cell shows, or `None` where the picture passes through
    /// (a cell no styled entity covers, under `scope = "entities"`).
    pub glyph: Option<char>,
    /// The glyph's ink, 8-bit display RGB (the picture's colour when passed).
    pub ink: [u8; 3],
    /// The ground behind it, likewise.
    pub ground: [u8; 3],
}

/// What a passed-through cell is written as.
pub const PASSED: char = ' ';

/// The glyph grid one frame drew, row by row from the top left.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GlyphText {
    pub cols: u32,
    pub rows: u32,
    pub cells: Vec<GlyphCell>,
}

impl GlyphText {
    /// The cell at column `x`, row `y`.
    pub fn cell(&self, x: u32, y: u32) -> Option<&GlyphCell> {
        (x < self.cols && y < self.rows).then(|| &self.cells[(y * self.cols + x) as usize])
    }

    /// Row `y` as characters.
    pub fn line(&self, y: u32) -> String {
        (0..self.cols)
            .map(|x| self.cell(x, y).and_then(|c| c.glyph).unwrap_or(PASSED))
            .collect()
    }

    /// The grid as UTF-8: `rows` lines of `cols` characters, each ending
    /// in `\n`.
    pub fn to_text(&self) -> String {
        let mut out = String::with_capacity(((self.cols + 1) * self.rows) as usize);
        for y in 0..self.rows {
            out.push_str(&self.line(y));
            out.push('\n');
        }
        out
    }

    /// The grid for a 24-bit colour terminal: each glyph in its ink over
    /// its ground, a colour code only where one changes, reset at each
    /// line's end.
    pub fn to_ansi(&self) -> String {
        let mut out = String::new();
        for y in 0..self.rows {
            let mut last: Option<([u8; 3], [u8; 3])> = None;
            for x in 0..self.cols {
                let c = self.cell(x, y).expect("in the grid");
                if last != Some((c.ink, c.ground)) {
                    let [r, g, b] = c.ink;
                    let [br, bg, bb] = c.ground;
                    out.push_str(&format!("\x1b[38;2;{r};{g};{b}m\x1b[48;2;{br};{bg};{bb}m"));
                    last = Some((c.ink, c.ground));
                }
                out.push(c.glyph.unwrap_or(PASSED));
            }
            out.push_str("\x1b[0m\n");
        }
        out
    }

    /// How many cells show each glyph, most first (ties by character).
    pub fn histogram(&self) -> Vec<(char, usize)> {
        let mut counts = std::collections::BTreeMap::new();
        for c in &self.cells {
            *counts.entry(c.glyph.unwrap_or(PASSED)).or_insert(0usize) += 1;
        }
        let mut list: Vec<_> = counts.into_iter().collect();
        list.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
        list
    }
}

/// The text texel's "no glyph" code (`TEXT_NONE` in ascii.wgsl).
pub(crate) const TEXT_NONE: u32 = u32::MAX;

/// Unpack an 8-bit RGB the text pass packed (`pack_rgb`).
pub(crate) fn unpack_rgb(v: u32) -> [u8; 3] {
    [v as u8, (v >> 8) as u8, (v >> 16) as u8]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cell(glyph: Option<char>, ink: [u8; 3]) -> GlyphCell {
        GlyphCell {
            glyph,
            ink,
            ground: [0, 0, 0],
        }
    }

    fn grid() -> GlyphText {
        GlyphText {
            cols: 3,
            rows: 2,
            cells: vec![
                cell(Some('#'), [255, 0, 0]),
                cell(Some('#'), [255, 0, 0]),
                cell(None, [9, 9, 9]),
                cell(Some('.'), [0, 255, 0]),
                cell(Some('╱'), [0, 255, 0]),
                cell(Some('.'), [1, 2, 3]),
            ],
        }
    }

    #[test]
    fn text_is_rows_of_cols_characters_with_passed_cells_blank() {
        assert_eq!(grid().to_text(), "## \n.╱.\n");
        assert_eq!(grid().line(1).chars().count(), 3);
    }

    #[test]
    fn ansi_writes_a_colour_only_where_it_changes() {
        let ansi = grid().to_ansi();
        let first = ansi.lines().next().unwrap();
        // Red for the two #, then grey for the passed cell
        assert_eq!(first.matches("\x1b[38;2;").count(), 2);
        assert!(first.starts_with("\x1b[38;2;255;0;0m\x1b[48;2;0;0;0m##"));
        assert!(first.ends_with("\x1b[0m"));
        assert_eq!(ansi.lines().count(), 2);
    }

    #[test]
    fn histogram_counts_most_first() {
        assert_eq!(grid().histogram()[0], ('#', 2));
        assert_eq!(unpack_rgb(0x00_33_22_11), [0x11, 0x22, 0x33]);
    }
}
