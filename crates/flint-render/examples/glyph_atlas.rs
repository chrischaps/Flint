//! Look at a glyph set: write its atlas, a coverage table and a lit sphere
//! drawn through each ramp, all on the CPU (#264).
//!
//! ```bash
//! cargo run -p flint-render --example glyph_atlas -- out/            # the built-in set
//! cargo run -p flint-render --example glyph_atlas -- out/ fonts/terminal.glyphs.toml
//! ```
//!
//! Writes `<set>_atlas.png` (4x, nearest), `<set>_coverage.json` (each
//! glyph's measured ink, and per ramp its ink curve: the ink each
//! luminance gets as the ramp maps it and calibrated by coverage, #281,
//! with the pairs where the first gives a brighter input less ink) and
//! `<set>_<ramp>.png` (a sphere and a floor gradient, one glyph per cell,
//! with edge glyphs on the silhouette), plus `<set>_<ramp>_calibrated.png`
//! for a ramp that isn't calibrated already. Outside the engine,
//! `flint gen fonts/terminal.glyphs.toml -o out/` draws the same specimens
//! with a labelled sheet (#284); both use `glyph_atlas::specimen`.

use flint_render::glyph_atlas::specimen::draw_specimen;
use flint_render::{GlyphRamp, GlyphSet, RampMap};
use std::path::{Path, PathBuf};

/// Luminance steps in each ramp's ink curve (#281).
const CURVE_SAMPLES: u32 = 100;

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

    let coverage: Vec<String> = atlas
        .glyphs()
        .iter()
        .map(|c| {
            format!(
                "{{\"glyph\":{},\"coverage\":{:.4}}}",
                serde_json_escape(*c),
                atlas.coverage(*c).unwrap()
            )
        })
        .collect();
    let ramps: Vec<String> = set
        .ramps()
        .map(|r| {
            let glyphs: String = r.glyphs().iter().collect();
            let cov: Vec<String> = r
                .glyphs()
                .iter()
                .map(|c| format!("{:.4}", atlas.coverage(*c).unwrap()))
                .collect();
            let calibrated = r.calibrated();
            let curve = |ramp: &GlyphRamp| {
                (0..=CURVE_SAMPLES)
                    .map(|i| {
                        let c = ramp.pick(i as f32 / CURVE_SAMPLES as f32);
                        format!("{:.4}", atlas.coverage(c).unwrap())
                    })
                    .collect::<Vec<_>>()
                    .join(",")
            };
            let luminance: Vec<String> = (0..=CURVE_SAMPLES)
                .map(|i| format!("{:.4}", i as f32 / CURVE_SAMPLES as f32))
                .collect();
            let inversions: Vec<String> = r
                .inversions()
                .iter()
                .map(|(a, b)| format!("[{},{}]", serde_json_escape(*a), serde_json_escape(*b)))
                .collect();
            let bounds = |ramp: &GlyphRamp| {
                ramp.bounds()
                    .iter()
                    .map(|b| format!("{b:.4}"))
                    .collect::<Vec<_>>()
                    .join(",")
            };
            format!(
                "{{\"name\":\"{}\",\"glyphs\":{},\"map\":\"{}\",\"coverage\":[{}],\
                 \"bounds\":[{}],\"inversions\":[{}],\"calibrated\":{{\"glyphs\":{},\
                 \"bounds\":[{}]}},\"curve\":{{\"luminance\":[{}],\"as_mapped\":[{}],\
                 \"calibrated\":[{}]}}}}",
                r.name(),
                json_string(&glyphs),
                match r.map() {
                    RampMap::Index => "index",
                    RampMap::Coverage => "coverage",
                },
                cov.join(","),
                bounds(r),
                inversions.join(","),
                json_string(&calibrated.glyphs().iter().collect::<String>()),
                bounds(&calibrated),
                luminance.join(","),
                curve(r),
                curve(&calibrated)
            )
        })
        .collect();
    std::fs::write(
        out.join(format!("{}_coverage.json", set.name())),
        format!(
            "{{\"set\":\"{}\",\"cell\":{:?},\"size\":{:?},\"glyphs\":[{}],\"ramps\":[{}]}}",
            set.name(),
            atlas.cell_size(),
            atlas.size(),
            coverage.join(","),
            ramps.join(",")
        ),
    )
    .unwrap();

    for ramp in set.ramps() {
        let specimen = draw_specimen(&set, ramp, 2);
        specimen
            .save(out.join(format!("{}_{}.png", set.name(), ramp.name())))
            .unwrap();
        if ramp.map() == RampMap::Index {
            draw_specimen(&set, &ramp.calibrated(), 2)
                .save(out.join(format!("{}_{}_calibrated.png", set.name(), ramp.name())))
                .unwrap();
        }
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

fn json_string(s: &str) -> String {
    let mut out = String::from("\"");
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

fn serde_json_escape(c: char) -> String {
    json_string(&c.to_string())
}
