//! Columns lined up for a terminal: a header, rows, each column as wide as
//! its widest cell, numbers right-aligned.

pub(crate) struct Table {
    header: Vec<&'static str>,
    /// Which columns are numbers (right-aligned).
    numeric: Vec<bool>,
    rows: Vec<Vec<String>>,
}

impl Table {
    /// Columns named in `header`; a name starting with `>` is a number
    /// column (the `>` isn't shown).
    pub fn new(header: &[&'static str]) -> Self {
        Self {
            numeric: header.iter().map(|h| h.starts_with('>')).collect(),
            header: header.iter().map(|h| h.trim_start_matches('>')).collect(),
            rows: Vec::new(),
        }
    }

    pub fn row(&mut self, cells: Vec<String>) {
        self.rows.push(cells);
    }

    pub fn is_empty(&self) -> bool {
        self.rows.is_empty()
    }

    /// The table, each line indented by `indent`.
    pub fn render(&self, indent: &str) -> String {
        let columns = self.header.len();
        let mut widths: Vec<usize> = self.header.iter().map(|h| h.chars().count()).collect();
        for row in &self.rows {
            for (i, cell) in row.iter().enumerate().take(columns) {
                widths[i] = widths[i].max(cell.chars().count());
            }
        }
        let line = |cells: Vec<&str>| {
            let mut out = String::from(indent);
            for (i, cell) in cells.iter().enumerate() {
                let pad = widths[i].saturating_sub(cell.chars().count());
                let last = i + 1 == cells.len();
                if self.numeric[i] {
                    out.push_str(&" ".repeat(pad));
                    out.push_str(cell);
                } else {
                    out.push_str(cell);
                    if !last {
                        out.push_str(&" ".repeat(pad));
                    }
                }
                if !last {
                    out.push_str("  ");
                }
            }
            out.trim_end().to_string() + "\n"
        };
        let mut out = line(self.header.clone());
        for row in &self.rows {
            out.push_str(&line(row.iter().map(String::as_str).collect()));
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn columns_line_up_and_numbers_go_right() {
        let mut t = Table::new(&["pass", ">ms"]);
        t.row(vec!["Scene".into(), "1.25".into()]);
        t.row(vec!["Bloom".into(), "10.50".into()]);
        assert_eq!(t.render(""), "pass      ms\nScene   1.25\nBloom  10.50\n");
    }
}
