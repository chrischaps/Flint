//! Look at a glyph set: write its atlas, a coverage table and a lit sphere
//! drawn through each ramp, all on the CPU (#264).
//!
//! ```bash
//! cargo run -p flint-render --example glyph_atlas -- out/            # the built-in set
//! cargo run -p flint-render --example glyph_atlas -- out/ fonts/terminal.glyphs.toml
//! ```
//!
//! Writes `<set>_atlas.png` (4x, nearest), `<set>_coverage.json` (each
//! glyph's measured ink) and `<set>_<ramp>.png` (a sphere and a floor
//! gradient, one glyph per cell, with edge glyphs on the silhouette).
//! Outside the engine, `flint gen fonts/terminal.glyphs.toml -o out/` writes
//! the same files plus a labelled sheet (#284).

use flint_render::glyph_atlas::specimen::{draw_specimen, is_rising, ramp_coverage};
use flint_render::GlyphSet;
use std::path::{Path, PathBuf};

fn main() {
    let mut args = std::env::args().skip(1);
    let out = PathBuf::from(args.next().unwrap_or_else(|| "glyph_out".into()));
    std::fs::create_dir_all(&out).expect("create the output folder");
    let set = match args.next() {
        Some(path) => GlyphSet::load(Path::new(&path)).unwrap_or_else(|e| panic!("{e}")),
        None => (*GlyphSet::builtin()).clone(),
    };
    let atlas = set.atlas();

    let img = atlas.to_image();
    let big = image::imageops::resize(
        &img,
        img.width() * 4,
        img.height() * 4,
        image::imageops::FilterType::Nearest,
    );
    big.save(out.join(format!("{}_atlas.png", set.name())))
        .unwrap();
    std::fs::write(
        out.join(format!("{}_coverage.json", set.name())),
        coverage_json(&set).to_string(),
    )
    .unwrap();
    for ramp in set.ramps() {
        draw_specimen(&set, ramp, 2)
            .save(out.join(format!("{}_{}.png", set.name(), ramp.name())))
            .unwrap();
    }
    println!(
        "{}: {} glyphs in {}x{} cells, atlas {}x{}, ramps {}",
        set.name(),
        atlas.glyphs().len(),
        atlas.cell_size()[0],
        atlas.cell_size()[1],
        atlas.size()[0],
        atlas.size()[1],
        set.ramps().map(|r| r.name()).collect::<Vec<_>>().join(", ")
    );
}

/// Each glyph's ink and each ramp's (`flint gen` writes the same report).
fn coverage_json(set: &GlyphSet) -> serde_json::Value {
    let atlas = set.atlas();
    let glyphs: Vec<_> = atlas
        .glyphs()
        .iter()
        .map(|c| serde_json::json!({"glyph": c.to_string(), "coverage": atlas.coverage(*c)}))
        .collect();
    let ramps: Vec<_> = set
        .ramps()
        .map(|r| {
            let cov = ramp_coverage(set, r);
            serde_json::json!({
                "name": r.name(),
                "glyphs": r.glyphs().iter().collect::<String>(),
                "rising": is_rising(&cov),
                "coverage": cov,
            })
        })
        .collect();
    serde_json::json!({
        "set": set.name(),
        "cell": atlas.cell_size(),
        "size": atlas.size(),
        "glyphs": glyphs,
        "ramps": ramps,
    })
}
