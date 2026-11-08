//! A sweep's contact sheet (#279): the same frame at several values of one
//! setting, side by side, each labelled with what it was given. The letters
//! are the ASCII style's own built-in 8x8 glyphs (#264), so the sheet speaks
//! in the same type the styles draw with.

use flint_render::glyph_atlas::GlyphAtlas;
use image::{Rgba, RgbaImage};

/// Ink on the sheet.
pub const PAPER: Rgba<u8> = Rgba([16, 17, 19, 255]);
pub const FRAME: Rgba<u8> = Rgba([46, 48, 54, 255]);
pub const TITLE: Rgba<u8> = Rgba([236, 178, 92, 255]);
pub const LABEL: Rgba<u8> = Rgba([226, 220, 206, 255]);
pub const DIM: Rgba<u8> = Rgba([128, 128, 134, 255]);

const MARGIN: u32 = 24;
const GUTTER: u32 = 16;
/// Columns before a sheet wraps to another row.
const PER_ROW: usize = 4;
const GLYPH: u32 = 8;

/// Everything on a sheet that isn't a picture.
pub struct SheetText {
    /// Top left, large: the scene.
    pub title: String,
    /// Under it: what was swept, and over what.
    pub subtitle: String,
    /// One per cell: the value it took.
    pub labels: Vec<String>,
}

/// How big the letters are: a glyph pixel per `scale` image pixels.
fn text_scale(cell_width: u32) -> u32 {
    (cell_width / 220).clamp(1, 4)
}

/// Lay `cells` (all one size) out in rows of up to four, under a title,
/// each with its label beneath.
pub fn compose(cells: &[RgbaImage], text: &SheetText) -> RgbaImage {
    let (cell_w, cell_h) = cells.first().map_or((1, 1), RgbaImage::dimensions);
    let columns = cells.len().clamp(1, PER_ROW) as u32;
    let rows = cells.len().div_ceil(PER_ROW).max(1) as u32;
    let scale = text_scale(cell_w);
    let title_scale = scale + 1;
    let line = GLYPH * scale;

    let head = GLYPH * title_scale + 6 * scale + line + 10 * scale;
    let label_band = 6 * scale + line + 4 * scale;
    let row_h = cell_h + 2 + label_band;
    let width = MARGIN * 2 + columns * (cell_w + 2) + (columns - 1) * GUTTER;
    let height = MARGIN * 2 + head + rows * row_h + (rows - 1) * GUTTER;
    let mut sheet = RgbaImage::from_pixel(width, height, PAPER);
    let font = GlyphAtlas::builtin();

    let mut y = MARGIN;
    draw_text(
        &mut sheet,
        &font,
        MARGIN,
        y,
        title_scale,
        TITLE,
        &text.title,
    );
    y += GLYPH * title_scale + 6 * scale;
    draw_text(&mut sheet, &font, MARGIN, y, scale, DIM, &text.subtitle);
    let top = MARGIN + head;

    for (i, cell) in cells.iter().enumerate() {
        let (col, row) = ((i % PER_ROW) as u32, (i / PER_ROW) as u32);
        let x = MARGIN + col * (cell_w + 2 + GUTTER);
        let y = top + row * (row_h + GUTTER);
        debug_assert_eq!((x + 1, y + 1), cell_origin(cell_w, cell_h, i));
        frame(&mut sheet, x, y, cell_w + 2, cell_h + 2);
        image::imageops::replace(&mut sheet, cell, i64::from(x + 1), i64::from(y + 1));
        if let Some(label) = text.labels.get(i) {
            let fit = fit_text(label, cell_w, scale);
            draw_text(
                &mut sheet,
                &font,
                x + 1,
                y + cell_h + 2 + 6 * scale,
                scale,
                LABEL,
                &fit,
            );
        }
    }
    sheet
}

/// Where cell `i`'s picture sits on a [`compose`]d sheet of `cell_w` x
/// `cell_h` cells (its top-left pixel, inside the frame), so a reader can
/// crop it back out (#357's report test).
pub fn cell_origin(cell_w: u32, cell_h: u32, i: usize) -> (u32, u32) {
    let scale = text_scale(cell_w);
    let line = GLYPH * scale;
    let head = GLYPH * (scale + 1) + 6 * scale + line + 10 * scale;
    let row_h = cell_h + 2 + 6 * scale + line + 4 * scale;
    let (col, row) = ((i % PER_ROW) as u32, (i / PER_ROW) as u32);
    (
        MARGIN + col * (cell_w + 2 + GUTTER) + 1,
        MARGIN + head + row * (row_h + GUTTER) + 1,
    )
}

/// `text` shortened with `~` so it fits `width` pixels.
fn fit_text(text: &str, width: u32, scale: u32) -> String {
    let max = (width / (GLYPH * scale)).max(1) as usize;
    let chars: Vec<char> = text.chars().collect();
    if chars.len() <= max {
        return text.to_string();
    }
    let mut short: String = chars[..max.saturating_sub(1)].iter().collect();
    short.push('~');
    short
}

/// A one-pixel frame around a cell.
fn frame(sheet: &mut RgbaImage, x: u32, y: u32, w: u32, h: u32) {
    for i in 0..w {
        sheet.put_pixel(x + i, y, FRAME);
        sheet.put_pixel(x + i, y + h - 1, FRAME);
    }
    for j in 0..h {
        sheet.put_pixel(x, y + j, FRAME);
        sheet.put_pixel(x + w - 1, y + j, FRAME);
    }
}

/// Draw `text` with the built-in glyphs, `scale` image pixels a glyph pixel,
/// blended by coverage. Characters the font lacks draw as `?`; text past
/// the sheet's edge is clipped.
pub fn draw_text(
    sheet: &mut RgbaImage,
    font: &GlyphAtlas,
    x: u32,
    y: u32,
    scale: u32,
    color: Rgba<u8>,
    text: &str,
) {
    let (w, h) = sheet.dimensions();
    for (i, ch) in text.chars().enumerate() {
        let ch = if font.contains(ch) { ch } else { '?' };
        let gx = x + i as u32 * GLYPH * scale;
        for py in 0..GLYPH {
            for px in 0..GLYPH {
                let a = font.sample(ch, px, py).unwrap_or(0);
                if a == 0 {
                    continue;
                }
                for sy in 0..scale {
                    for sx in 0..scale {
                        let (ox, oy) = (gx + px * scale + sx, y + py * scale + sy);
                        if ox >= w || oy >= h {
                            continue;
                        }
                        let under = *sheet.get_pixel(ox, oy);
                        sheet.put_pixel(ox, oy, blend(under, color, a));
                    }
                }
            }
        }
    }
}

/// The style mask view's legend (#309), in the image's lower left: a
/// swatch of each style id's hue, its id and its name, on a dark card.
/// Nothing styled still gets a card saying so, so an empty mask reads as
/// empty rather than broken.
pub fn draw_legend(img: &mut RgbaImage, entries: &[flint_render::style_mask::LegendEntry]) {
    let rows: Vec<(Option<[f32; 3]>, String)> = if entries.is_empty() {
        vec![(None, "no styled entities".to_string())]
    } else {
        entries
            .iter()
            .map(|e| (Some(e.color), format!("{} {}", e.id, e.name)))
            .collect()
    };
    let scale = (img.width() / 640).clamp(1, 3);
    draw_card(img, "style mask", &rows, scale);
}

/// A legend card in the image's lower left: `title`, then a row each of
/// a swatch (display values) and its text, or the text alone. The style
/// mask view's (#309) and the ASCII edge view's (#349). `scale` is image
/// pixels a font pixel.
pub fn draw_card(
    img: &mut RgbaImage,
    title: &str,
    rows: &[(Option<[f32; 3]>, String)],
    scale: u32,
) {
    let (w, h) = img.dimensions();
    let line = GLYPH * scale;
    let pad = 6 * scale;
    let gap = 4 * scale;
    let longest = rows
        .iter()
        .map(|(_, t)| t.chars().count())
        .max()
        .unwrap_or(0) as u32;
    let swatch = line;
    let card_w = (pad * 2 + swatch + gap * 2 + longest * line).min(w);
    let header = line + gap;
    let card_h = (pad * 2 + header + rows.len() as u32 * (line + gap) - gap).min(h);
    let margin = 8 * scale;
    let x0 = margin.min(w.saturating_sub(card_w));
    let y0 = h.saturating_sub(card_h + margin);
    for y in y0..(y0 + card_h).min(h) {
        for x in x0..(x0 + card_w).min(w) {
            let under = *img.get_pixel(x, y);
            img.put_pixel(x, y, blend(under, PAPER, 215));
        }
    }
    let font = GlyphAtlas::builtin();
    draw_text(img, &font, x0 + pad, y0 + pad, scale, DIM, title);
    for (i, (color, text)) in rows.iter().enumerate() {
        let y = y0 + pad + header + i as u32 * (line + gap);
        let mut tx = x0 + pad;
        if let Some([r, g, b]) = color {
            let byte = |v: f32| (v.clamp(0.0, 1.0) * 255.0).round() as u8;
            let c = Rgba([byte(*r), byte(*g), byte(*b), 255]);
            for sy in y..(y + swatch).min(h) {
                for sx in tx..(tx + swatch).min(w) {
                    img.put_pixel(sx, sy, c);
                }
            }
            tx += swatch + gap * 2;
        }
        draw_text(img, &font, tx, y, scale, LABEL, text);
    }
}

fn blend(under: Rgba<u8>, over: Rgba<u8>, alpha: u8) -> Rgba<u8> {
    let a = u32::from(alpha);
    let mix = |u: u8, o: u8| ((u32::from(u) * (255 - a) + u32::from(o) * a) / 255) as u8;
    Rgba([
        mix(under[0], over[0]),
        mix(under[1], over[1]),
        mix(under[2], over[2]),
        255,
    ])
}

#[cfg(test)]
mod tests {
    use super::*;

    const RED: Rgba<u8> = Rgba([200, 10, 10, 255]);
    const BLUE: Rgba<u8> = Rgba([10, 10, 200, 255]);

    fn text(labels: &[&str]) -> SheetText {
        SheetText {
            title: "A Scene".into(),
            subtitle: "ascii.cell".into(),
            labels: labels.iter().map(|l| l.to_string()).collect(),
        }
    }

    fn count(sheet: &RgbaImage, color: Rgba<u8>) -> usize {
        sheet.pixels().filter(|p| **p == color).count()
    }

    #[test]
    fn every_cell_lands_whole() {
        let cells = vec![RgbaImage::from_pixel(64, 48, RED); 3];
        let sheet = compose(&cells, &text(&["6", "8", "12"]));
        assert_eq!(
            count(&sheet, RED),
            3 * 64 * 48,
            "a cell was cropped or drawn over"
        );
        assert_eq!(sheet.width(), 2 * MARGIN + 3 * 66 + 2 * GUTTER);
    }

    #[test]
    fn cells_sit_in_order_and_wrap_after_four() {
        let mut cells = vec![RgbaImage::from_pixel(32, 32, RED); 5];
        cells[1] = RgbaImage::from_pixel(32, 32, BLUE);
        cells[4] = RgbaImage::from_pixel(32, 32, BLUE);
        let sheet = compose(&cells, &text(&["a", "b", "c", "d", "e"]));
        let blue: Vec<(u32, u32)> = sheet
            .enumerate_pixels()
            .filter(|(_, _, p)| **p == BLUE)
            .map(|(x, y, _)| (x, y))
            .collect();
        let min_x_row0 = blue
            .iter()
            .filter(|(_, y)| *y < sheet.height() / 2)
            .map(|p| p.0)
            .min();
        assert_eq!(
            min_x_row0,
            Some(MARGIN + 34 + GUTTER + 1),
            "second cell, first row"
        );
        let row1: Vec<_> = blue
            .iter()
            .filter(|(_, y)| *y > sheet.height() / 2)
            .collect();
        assert_eq!(
            row1.iter().map(|p| p.0).min(),
            Some(MARGIN + 1),
            "fifth cell starts row two"
        );
        assert_eq!(
            sheet.width(),
            2 * MARGIN + 4 * 34 + 3 * GUTTER,
            "four across at most"
        );
    }

    #[test]
    fn labels_are_drawn_and_differ() {
        let cells = vec![RgbaImage::from_pixel(240, 120, RED); 2];
        let short = compose(&cells, &text(&["1", "1"]));
        let long = compose(&cells, &text(&["0.25", "0.75"]));
        let ink = |s: &RgbaImage| s.pixels().filter(|p| p[0] > 150 && p[2] > 150).count();
        assert!(ink(&long) > ink(&short));
    }

    #[test]
    fn the_legend_paints_each_style_swatch_in_the_lower_left() {
        use flint_render::style_mask::{debug_color, LegendEntry};
        let entry = |id: u16, name: &str| LegendEntry {
            id,
            name: name.into(),
            color: debug_color(id),
            color_name: "",
        };
        let mut img = RgbaImage::from_pixel(640, 360, Rgba([90, 90, 90, 255]));
        draw_legend(&mut img, &[entry(1, "ascii"), entry(2, "pencil")]);
        for id in [1u16, 2] {
            let c = debug_color(id).map(|v| (v * 255.0).round() as u8);
            let at: Vec<(u32, u32)> = img
                .enumerate_pixels()
                .filter(|(_, _, p)| p.0[..3] == c)
                .map(|(x, y, _)| (x, y))
                .collect();
            assert_eq!(at.len(), 64, "one 8x8 swatch for id {id}");
            assert!(at.iter().all(|&(x, y)| x < 320 && y > 180));
        }
        // The top right is untouched
        assert_eq!(*img.get_pixel(600, 20), Rgba([90, 90, 90, 255]));
    }

    #[test]
    fn long_labels_are_cut_to_fit() {
        assert_eq!(fit_text("ascii_noir", 8 * 5, 1), "asci~");
        assert_eq!(fit_text("6", 80, 1), "6");
    }

    #[test]
    fn the_same_sheet_comes_out_the_same() {
        let cells = vec![RgbaImage::from_pixel(64, 48, RED); 3];
        let t = text(&["a", "b", "c"]);
        assert_eq!(compose(&cells, &t), compose(&cells, &t));
    }
}
