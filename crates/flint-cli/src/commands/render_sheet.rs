//! `flint render --sweep` as one image (#216): the same moment under different
//! settings, side by side on a strip of film, each column named for what it was
//! given. The ink is the contact strip's, so the two read as one kit.

use image::RgbImage;

use super::gen_strip::{
    draw_perforations, draw_text, draw_text_right, AMBER, BONE, DIM, GLYPH_HEIGHT, MARGIN, PAPER,
    PERFORATION_BAND,
};

const GUTTER: u32 = 12;

/// Everything written on a sheet that isn't a picture.
pub struct SheetText {
    /// The scene's name, top left.
    pub title: String,
    /// The knob being swept and the moment, under it.
    pub subtitle: String,
    /// The game time, top right.
    pub time: String,
    /// One per column: the knob's value.
    pub labels: Vec<String>,
    /// One per column: anything a reader has to know to believe the label, or
    /// nothing. Zones can keep a prop off a value the sweep gave.
    pub notes: Vec<String>,
}

/// Letters big enough to read when a sheet of these columns is shown at a screen's
/// width: a sheet is far wider than a screen.
fn text_scale(columns: u32, cell_width: u32) -> u32 {
    let sheet = columns * cell_width;
    (sheet / 640).clamp(2, 5)
}

/// The cells across, with a title block above and a label under each.
pub fn compose(cells: &[RgbImage], text: &SheetText) -> RgbImage {
    let (cell_w, cell_h) = cells.first().map_or((1, 1), RgbImage::dimensions);
    let columns = cells.len() as u32;
    let scale = text_scale(columns, cell_w);
    let title_scale = scale + 1;

    let inner = columns * cell_w + columns.saturating_sub(1) * GUTTER;
    let width = inner + MARGIN * 2;
    let head = GLYPH_HEIGHT * title_scale + 5 * scale + GLYPH_HEIGHT * scale;
    let noted = text.notes.iter().any(|note| !note.is_empty());
    let label =
        4 * scale + GLYPH_HEIGHT * scale + 3 * scale + if noted { GLYPH_HEIGHT * scale } else { 0 };
    let band = PERFORATION_BAND;
    let height = MARGIN + head + 6 + band + cell_h + label + 6 + band + MARGIN;
    let mut sheet = RgbImage::from_pixel(width, height, PAPER);

    let mut y = MARGIN;
    let title_upper = text.title.to_uppercase();
    draw_text(&mut sheet, MARGIN, y, title_scale, AMBER, &title_upper);
    draw_text_right(
        &mut sheet,
        width - MARGIN,
        y,
        title_scale,
        BONE,
        &text.time.to_uppercase(),
    );
    y += GLYPH_HEIGHT * title_scale + 5 * scale;
    draw_text(
        &mut sheet,
        MARGIN,
        y,
        scale,
        DIM,
        &text.subtitle.to_uppercase(),
    );
    y += GLYPH_HEIGHT * scale + 6;
    draw_perforations(&mut sheet, y);
    y += band;

    for (i, cell) in cells.iter().enumerate() {
        let x = MARGIN + i as u32 * (cell_w + GUTTER);
        image::imageops::replace(&mut sheet, cell, i64::from(x), i64::from(y));
        let under = y + cell_h + 4 * scale;
        draw_text(
            &mut sheet,
            x,
            under,
            scale,
            AMBER,
            &text.labels[i].to_uppercase(),
        );
        if let Some(note) = text.notes.get(i).filter(|n| !n.is_empty()) {
            let below = under + GLYPH_HEIGHT * scale + 3 * scale;
            draw_text(&mut sheet, x, below, scale, DIM, &note.to_uppercase());
        }
    }
    draw_perforations(&mut sheet, height - MARGIN - band);
    sheet
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::Rgb;

    const RED: Rgb<u8> = Rgb([200, 10, 10]);
    const BLUE: Rgb<u8> = Rgb([10, 10, 200]);

    fn text(labels: &[&str]) -> SheetText {
        SheetText {
            title: "A Scene".into(),
            subtitle: "wooze.glide".into(),
            time: "T = 3.50 S".into(),
            labels: labels.iter().map(|l| l.to_string()).collect(),
            notes: vec![String::new(); labels.len()],
        }
    }

    #[test]
    fn four_columns_stand_side_by_side_at_full_size() {
        let cells = vec![RgbImage::from_pixel(64, 48, RED); 4];
        let sheet = compose(&cells, &text(&["0", "0.25", "0.5", "1"]));
        assert_eq!(sheet.width(), 4 * 64 + 3 * GUTTER + 2 * MARGIN);
        assert!(sheet.height() > 48);
        let red_pixels = sheet.pixels().filter(|p| **p == RED).count();
        assert_eq!(
            red_pixels,
            4 * 64 * 48,
            "a cell was cropped or written over"
        );
    }

    #[test]
    fn each_column_sits_where_its_order_says() {
        let cells = [
            RgbImage::from_pixel(32, 32, RED),
            RgbImage::from_pixel(32, 32, BLUE),
        ];
        let sheet = compose(&cells, &text(&["0", "1"]));
        let first_of = |color: Rgb<u8>| {
            sheet
                .enumerate_pixels()
                .filter(|(_, _, p)| **p == color)
                .map(|(x, _, _)| x)
                .min()
                .unwrap()
        };
        assert_eq!(first_of(RED), MARGIN);
        assert_eq!(first_of(BLUE), MARGIN + 32 + GUTTER);
    }

    #[test]
    fn labels_are_really_drawn_and_differ_by_column() {
        let cells = vec![RgbImage::from_pixel(64, 48, RED); 2];
        let amber = |sheet: &RgbImage| sheet.pixels().filter(|p| **p == AMBER).count();
        let short = compose(&cells, &text(&["1", "1"]));
        let long = compose(&cells, &text(&["0.25", "0.75"]));
        assert_eq!(short.dimensions(), long.dimensions());
        assert!(amber(&long) > amber(&short));
    }

    #[test]
    fn a_note_prints_under_its_label() {
        let cells = vec![RgbImage::from_pixel(64, 48, RED); 2];
        let dim = |sheet: &RgbImage| sheet.pixels().filter(|p| **p == DIM).count();
        let plain = compose(&cells, &text(&["0", "1"]));
        let mut noted = text(&["0", "1"]);
        noted.notes[1] = "props read 0.3".into();
        assert!(dim(&compose(&cells, &noted)) > dim(&plain));
    }

    #[test]
    fn the_same_sheet_comes_out_the_same() {
        let cells = vec![RgbImage::from_pixel(64, 48, RED); 3];
        let t = text(&["a", "b", "c"]);
        assert_eq!(compose(&cells, &t), compose(&cells, &t));
    }
}
