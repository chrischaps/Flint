//! Cells, bands, dissolve order, snapping, edge regions and history: what
//! the passes keep and share (moved from `ascii.rs`, #742).

use super::*;

/// The variant cell state (#313): two textures, a texel a cell in each,
/// each texel a variant's colour sum (f32 bits in RGB) and its slot,
/// sample count and overflow bit in A.
pub(super) const VARIANT_CELL_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba32Uint;

/// Grids the variant cell state holds (#313), each a band of rows; a
/// variant on another grid samples for itself. `MAX_BANDS` in
/// ascii_common.wgsl.
pub const MAX_BANDS: usize = 8;

/// The least footprint a variant's cells are summed in the pre-pass at
/// (#313). At 1 the glyph pass's 16 loads a pixel all land on the 16
/// texels its cell shares, so they come from the texture cache, and the
/// pre-pass (a whole screen of cells a band) cost more than it saved: at
/// 4K on an RTX 4070, ASCII went 0.285 -> 0.326 ms on
/// `flint-examples/ascii_entities`. At 2 the lattice spreads over four cells' worth
/// of picture and the pre-pass wins (0.485 -> 0.405 ms close up).
pub(super) const MIN_BAND_FOOTPRINT: f32 = 2.0;

/// The least footprint summed in the pre-pass when the mask's ids have
/// screen bounds (#507): the pass then draws only the cells near them
/// ([`band_cells_in`]), and at footprint 1 one texel a pixel beats the 16
/// lattice loads.
pub(super) const MIN_BAND_FOOTPRINT_BOUNDED: f32 = 1.0;

/// The variant cell state's widest and tallest, inside the least texture
/// size every backend offers.
pub(super) const MAX_BAND_TEXELS: u32 = 8192;

/// The highest slot a band's texel can name: it packs the slot in 8 bits
/// (`slot | count << 8 | more << 16`, ascii_variant_cells.wgsl). A row past
/// it samples for itself (#507: `flint-examples/ascii_crowd`'s 500 pillars put
/// slots past 255 on the banded grids).
pub(super) const MAX_BAND_SLOT: usize = 255;

/// One grid's rows in the variant cell state (#313).
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct Band {
    pub(super) cell: [f32; 2],
    pub(super) origin: [f32; 2],
    pub(super) footprint: f32,
    /// The band's first row, its rows and its cells across.
    pub(super) row: u32,
    pub(super) rows: u32,
    pub(super) cols: u32,
}

/// Gives each drawn per-entity variant (rows past 0) with a footprint of
/// at least `min_footprint` ([`MIN_BAND_FOOTPRINT`], or
/// [`MIN_BAND_FOOTPRINT_BOUNDED`] when the pass is cut to the ids' bounds)
/// the band of its grid: variants with the same cell, origin and footprint
/// share one, so their cells are summed in one pass. Narrower variants,
/// variants past [`MAX_BANDS`] grids or [`MAX_BAND_SLOT`] and ones past the
/// texture's size get none and sample in the glyph pass.
pub(super) fn assign_bands(rows: &mut [VariantUniform], size: [u32; 2], min_footprint: f32) -> Vec<Band> {
    let mut bands: Vec<Band> = Vec::new();
    let mut next = 0u32;
    for (slot, row) in rows.iter_mut().enumerate().skip(1) {
        row.cell_row = 0;
        if row.flags & PASS_THROUGH != 0 || row.footprint < min_footprint || slot > MAX_BAND_SLOT
        {
            continue;
        }
        let same = |b: &&Band| {
            b.cell.map(f32::to_bits) == row.cell.map(f32::to_bits)
                && b.origin.map(f32::to_bits) == row.origin.map(f32::to_bits)
                && b.footprint.to_bits() == row.footprint.to_bits()
        };
        if let Some(b) = bands.iter().find(same) {
            row.cell_row = b.row + 1;
            continue;
        }
        let grid = cell_grid(size, row.cell, row.origin);
        if bands.len() == MAX_BANDS || grid[0] > MAX_BAND_TEXELS || next + grid[1] > MAX_BAND_TEXELS
        {
            continue;
        }
        bands.push(Band {
            cell: row.cell,
            origin: row.origin,
            footprint: row.footprint,
            row: next,
            rows: grid[1],
            cols: grid[0],
        });
        row.cell_row = next + 1;
        next += grid[1];
    }
    bands
}

/// Variants whose dissolve is ranked among their own cells (#398); a
/// dissolving variant past them takes its cells' order as it is.
/// `MAX_DISSOLVE` in ascii_common.wgsl.
pub const MAX_DISSOLVE: usize = 8;

/// The dissolve state's texel: a cell's order, then its rank (#398).
pub(super) const DISSOLVE_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::R32Float;

/// Gives each drawn per-entity variant (rows past 0) that dissolves a band
/// of rows of its own in the dissolve state (#398), on its own grid: the
/// rank pass counts only the variant's cells, so variants never share one.
/// Returns each band's (slot, first row, rows, columns).
pub(super) fn assign_dissolve(rows: &mut [VariantUniform], size: [u32; 2]) -> Vec<[u32; 4]> {
    let mut bands: Vec<[u32; 4]> = Vec::new();
    let mut next = 0u32;
    for (slot, row) in rows.iter_mut().enumerate().skip(1) {
        row.dissolve_row = 0;
        if row.flags & (DISSOLVE | PASS_THROUGH) != DISSOLVE || bands.len() == MAX_DISSOLVE {
            continue;
        }
        let grid = cell_grid(size, row.cell, row.origin);
        if grid[0] > MAX_BAND_TEXELS || next + grid[1] > MAX_BAND_TEXELS {
            continue;
        }
        bands.push([slot as u32, next, grid[1], grid[0]]);
        row.dissolve_row = next + 1;
        next += grid[1];
    }
    bands
}

/// The dissolve state's size for `bands`: a texel a cell across the
/// widest, every band's rows stacked, and never less than `held` (the
/// size it has): a frame without a dissolve keeps the textures, so a
/// dissolve toggled on and off doesn't remake them each time.
pub(super) fn dissolve_texture_size(bands: &[[u32; 4]], held: Option<[u32; 2]>) -> [u32; 2] {
    let wide = bands.iter().map(|b| b[3]).max().unwrap_or(0);
    let tall = bands.last().map_or(0, |b| b[1] + b[2]);
    let [w, h] = held.unwrap_or([1, 1]);
    [wide.max(w).max(1), tall.max(h).max(1)]
}

/// One bind-cache key from the generations and ids a bind group is made
/// over (FNV-1a over their bytes).
pub(super) fn bind_key(parts: &[u64]) -> u64 {
    parts
        .iter()
        .flat_map(|p| p.to_le_bytes())
        .fold(0xcbf2_9ce4_8422_2325, |k, b| {
            (k ^ u64::from(b)).wrapping_mul(0x0000_0100_0000_01b3)
        })
}

/// The variant cell state's size for `bands`: a texel a cell across the
/// widest band, every band's rows stacked, and never less than `held` (the
/// size it has). A script tweening a per-entity `cell` changes its band's
/// grid every few frames; remade at each size, the textures and the entity
/// bind group keyed by their generation piled up in the bind cache.
pub(super) fn band_texture_size(bands: &[Band], held: Option<[u32; 2]>) -> [u32; 2] {
    let wide = bands.iter().map(|b| b.cols).max().unwrap_or(0);
    let tall = bands.last().map_or(0, |b| b.row + b.rows);
    let [w, h] = held.unwrap_or([1, 1]);
    [wide.max(w).max(1), tall.max(h).max(1)]
}

/// The texels of band `b` in the variant cell state whose cells' lattice
/// can sample a pixel of `mask` (#507): every cell whose footprint, centred
/// on it, meets the rect, and one more on every side. Texel t of a row
/// holds cell t - 1 (`CELL_OFFSET`). Any other cell's 16 samples read only
/// unstyled mask texels, so the pass would write it empty.
pub(super) fn band_cells_in(b: &Band, mask: kit::PixelRect) -> kit::PixelRect {
    if mask.is_empty() {
        return kit::PixelRect::EMPTY;
    }
    let reach = |c: f32| 0.5 * c * b.footprint.max(1.0);
    let texels = |start: u32, len: u32, c: f32, o: f32, n: u32| {
        // Cell k's centre is o + (k + 0.5) c; its lattice lies within
        // `reach` of it
        let lo = ((start as f32 - reach(c) - o) / c - 0.5).floor() as i64 - 1;
        let hi = (((start + len) as f32 + reach(c) - o) / c - 0.5).ceil() as i64 + 1;
        let t0 = (lo + 1).clamp(0, n as i64);
        let t1 = (hi + 2).clamp(t0, n as i64);
        (t0 as u32, (t1 - t0) as u32)
    };
    let (x, width) = texels(mask.x, mask.width, b.cell[0], b.origin[0], b.cols);
    let (y, height) = texels(mask.y, mask.height, b.cell[1], b.origin[1], b.rows);
    kit::PixelRect {
        x,
        y: b.row + y,
        width,
        height,
    }
}

/// The tone statistics (#289): a texel per (slot, lattice row) of count,
/// sum and sum of squares, then a texel per slot of its range.
pub(super) const STATS_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba32Float;

/// The tone statistics lattice's rows; `STATS_ROWS` in ascii_common.wgsl.
pub const STATS_ROWS: u32 = 180;

/// The runs each lattice row is summed in, a texel each (#337);
/// `STATS_CHUNKS` in ascii_common.wgsl.
pub const STATS_CHUNKS: u32 = 16;

/// The `temporal` option's spellings: `off` draws every frame as if it
/// were the first.
pub const TEMPORAL_MODES: &[&str] = &["on", "off", "reproject"];

/// What a `[post_process.styles.ascii]` table covers: the whole frame, or
/// only the entities that ask (#266), the table then being their preset.
pub const SCOPES: &[&str] = &["screen", "entities"];

/// Where a variant's cells line up: one grid centred on the frame, or a
/// grid pinned to each styled root's projected origin (#266).
pub const GRIDS: &[&str] = &["screen", "object"];

/// How an entity part-way into ASCII (`blend` below 1, #278) shows it:
/// every pixel a mix of its shading and its glyph, or that share of its
/// cells turned to glyphs whole, in patches that grow as the blend rises.
pub const TRANSITIONS: &[&str] = &["fade", "dissolve"];

/// Variants one frame can hold, the whole-frame one included; any more
/// are drawn as the whole frame is (logged once). The table is a storage
/// buffer sized to the frame (#336), so this is only the tone statistics'
/// texture width (a column a variant), well inside every backend's limit.
pub const MAX_VARIANTS: usize = 4096;

/// A variant's flag: leave its pixels as they are.
pub(super) const PASS_THROUGH: u32 = 1;

/// A variant's flag: a blend below 1 turns whole cells (#278).
pub(super) const DISSOLVE: u32 = 2;

/// A variant's flag: its silhouette snaps to whole cells (#338); `SNAP` in
/// ascii.wgsl.
pub(super) const SNAP: u32 = 4;

/// How a per-entity variant's silhouette meets the scene (#338): cut to the
/// mask's pixel, or whole cells, each drawn or not by how much of it the
/// entity covers.
pub const SILHOUETTES: &[&str] = &["pixel", "cell"];

/// Grids the glyph pass asks for a snapped cell (#338); `MAX_SNAP_GRIDS`
/// in ascii_common.wgsl.
pub const MAX_SNAP_GRIDS: usize = 8;

/// The distinct grids, (cell, origin), of the drawn variants (rows past 0)
/// with [`SNAP`], first met first, at most [`MAX_SNAP_GRIDS`] (#338).
pub(super) fn snap_grids(rows: &[VariantUniform]) -> Vec<[f32; 4]> {
    let mut grids: Vec<[f32; 4]> = Vec::new();
    for row in rows.iter().skip(1) {
        if row.flags & (SNAP | PASS_THROUGH) != SNAP {
            continue;
        }
        let grid = [row.cell[0], row.cell[1], row.origin[0], row.origin[1]];
        if grids
            .iter()
            .any(|g| g.map(f32::to_bits) == grid.map(f32::to_bits))
        {
            continue;
        }
        if grids.len() == MAX_SNAP_GRIDS {
            break;
        }
        grids.push(grid);
    }
    grids
}

/// The snap owner state (#929, ascii_snap_owner.wgsl): a texel a cell of a
/// snapping grid, the slot of the snapping variant holding the cell | the
/// mask's blend at its first sample << 16; 0 for none.
pub(super) const SNAP_OWNER_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::R32Uint;

/// One snapping grid's rows in the snap owner state (#929): the cells near
/// its snapping variants' bounds, every cell any of them can hold.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct OwnerBand {
    pub(super) cell: [f32; 2],
    pub(super) origin: [f32; 2],
    /// The grid's first snapping variant: the pass reads the grid from its
    /// row, drawn with it as the instance.
    pub(super) slot: u32,
    /// The grid's cell at the band's first texel.
    pub(super) first: [i32; 2],
    /// The band's first row, its rows and its cells across.
    pub(super) row: u32,
    pub(super) rows: u32,
    pub(super) cols: u32,
}

impl OwnerBand {
    /// The band as a variant row's `owner` and the uniform's `snap_bands`
    /// hold it: (first cell x, y, first row + 1, columns | rows << 16).
    pub(super) fn packed(&self) -> [i32; 4] {
        [
            self.first[0],
            self.first[1],
            self.row as i32 + 1,
            (self.cols | (self.rows << 16)) as i32,
        ]
    }

    /// Its texels in the snap owner state.
    pub(super) fn rect(&self) -> kit::PixelRect {
        kit::PixelRect {
            x: 0,
            y: self.row,
            width: self.cols,
            height: self.rows,
        }
    }

    fn grid(&self) -> [u32; 4] {
        grid_bits(self.cell, self.origin)
    }
}

/// A grid (cell, origin) as bits, to compare as the shaders do.
fn grid_bits(cell: [f32; 2], origin: [f32; 2]) -> [u32; 4] {
    [cell[0], cell[1], origin[0], origin[1]].map(f32::to_bits)
}

/// Whether bounds (x0, y0, x1, y1) hold a pixel of a `size` frame.
fn on_screen([x0, y0, x1, y1]: [f32; 4], size: [u32; 2]) -> bool {
    x1 > x0 && y1 > y0 && x1 > 0.0 && y1 > 0.0 && x0 < size[0] as f32 && y0 < size[1] as f32
}

/// The cells of the grid (`c`, `o`) along one axis, [lo, hi], that a pixel
/// of `[x0, x1)` on a `len`-pixel frame can be in (the shader's
/// `floor((p - o) / c)` at the pixel centres), one more each side for its
/// rounding.
fn cells_along(x0: f32, x1: f32, len: u32, c: f32, o: f32) -> (i64, i64) {
    let at = |x: f32| ((x - o) / c).floor() as i64;
    (at(x0.max(0.0)) - 1, at(x1.min(len as f32)) + 1)
}

/// Gives each snapping grid (of the drawn variants with [`SNAP`]) a band of
/// the snap owner state (#929), its cells over the union of its snapping
/// variants' bounds, and each such variant's row its grid's
/// [`OwnerBand::packed`]. A grid whose band would pass [`MAX_BAND_TEXELS`]
/// gets none, and its variants' `owner` stays zeros (their pixels keep
/// their own); so does one whose variants have nothing on screen.
pub(super) fn assign_owner_bands(rows: &mut [VariantUniform], size: [u32; 2]) -> Vec<OwnerBand> {
    // The grids, first met first, each with its span of cells
    let mut grids: Vec<(OwnerBand, [i64; 4])> = Vec::new();
    for (slot, row) in rows.iter_mut().enumerate().skip(1) {
        row.owner = [0; 4];
        if row.flags & (SNAP | PASS_THROUGH) != SNAP {
            continue;
        }
        if !on_screen(row.bounds, size) {
            continue;
        }
        let [x0, y0, x1, y1] = row.bounds;
        let (cx0, cx1) = cells_along(x0, x1, size[0], row.cell[0], row.origin[0]);
        let (cy0, cy1) = cells_along(y0, y1, size[1], row.cell[1], row.origin[1]);
        let grid = grid_bits(row.cell, row.origin);
        match grids.iter_mut().find(|(b, _)| b.grid() == grid) {
            Some((_, span)) => {
                *span = [span[0].min(cx0), span[1].min(cy0), span[2].max(cx1), span[3].max(cy1)];
            }
            None => grids.push((
                OwnerBand {
                    cell: row.cell,
                    origin: row.origin,
                    slot: slot as u32,
                    first: [0, 0],
                    row: 0,
                    rows: 0,
                    cols: 0,
                },
                [cx0, cy0, cx1, cy1],
            )),
        }
    }
    let mut bands: Vec<OwnerBand> = Vec::new();
    let mut next = 0u64;
    for (mut band, [cx0, cy0, cx1, cy1]) in grids {
        let (cols, rows_tall) = ((cx1 - cx0 + 1) as u64, (cy1 - cy0 + 1) as u64);
        if cols > MAX_BAND_TEXELS as u64 || next + rows_tall > MAX_BAND_TEXELS as u64 {
            continue;
        }
        band.first = [cx0 as i32, cy0 as i32];
        band.row = next as u32;
        band.rows = rows_tall as u32;
        band.cols = cols as u32;
        next += rows_tall;
        bands.push(band);
    }
    for row in rows.iter_mut().skip(1) {
        if row.flags & (SNAP | PASS_THROUGH) != SNAP {
            continue;
        }
        let grid = grid_bits(row.cell, row.origin);
        if let Some(b) = bands.iter().find(|b| b.grid() == grid) {
            row.owner = b.packed();
        }
    }
    bands
}

/// Each of `snap_grids`' band, as the uniform's `snap_bands` holds them;
/// zeros for a grid without one.
pub(super) fn snap_bands(snapping: &[[f32; 4]], bands: &[OwnerBand]) -> [[i32; 4]; MAX_SNAP_GRIDS] {
    let mut out = [[0; 4]; MAX_SNAP_GRIDS];
    for (o, g) in out.iter_mut().zip(snapping) {
        let grid = grid_bits([g[0], g[1]], [g[2], g[3]]);
        if let Some(b) = bands.iter().find(|b| b.grid() == grid) {
            *o = b.packed();
        }
    }
    out
}

/// The snap owner state's size: every band, and never smaller than the
/// texture held, so a frame with fewer bands keeps its bind group.
pub(super) fn owner_texture_size(bands: &[OwnerBand], held: Option<[u32; 2]>) -> [u32; 2] {
    let wide = bands.iter().map(|b| b.cols).max().unwrap_or(0);
    let tall = bands.last().map_or(0, |b| b.row + b.rows);
    let [w, h] = held.unwrap_or([1, 1]);
    [wide.max(w).max(1), tall.max(h).max(1)]
}

/// Say once (#740) when a variant wanted a snap grid, a snap owner band or
/// a dissolve band and the frame had none left, naming the first such
/// variant's entity.
pub(super) fn report_left_out(
    frame: &StyleFrame<'_>,
    rows: &[VariantUniform],
    slots: &[u32],
    snapping: &[[f32; 4]],
    bands: usize,
) {
    let size = [frame.width, frame.height];
    let entity = |slot: usize| {
        slots
            .iter()
            .position(|&s| s as usize == slot)
            .and_then(|id| frame.style_ids.entity(id as u16))
            .unwrap_or("?")
            .to_string()
    };
    let drawn = |row: &VariantUniform, flag: u32| row.flags & (flag | PASS_THROUGH) == flag;
    let unsnapped = rows.iter().enumerate().skip(1).find(|(_, row)| {
        let grid = [row.cell[0], row.cell[1], row.origin[0], row.origin[1]];
        drawn(row, SNAP)
            && !snapping
                .iter()
                .any(|g| g.map(f32::to_bits) == grid.map(f32::to_bits))
    });
    if let Some((slot, _)) = unsnapped {
        report_drop(
            "ascii",
            "snap grids",
            MAX_SNAP_GRIDS,
            &entity(slot),
            "cut to whole cells of its own only: it fills no scene cells",
        );
    }
    // A snapping grid past the snap owner state's rows (#929)
    let unowned = rows
        .iter()
        .enumerate()
        .skip(1)
        .find(|(_, row)| drawn(row, SNAP) && row.owner[2] == 0 && on_screen(row.bounds, size));
    if let Some((slot, _)) = unowned {
        report_drop(
            "ascii",
            "rows of snap owner state",
            MAX_BAND_TEXELS as usize,
            &entity(slot),
            "cut to the pixel: no cell of its grid is drawn whole",
        );
    }
    let unranked = rows
        .iter()
        .enumerate()
        .skip(1)
        .find(|(_, row)| drawn(row, DISSOLVE) && row.dissolve_row == 0);
    if let Some((slot, _)) = unranked {
        let (cap, limit) = if bands == MAX_DISSOLVE {
            ("dissolve bands", MAX_DISSOLVE)
        } else {
            ("rows of dissolve state", MAX_BAND_TEXELS as usize)
        };
        report_drop(
            "ascii",
            cap,
            limit,
            &entity(slot),
            "dissolved in its cells' unranked order",
        );
    }
}

/// What the edge passes must draw for the glyph pass inside a rect (#385).
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct EdgeRegions {
    /// Texels of the edge cell texture (a texel a cell, `CELL_OFFSET` in).
    pub(super) edge_cells: kit::PixelRect,
    /// Pixels of the edge texture (the depth's size).
    pub(super) edge_pixels: kit::PixelRect,
}

/// The edge cells the glyph pass reads inside output rect `glyph` (each
/// cell under it, and more on every side: a drawn cell's corner reads
/// `EDGE_CORNER_REACH` cells away, and the rect already holds
/// `GLYPH_RECT_CELLS` of them round the entities, #930), and the edge
/// pixels the edge cell pass sums for them. The cells are `cell` pixels
/// from `origin` over a `size` frame; the edge cell texture is `grid` and
/// the edge texture `edge_size`.
pub(super) fn edge_regions(
    glyph: kit::PixelRect,
    cell: [f32; 2],
    origin: [f32; 2],
    size: [u32; 2],
    grid: [u32; 2],
    edge_size: [u32; 2],
) -> EdgeRegions {
    if glyph.is_empty() {
        return EdgeRegions {
            edge_cells: kit::PixelRect::EMPTY,
            edge_pixels: kit::PixelRect::EMPTY,
        };
    }
    // Texel t holds cell t - 1, the pixels from origin + (t - 1) * cell
    let more = (EDGE_CORNER_REACH - GLYPH_RECT_CELLS) as i64;
    let texels = |start: u32, len: u32, c: f32, o: f32, n: u32| {
        let first = ((start as f32 - o) / c).floor() as i64; // cell under the first pixel
        let last = (((start + len) as f32 - 1.0 - o) / c).floor() as i64;
        let t0 = (first + 1 - more).clamp(0, n as i64) as u32;
        let t1 = (last + 1 + more).clamp(-1, n as i64 - 1);
        (t0, (t1 + 1).max(t0 as i64) as u32 - t0)
    };
    let (tx, tw) = texels(glyph.x, glyph.width, cell[0], origin[0], grid[0]);
    let (ty, th) = texels(glyph.y, glyph.height, cell[1], origin[1], grid[1]);
    let edge_cells = kit::PixelRect {
        x: tx,
        y: ty,
        width: tw,
        height: th,
    };
    // Those cells' pixels, then the edge pixels under them
    let pixels = |t: u32, n: u32, c: f32, o: f32, len: u32| {
        let p0 = (o + (t as f32 - 1.0) * c).floor().max(0.0) as u32;
        let p1 = ((o + (t + n) as f32 * c - c).ceil().max(0.0) as u32).min(len);
        (p0.min(len), p1.saturating_sub(p0.min(len)))
    };
    let (px, pw) = pixels(tx, tw, cell[0], origin[0], size[0]);
    let (py, ph) = pixels(ty, th, cell[1], origin[1], size[1]);
    let edge_pixels = kit::PixelRect {
        x: px,
        y: py,
        width: pw,
        height: ph,
    }
    .rescaled(size, edge_size)
    .padded(1, edge_size[0], edge_size[1]);
    EdgeRegions {
        edge_cells,
        edge_pixels,
    }
}

/// The cell state texture's size for a `size` frame: every whole and part
/// cell, plus the one texel left of (above) the origin the shaders offset
/// by (`CELL_OFFSET`).
pub fn cell_grid(size: [u32; 2], cell: [f32; 2], origin: [f32; 2]) -> [u32; 2] {
    let n = |len: u32, c: f32, o: f32| ((len as f32 - o) / c).ceil().max(0.0) as u32 + 1;
    [
        n(size[0], cell[0], origin[0]),
        n(size[1], cell[1], origin[1]),
    ]
}

/// What last frame's cells must share with this frame's to be its past:
/// the same grid over the same picture size, the same ramp (a step means
/// a glyph only within one ramp) and the same history epoch.
#[derive(Clone, PartialEq, Debug)]
pub(super) struct HistoryKey {
    pub(super) epoch: u64,
    pub(super) size: [u32; 2],
    pub(super) grid: [u32; 2],
    pub(super) cell: [u32; 2],
    pub(super) origin: [u32; 2],
    pub(super) font: String,
    pub(super) ramp: String,
    pub(super) ramp_len: u32,
}

/// The two cell state textures the style ping-pongs and what it last drew.
#[derive(Default)]
pub(super) struct History {
    /// Held so a texture's address can't come back as another's while a
    /// bind group over it is cached; `generation` keys those bind groups.
    pub(super) textures: Option<[Arc<StyleTexture>; 2]>,
    /// Each cell centre's view depth beside each (#344).
    pub(super) depths: Option<[Arc<StyleTexture>; 2]>,
    pub(super) generation: u64,
    /// Which texture this frame writes.
    pub(super) parity: usize,
    /// The key and frame number of the last frame drawn.
    pub(super) last: Option<(HistoryKey, u64)>,
    /// The camera the last frame drawn was seen from (#344).
    pub(super) last_view_proj: Option<[[f32; 4]; 4]>,
    /// Whether the last frame drawn read its past, and the camera it took
    /// as last frame's: what a frame capture hands out (#1136).
    pub(super) used_past: bool,
    pub(super) used_view_proj: [[f32; 4]; 4],
}

impl HistoryKey {
    /// What a frame capture keys the tone history on (#1136): all of it
    /// but the epoch, which is the capturing renderer's.
    pub(super) fn exported(&self) -> serde_json::Value {
        serde_json::json!({
            "size": self.size,
            "grid": self.grid,
            "cell": self.cell.map(f32::from_bits),
            "origin": self.origin.map(f32::from_bits),
            "font": self.font,
            "ramp": self.ramp,
            "ramp_len": self.ramp_len,
        })
    }
}

impl EdgeKey {
    /// What a frame capture keys the edge history on (#1136): all of it
    /// but the epoch and the textures' generations, the capturing
    /// renderer's.
    pub(super) fn exported(&self) -> serde_json::Value {
        serde_json::json!({
            "size": self.size,
            "depth_size": self.depth_size,
            "cell": self.cell.map(f32::from_bits),
            "origin": self.origin.map(f32::from_bits),
        })
    }
}

impl History {
    /// Take this frame's textures; true when they're not last frame's
    /// (made again on a resize or cell change), so nothing in them is
    /// history.
    pub(super) fn adopt(&mut self, textures: [Arc<StyleTexture>; 2], depths: [Arc<StyleTexture>; 2]) -> bool {
        let both = |a: &[Arc<StyleTexture>; 2], b: &[Arc<StyleTexture>; 2]| {
            Arc::ptr_eq(&a[0], &b[0]) && Arc::ptr_eq(&a[1], &b[1])
        };
        let same = self.textures.as_ref().is_some_and(|t| both(t, &textures))
            && self.depths.as_ref().is_some_and(|d| both(d, &depths));
        if !same {
            self.textures = Some(textures);
            self.depths = Some(depths);
            self.generation += 1;
            self.parity = 0;
        }
        !same
    }

    /// Whether last frame's cells hold this frame's past, and note this
    /// frame as drawn.
    pub(super) fn advance(&mut self, key: HistoryKey, frame: u64, temporal: bool) -> bool {
        let follows = self
            .last
            .as_ref()
            .is_some_and(|(k, f)| *k == key && f.wrapping_add(1) == frame);
        self.last = Some((key, frame));
        temporal && follows
    }

    /// The camera last frame was drawn from, noting `view_proj` as this
    /// frame's; this frame's own when there was none.
    pub(super) fn swap_view_proj(&mut self, view_proj: [[f32; 4]; 4]) -> [[f32; 4]; 4] {
        self.last_view_proj.replace(view_proj).unwrap_or(view_proj)
    }
}

/// What the edge passes' history is keyed on (#477): last frame's edge
/// cells and slots are this frame's past only if the grid, the frame and
/// the edge pixels' size are unchanged and the same textures hold them.
#[derive(Clone, PartialEq, Debug)]
pub(super) struct EdgeKey {
    pub(super) epoch: u64,
    pub(super) size: [u32; 2],
    pub(super) depth_size: [u32; 2],
    pub(super) cell: [u32; 2],
    pub(super) origin: [u32; 2],
    /// The edge cells' and slots' generations, which a resize bumps.
    pub(super) textures: [u64; 2],
}

/// Last frame's edge cells and slots (#477), and the key and frame number
/// of the frame that wrote them.
#[derive(Default)]
pub(super) struct EdgePast {
    pub(super) cells: Option<Owned>,
    pub(super) slots: Option<Owned>,
    pub(super) last: Option<(EdgeKey, u64)>,
    /// Whether the last frame drawn read them (#1136).
    pub(super) used: bool,
}

impl EdgePast {
    /// Whether the frame drawn last wrote this frame's past, and note this
    /// frame as drawn. The edge passes may skip a frame (edges off, the
    /// style off) and then nothing in the textures is the last frame's.
    pub(super) fn advance(&mut self, key: EdgeKey, frame: u64, temporal: bool) -> bool {
        let follows = self
            .last
            .as_ref()
            .is_some_and(|(k, f)| *k == key && f.wrapping_add(1) == frame);
        self.last = Some((key, frame));
        temporal && follows
    }
}

/// How much of last frame's edge length and direction a cell keeps this
/// frame (#477), for the style's `smoothing` (per 60 Hz frame) and a frame
/// `dt` seconds long: its square root, so edges keep more than tone does
/// (0.6 is 0.77). A cell's edge length counts the pixels of one-pixel
/// lines, which come and go with a sub-pixel move far more than a colour
/// averaged over 16 samples does; at the tone's weight the motion bench
/// kept 1.2 reversals a frame, at this 0.9.
pub(super) fn edge_smoothing(smoothing: f32, dt: f32) -> f32 {
    super::super::per_60hz(smoothing.clamp(0.0, 0.99), dt)
        .sqrt()
        .min(SMOOTHING_MAX)
}

/// Copy `from` into `to`, two textures of one size and format.
pub(super) fn copy_whole(encoder: &mut wgpu::CommandEncoder, from: &Owned, to: &Owned) {
    encoder.copy_texture_to_texture(
        from.texture.as_image_copy(),
        to.texture.as_image_copy(),
        wgpu::Extent3d {
            width: from.size[0],
            height: from.size[1],
            depth_or_array_layers: 1,
        },
    );
}

/// The cell, in output pixels, for a cell `points` wide at
/// `pixels_per_point`, with a font whose glyphs are `glyph` pixels: whole
/// pixels, at least 2 wide, the height keeping the glyph's aspect.
pub fn cell_pixels(points: f32, pixels_per_point: f32, glyph: [u32; 2]) -> [f32; 2] {
    let ppp = if pixels_per_point.is_finite() && pixels_per_point > 0.0 {
        pixels_per_point
    } else {
        1.0
    };
    let w = (points.max(1.0) * ppp).round().max(2.0);
    let aspect = glyph[1].max(1) as f32 / glyph[0].max(1) as f32;
    [w, (w * aspect).round().max(2.0)]
}

/// Where the grid starts so that the part cells at the borders match: half
/// of what's left over on each axis.
pub fn grid_origin(size: [u32; 2], cell: [f32; 2]) -> [f32; 2] {
    let left = |n: u32, c: f32| ((n as f32) % c) * 0.5;
    [
        left(size[0], cell[0]).floor(),
        left(size[1], cell[1]).floor(),
    ]
}

/// The colour mode's shader number; an unknown name is source colour.
pub fn color_mode(name: &str) -> u32 {
    COLOR_MODES.iter().position(|m| *m == name).unwrap_or(0) as u32
}

/// The ground mode's shader number; an unknown name is `dim`.
pub fn ground_mode(name: &str) -> u32 {
    GROUND_MODES.iter().position(|m| *m == name).unwrap_or(0) as u32
}
