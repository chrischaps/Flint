use super::*;
use flint_core::bundle::{Bundle, BundleManifest};
use flint_core::project::AssetScope;
use flint_core::vfs::{self, MemorySource};
use std::path::Path;
use std::sync::Arc;

const HACK: &[u8] = epaint_default_fonts::HACK_REGULAR;

#[test]
fn builtin_atlas_holds_printable_ascii_and_the_shade_blocks() {
    let atlas = GlyphAtlas::builtin();
    assert_eq!(atlas.cell_size(), [8, 8]);
    for c in printable_ascii().chain("─│╱╲░▒▓█".chars()) {
        assert!(atlas.contains(c), "built-in atlas lacks {c:?}");
    }
    // ASCII, edges + shades, corners and tees, quadrant blocks, braille.
    assert_eq!(atlas.glyphs().len(), 95 + 8 + 9 + 14 + 256);
    // 16 columns of 8 px cells with a 1 px gutter.
    assert_eq!(atlas.size(), [1 + 16 * 9, 1 + 24 * 9]);
    // The new groups go after the old, so every older glyph keeps its cell.
    assert_eq!(atlas.index_of('█'), Some(95 + 8 + 9 - 1));
}

#[test]
fn builtin_corners_and_tees_meet_the_edge_strokes() {
    let atlas = GlyphAtlas::builtin();
    // Which of a glyph's arms (left, right, up, down) leave the cell, on
    // the row and column the edge glyphs `─` and `│` use.
    let arms = |c: char| {
        let ink = |x, y| atlas.sample(c, x, y) == Some(255);
        [ink(0, 3), ink(7, 3), ink(3, 0), ink(3, 7)]
    };
    let cases = [
        ('┌', [false, true, false, true]),
        ('┐', [true, false, false, true]),
        ('└', [false, true, true, false]),
        ('┘', [true, false, true, false]),
        ('├', [false, true, true, true]),
        ('┤', [true, false, true, true]),
        ('┬', [true, true, false, true]),
        ('┴', [true, true, true, false]),
        ('┼', [true, true, true, true]),
    ];
    for (c, want) in cases {
        assert_eq!(arms(c), want, "{c:?} arms (left, right, up, down)");
        // Every arm reaches the centre pixel, so the turn has no gap.
        assert_eq!(atlas.sample(c, 3, 3), Some(255), "{c:?} centre");
        // One pixel wide: nothing but its arms is inked.
        // The centre is (3, 3): left and up arms are 3 px, right and down 4.
        let arm_px = 1 + [3, 4, 3, 4]
            .iter()
            .zip(want)
            .filter(|(_, on)| *on)
            .map(|(n, _)| n)
            .sum::<usize>();
        let inked = (0..8)
            .flat_map(|y| (0..8).map(move |x| (x, y)))
            .filter(|&(x, y)| atlas.sample(c, x, y) == Some(255))
            .count();
        assert_eq!(inked, arm_px, "{c:?} stroke width");
    }
    assert_eq!(arms('─'), [true, true, false, false]);
    assert_eq!(arms('│'), [false, false, true, true]);
}

#[test]
fn builtin_quadrant_blocks_ink_their_quadrants() {
    let atlas = GlyphAtlas::builtin();
    let quadrant_ink = |c: char, q: u32| {
        let (x0, y0) = ((q % 2) * 4, (q / 2) * 4);
        let all = (0..4).all(|dy| (0..4).all(|dx| atlas.sample(c, x0 + dx, y0 + dy) == Some(255)));
        let none = (0..4).all(|dy| (0..4).all(|dx| atlas.sample(c, x0 + dx, y0 + dy) == Some(0)));
        assert!(all || none, "{c:?} quadrant {q} is half inked");
        all
    };
    for (c, bits) in QUADRANT_BLOCKS {
        for q in 0..4 {
            assert_eq!(
                quadrant_ink(c, q),
                bits & (1 << q) != 0,
                "{c:?} quadrant {q}"
            );
        }
        assert_eq!(atlas.coverage(c), Some(bits.count_ones() as f32 / 4.0));
    }
    // With space and `█`, the fourteen are every one of the 16 patterns.
    let mut seen: Vec<u8> = QUADRANT_BLOCKS.iter().map(|(_, b)| *b).collect();
    seen.extend([0, 15]);
    seen.sort();
    assert_eq!(seen, (0..16).collect::<Vec<u8>>());
    // The halves are what their names say.
    assert_eq!(atlas.sample('▀', 7, 3), Some(255));
    assert_eq!(atlas.sample('▀', 0, 4), Some(0));
    assert_eq!(atlas.sample('▐', 4, 7), Some(255));
    assert_eq!(atlas.sample('▐', 3, 0), Some(0));
}

#[test]
fn builtin_braille_glyphs_are_their_dot_patterns() {
    let atlas = GlyphAtlas::builtin();
    assert_eq!(braille_char(0), '⠀');
    assert_eq!(braille_char(0xff), '⣿');
    assert_eq!(braille_char(braille_dot(0, 0)), '⠁');
    assert_eq!(braille_char(braille_dot(0, 3)), '⡀');
    assert_eq!(braille_char(braille_dot(1, 3)), '⢀');
    for dots in 0..=255u8 {
        let c = braille_char(dots);
        assert!(atlas.contains(c), "{c:?}");
        for column in 0..2 {
            for row in 0..4 {
                let on = dots & braille_dot(column, row) != 0;
                // A dot is the 2x1 px at (1 + 4 column, 2 row).
                let (x, y) = (1 + column * 4, row * 2);
                let want = Some(if on { 255 } else { 0 });
                assert_eq!(atlas.sample(c, x, y), want, "{c:?} dot ({column}, {row})");
                assert_eq!(
                    atlas.sample(c, x + 1, y),
                    want,
                    "{c:?} dot ({column}, {row})"
                );
                // The row and column round each dot stay paper.
                assert_eq!(atlas.sample(c, x, y + 1), Some(0));
                assert_eq!(atlas.sample(c, x - 1, y), Some(0));
            }
        }
        assert_eq!(atlas.coverage(c), Some(dots.count_ones() as f32 / 32.0));
    }
}

#[test]
fn the_sub_cell_ramps_add_one_piece_a_step() {
    let set = GlyphSet::builtin();
    let atlas = set.atlas();
    let ink = |c: char| -> Vec<bool> {
        (0..64)
            .map(|i| atlas.sample(c, i % 8, i / 8) == Some(255))
            .collect()
    };
    for (name, steps, piece) in [("quadrants", 5, 16), ("braille", 9, 2)] {
        let ramp = set.ramp(name).unwrap();
        assert_eq!(ramp.glyphs().len(), steps, "{name}");
        assert_eq!(ramp.glyphs()[0], ' ');
        for pair in ramp.glyphs().windows(2) {
            let (a, b) = (ink(pair[0]), ink(pair[1]));
            // Each step keeps every inked pixel and adds exactly one piece.
            assert!(
                a.iter().zip(&b).all(|(a, b)| !a || *b),
                "{name}: {pair:?} drops ink"
            );
            let added = a.iter().zip(&b).filter(|(a, b)| !**a && **b).count();
            assert_eq!(added, piece, "{name}: {pair:?}");
        }
    }
    assert_eq!(
        *set.ramp("quadrants").unwrap().glyphs().last().unwrap(),
        '█'
    );
    assert_eq!(*set.ramp("braille").unwrap().glyphs().last().unwrap(), '⣿');
}

#[test]
fn every_builtin_ramp_glyph_has_a_cell() {
    let set = GlyphSet::builtin();
    let atlas = set.atlas();
    let mut ramps = 0;
    for ramp in set.ramps() {
        ramps += 1;
        assert!(!ramp.is_empty());
        for (c, i) in ramp.glyphs().iter().zip(ramp.atlas_indices()) {
            assert_eq!(atlas.index_of(*c), Some(*i), "ramp {} {c:?}", ramp.name());
            let uv = atlas.uv_rect(*c).unwrap();
            assert!(uv[0] < uv[2] && uv[1] < uv[3] && uv[2] <= 1.0 && uv[3] <= 1.0);
        }
    }
    assert_eq!(ramps, 6);
    for c in set.edges().all() {
        assert!(atlas.contains(c), "edge glyph {c:?}");
    }
    assert_eq!(set.default_ramp().name(), "classic");
}

#[test]
fn the_shipped_ramps_run_from_paper_to_ink() {
    let set = GlyphSet::builtin();
    let atlas = set.atlas();
    for name in ["blocks", "detailed"] {
        let ramp = set.ramp(name).unwrap();
        let cov: Vec<f32> = ramp
            .glyphs()
            .iter()
            .map(|c| atlas.coverage(*c).unwrap())
            .collect();
        assert!(
            cov.windows(2).all(|w| w[0] <= w[1]),
            "{name} is not monotone: {cov:?}"
        );
    }
    let blocks = set.ramp("blocks").unwrap();
    let cov: Vec<f32> = blocks
        .glyphs()
        .iter()
        .map(|c| atlas.coverage(*c).unwrap())
        .collect();
    assert_eq!(cov, vec![0.0, 0.25, 0.5, 0.75, 1.0]);
    // The classic ramp, as written, starts blank and ends densest of its first half.
    let classic = set.ramp("classic").unwrap();
    assert_eq!(classic.glyphs()[0], ' ');
    assert_eq!(atlas.coverage(' '), Some(0.0));
}

#[test]
fn ramp_pick_maps_luminance_into_equal_bands() {
    let set = GlyphSet::builtin();
    let ramp = set.ramp("classic").unwrap();
    assert_eq!(ramp.pick(0.0), ' ');
    assert_eq!(ramp.pick(1.0), '@');
    assert_eq!(ramp.pick(0.999), '@');
    assert_eq!(ramp.pick(0.15), '.');
    assert_eq!(ramp.pick(-3.0), ' ');
    assert_eq!(ramp.pick(f32::NAN), ' ');
    assert_eq!(ramp.pick(7.0), '@');
}

/// The ink each of `ramp`'s picks gives across 0..1 in `samples` steps.
fn ink_curve(ramp: &GlyphRamp, atlas: &GlyphAtlas, samples: usize) -> Vec<(f32, char, f32)> {
    (0..=samples)
        .map(|i| {
            let l = i as f32 / samples as f32;
            let c = ramp.pick(l);
            (l, c, atlas.coverage(c).unwrap())
        })
        .collect()
}

#[test]
fn the_classic_ramp_as_written_gives_brighter_inputs_less_ink() {
    // #281's finding, kept as a test so a redrawn font that fixes it says so
    let set = GlyphSet::builtin();
    let classic = set.ramp("classic").unwrap();
    assert_eq!(classic.map(), RampMap::Index);
    assert_eq!(
        classic.inversions(),
        vec![(':', '-'), ('=', '+'), ('#', '%')]
    );
    let curve = ink_curve(classic, set.atlas(), 1000);
    assert!(curve.windows(2).any(|w| w[1].2 < w[0].2));
}

#[test]
fn a_calibrated_classic_ramp_never_gives_a_brighter_input_less_ink() {
    // #281's done-when, in the built-in 8x8 font and in Hack at 16 px
    let hack = GlyphAtlas::from_ttf(HACK, 16.0, printable_ascii()).unwrap();
    let builtin = GlyphAtlas::builtin();
    for atlas in [&*builtin, &hack] {
        let ramp = GlyphRamp::mapped(
            "classic",
            " .:-=+*#%@",
            RampOrder::AsWritten,
            RampMap::Coverage,
            atlas,
        )
        .unwrap();
        assert_eq!(ramp.map(), RampMap::Coverage);
        assert!(ramp.inversions().is_empty());
        assert_eq!(ramp.len(), 10, "every glyph kept");
        let curve = ink_curve(&ramp, atlas, 10_000);
        for w in curve.windows(2) {
            assert!(
                w[1].2 >= w[0].2,
                "{:?} at {} inks less than {:?} at {}",
                w[1].1,
                w[1].0,
                w[0].1,
                w[0].0
            );
        }
        // The ends stay paper and the most ink
        assert_eq!(ramp.pick(0.0), ' ');
        let most = ramp.coverage().iter().copied().fold(0.0, f32::max);
        assert_eq!(atlas.coverage(ramp.pick(1.0)), Some(most));
        // Nearest measured ink: each glyph is picked where its normalised
        // coverage lies
        let (lo, hi) = (ramp.coverage()[0], most);
        for (c, cov) in ramp.glyphs().iter().zip(ramp.coverage()) {
            let at = (cov - lo) / (hi - lo);
            assert_eq!(atlas.coverage(ramp.pick(at)), Some(*cov), "{c:?} at {at}");
        }
    }
    // In the 8x8 font `#` (.313) out-inks `@` (.281): it becomes the top step
    let ramp = GlyphSet::builtin().ramp("classic").unwrap().calibrated();
    assert_eq!(ramp.glyphs().iter().collect::<String>(), " .-:+=*%@#");
    assert_eq!(ramp.pick(1.0), '#');
}

#[test]
fn calibrated_bands_start_at_the_midpoints_between_neighbours() {
    let b = calibrated_bounds(&[0.0, 0.1, 0.3, 0.5]);
    let want = [0.0, 0.1, 0.4, 0.8];
    for (got, want) in b.iter().zip(want) {
        assert!((got - want).abs() < 1e-6, "{b:?}");
    }
    // No range at all: equal bands
    assert_eq!(calibrated_bounds(&[0.2, 0.2]), vec![0.0, 0.5]);
    assert!(calibrated_bounds(&[]).is_empty());
    // Ties keep a step that is never picked rather than a step backwards
    let tied = calibrated_bounds(&[0.0, 0.5, 0.5, 1.0]);
    assert_eq!(tied, vec![0.0, 0.25, 0.5, 0.75]);
}

#[test]
fn a_ramp_maps_by_coverage_when_its_toml_says_so() {
    let toml = "[ramps.classic]\nglyphs = \" .:-=+*#%@\"\nmap = \"coverage\"\n\
                [ramps.plain]\nglyphs = \" .:-=+*#%@\"\n";
    let set = GlyphSet::from_toml("cal", toml, Path::new("")).unwrap();
    let classic = set.ramp("classic").unwrap();
    assert_eq!(classic.map(), RampMap::Coverage);
    let plain = set.ramp("plain").unwrap().calibrated();
    assert_eq!(classic.glyphs(), plain.glyphs());
    assert_eq!(classic.bounds(), plain.bounds());
    assert_eq!(set.ramp("plain").unwrap().map(), RampMap::Index);
    assert!(GlyphSet::from_toml(
        "x",
        "[ramps.a]\nglyphs = \" .\"\nmap = \"ink\"\n",
        Path::new("")
    )
    .is_err());
}

#[test]
fn edge_glyphs_follow_the_edge_direction() {
    let e = EdgeGlyphs::default();
    use std::f32::consts::PI;
    assert_eq!(e.for_angle(0.0), '-');
    assert_eq!(e.for_angle(PI), '-');
    assert_eq!(e.for_angle(PI / 4.0), '/');
    assert_eq!(e.for_angle(PI / 2.0), '|');
    assert_eq!(e.for_angle(-PI / 2.0), '|');
    assert_eq!(e.for_angle(3.0 * PI / 4.0), '\\');
    assert_eq!(e.for_angle(-PI / 4.0), '\\');
}

#[test]
fn corners_are_picked_by_the_arms_that_meet() {
    let set = GlyphSet::builtin();
    let e = set.edges();
    assert_eq!(e.corner_count(), 9);
    assert_eq!(
        e.slots().iter().collect::<String>(),
        "─│╱╲┌┐└┘├┤┬┴┼",
        "lines, then corners, tees and the cross: the shader's slot order"
    );
    // (right, left, down, up), as ascii_edge_corners.wgsl's ARM_GLYPHS
    assert_eq!(e.for_arms(true, false, true, false), Some('┌'));
    assert_eq!(e.for_arms(false, true, true, false), Some('┐'));
    assert_eq!(e.for_arms(true, false, false, true), Some('└'));
    assert_eq!(e.for_arms(false, true, false, true), Some('┘'));
    assert_eq!(e.for_arms(true, false, true, true), Some('├'));
    assert_eq!(e.for_arms(false, true, true, true), Some('┤'));
    assert_eq!(e.for_arms(true, true, true, false), Some('┬'));
    assert_eq!(e.for_arms(true, true, false, true), Some('┴'));
    assert_eq!(e.for_arms(true, true, true, true), Some('┼'));
    assert_eq!(e.for_arms(true, true, false, false), Some('─'));
    assert_eq!(e.for_arms(false, false, true, true), Some('│'));
    assert_eq!(e.for_arms(true, false, false, false), None);
    assert_eq!(e.for_arms(false, false, false, false), None);
    // A corner's arms meet a line's stroke: `┌` inks `─`'s row to its
    // right and `│`'s column below it, so an outline closes
    let atlas = set.atlas();
    assert_eq!(atlas.sample('┌', 7, 3), atlas.sample('─', 7, 3));
    assert_eq!(atlas.sample('┌', 3, 7), atlas.sample('│', 3, 7));
    assert_eq!(atlas.sample('┌', 0, 3), Some(0));
}

#[test]
fn a_set_names_four_corners_nine_or_none() {
    let none = GlyphSet::from_toml("x", "[ramps.a]\nglyphs = \" .\"\n", Path::new("")).unwrap();
    assert_eq!(none.edges().corner_count(), 0);
    assert_eq!(none.edges().slots().len(), 4);
    assert_eq!(
        none.edges().for_arms(true, false, true, false),
        None,
        "no corner glyphs: the cell keeps its line"
    );
    let four = GlyphSet::from_toml(
        "x",
        "[ramps.a]\nglyphs = \" .\"\n[edges]\ncorners = \"++++\"\n",
        Path::new(""),
    )
    .unwrap();
    assert_eq!(four.edges().corner_count(), 4);
    assert_eq!(four.edges().for_arms(true, false, true, false), Some('+'));
    assert_eq!(
        four.edges().for_arms(true, true, true, false),
        None,
        "no tees"
    );
    let err = GlyphSet::from_toml(
        "x",
        "[ramps.a]\nglyphs = \" .\"\n[edges]\ncorners = \"+++\"\n",
        Path::new(""),
    )
    .unwrap_err();
    assert!(err.to_string().contains("corners"), "{err}");
}

#[test]
fn builtin_edge_glyphs_tile_into_unbroken_lines() {
    let atlas = GlyphAtlas::builtin();
    // `│` inks the same column top to bottom; `─` the same row edge to edge.
    for y in 0..8 {
        assert_eq!(atlas.sample('│', 3, y), Some(255));
    }
    for x in 0..8 {
        assert_eq!(atlas.sample('─', x, 3), Some(255));
    }
    // `╱` leaves its cell at the top-right corner and `╲` at the bottom-right.
    assert_eq!(atlas.sample('╱', 7, 0), Some(255));
    assert_eq!(atlas.sample('╱', 0, 7), Some(255));
    assert_eq!(atlas.sample('╲', 7, 7), Some(255));
    // The ASCII `-` stays an ordinary glyph, short of its cell's edges.
    assert_eq!(atlas.sample('-', 0, 3), Some(0));
    assert_eq!(GlyphSet::builtin().edges().all(), ['─', '│', '╱', '╲']);
}

#[test]
fn gutters_stay_paper() {
    let atlas = GlyphAtlas::builtin();
    let [w, h] = atlas.size();
    let px = atlas.pixels();
    for i in 0..atlas.glyphs().len() as u32 {
        let [x, y] = atlas.cell_origin(i);
        assert_eq!(px[((y - 1) * w + x) as usize], 0, "gutter above cell {i}");
        assert_eq!(px[(y * w + x - 1) as usize], 0, "gutter left of cell {i}");
    }
    assert!(px[..w as usize].iter().all(|v| *v == 0));
    assert!(px[((h - 1) * w) as usize..].iter().all(|v| *v == 0));
}

#[test]
fn pixel_text_rejects_what_it_cannot_draw() {
    let bad_row = "== a\n..x..\n";
    assert!(matches!(
        GlyphAtlas::from_pixel_text(bad_row, [8, 8]),
        Err(GlyphError::Parse(_))
    ));
    let short = "== a\n.....\n";
    assert!(matches!(
        GlyphAtlas::from_pixel_text(short, [8, 8]),
        Err(GlyphError::Parse(_))
    ));
    let two = "== ab\n";
    assert!(GlyphAtlas::from_pixel_text(two, [8, 8]).is_err());
}

#[test]
fn ttf_rasterizes_a_monospaced_cell_for_every_glyph() {
    let atlas = GlyphAtlas::from_ttf(HACK, 16.0, printable_ascii()).unwrap();
    let [cw, ch] = atlas.cell_size();
    assert!(
        (8..=11).contains(&cw),
        "Hack at 16 px is ~10 px wide, got {cw}"
    );
    assert!((16..=20).contains(&ch), "line height {ch}");
    assert_eq!(atlas.glyphs().len(), 95);
    assert_eq!(atlas.coverage(' '), Some(0.0));
    let at = atlas.coverage('@').unwrap();
    let dot = atlas.coverage('.').unwrap();
    assert!(at > dot * 3.0, "'@' {at} should out-ink '.' {dot}");
    // Ink stays inside its cell: nothing in any gutter.
    let [w, _] = atlas.size();
    for i in 0..atlas.glyphs().len() as u32 {
        let [x, y] = atlas.cell_origin(i);
        for dx in 0..cw {
            assert_eq!(atlas.pixels()[((y - 1) * w + x + dx) as usize], 0);
        }
    }
}

#[test]
fn ttf_names_the_glyphs_it_lacks() {
    // Hack has no CJK.
    let err = GlyphAtlas::from_ttf(HACK, 16.0, "ab漢字".chars()).unwrap_err();
    assert_eq!(err, GlyphError::Missing(vec!['字', '漢']));
    assert!(err.to_string().contains("'漢'"));
    assert!(matches!(
        GlyphAtlas::from_ttf(b"not a font", 16.0, "a".chars()),
        Err(GlyphError::Font(_))
    ));
    assert!(GlyphAtlas::from_ttf(HACK, 1.0, "a".chars()).is_err());
}

const TERMINAL: &str = r#"
default_ramp = "rain"

[font]
file = "Hack-Regular.ttf"
size = 12

[ramps.rain]
glyphs = " .|!lI"

[ramps.sorted]
glyphs = "@#. "
sort = "coverage"

[edges]
horizontal = "~"
"#;

fn game_files() -> MemorySource {
    let mut src = MemorySource::new();
    src.insert("game/fonts/terminal.glyphs.toml", TERMINAL);
    src.insert("game/fonts/Hack-Regular.ttf", HACK);
    src.insert(
        "game/fonts/plain.glyphs.toml",
        "[ramps.dots]\nglyphs = \" .:\"\n",
    );
    src
}

#[test]
fn a_custom_font_and_ramp_are_named_in_toml() {
    vfs::scoped(Arc::new(game_files()), || {
        let set = GlyphSet::load(Path::new("game/fonts/terminal.glyphs.toml")).unwrap();
        assert_eq!(set.name(), "terminal");
        assert_eq!(set.default_ramp().name(), "rain");
        assert_eq!(
            set.ramp("rain").unwrap().glyphs(),
            &[' ', '.', '|', '!', 'l', 'I']
        );
        assert_eq!(set.ramp("sorted").unwrap().glyphs(), &[' ', '.', '#', '@']);
        assert_eq!(set.edges().horizontal, '~');
        assert_eq!(set.edges().vertical, '|');
        // A 12 px TTF cell, not the built-in 8x8.
        assert_ne!(set.atlas().cell_size(), [8, 8]);
        for c in printable_ascii() {
            assert!(set.atlas().contains(c));
        }

        // The built-in font with a ramp of its own.
        let plain = GlyphSet::load(Path::new("game/fonts/plain.glyphs.toml")).unwrap();
        assert_eq!(plain.atlas().cell_size(), [8, 8]);
        assert_eq!(plain.default_ramp().glyphs(), &[' ', '.', ':']);
    });
}

#[test]
fn a_set_without_ramps_inherits_the_builtin_ones_its_font_draws() {
    let mut src = MemorySource::new();
    src.insert(
        "f/hack.glyphs.toml",
        "[font]\nfile = \"Hack-Regular.ttf\"\n",
    );
    src.insert("f/Hack-Regular.ttf", HACK);
    vfs::scoped(Arc::new(src), || {
        let set = GlyphSet::load(Path::new("f/hack.glyphs.toml")).unwrap();
        assert!(set.ramp("classic").is_some());
        assert!(set.ramp("detailed").is_some());
        assert_eq!(set.default_ramp().name(), "classic");
        // Hack draws the shade blocks too.
        assert!(set.ramp("blocks").is_some());
    });
}

#[test]
fn bad_sets_say_what_is_wrong() {
    let builtin_dir = Path::new("");
    let missing = GlyphSet::from_toml("x", "[ramps.a]\nglyphs = \" é\"\n", builtin_dir);
    assert_eq!(missing.unwrap_err(), GlyphError::Missing(vec!['é']));
    let unknown_key = GlyphSet::from_toml("x", "[ramps.a]\nglyph = \" .\"\n", builtin_dir);
    assert!(matches!(unknown_key, Err(GlyphError::Parse(_))));
    let bad_default = GlyphSet::from_toml(
        "x",
        "default_ramp = \"nope\"\n[ramps.a]\nglyphs = \" .\"\n",
        builtin_dir,
    );
    assert!(bad_default.unwrap_err().to_string().contains("nope"));
    let two_char_edge = GlyphSet::from_toml("x", "[edges]\nvertical = \"||\"\n", builtin_dir);
    assert!(two_char_edge.is_err());
    let no_file = vfs::scoped(Arc::new(MemorySource::new()), || {
        GlyphSet::from_toml("x", "[font]\nfile = \"gone.ttf\"\n", Path::new("fonts"))
    });
    assert!(matches!(no_file, Err(GlyphError::Read(_))));
}

#[test]
fn a_grid_image_cuts_into_cells() {
    // Two 4x4 cells side by side: 'A' solid white, 'B' its left half.
    let mut img = image::RgbaImage::new(8, 4);
    for y in 0..4 {
        for x in 0..4 {
            img.put_pixel(x, y, image::Rgba([255, 255, 255, 255]));
        }
        for x in 4..6 {
            img.put_pixel(x, y, image::Rgba([255, 255, 255, 255]));
        }
    }
    let dynamic = image::DynamicImage::ImageRgba8(img);
    let atlas = GlyphAtlas::from_grid_image(&dynamic, [4, 4], "AB".chars(), false).unwrap();
    assert_eq!(atlas.coverage('A'), Some(1.0));
    assert_eq!(atlas.coverage('B'), Some(0.5));
    let inverted = GlyphAtlas::from_grid_image(&dynamic, [4, 4], "AB".chars(), true).unwrap();
    assert_eq!(inverted.coverage('A'), Some(0.0));
    assert!(GlyphAtlas::from_grid_image(&dynamic, [4, 4], "ABC".chars(), false).is_err());
}

#[test]
fn a_grid_png_loads_from_toml() {
    let mut img = image::GrayImage::new(16, 8);
    for x in 0..8 {
        img.put_pixel(x + 8, 4, image::Luma([255]));
    }
    let mut png = Vec::new();
    image::DynamicImage::ImageLuma8(img)
        .write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
        .unwrap();
    let mut src = MemorySource::new();
    src.insert("fonts/grid.png", png);
    let toml = "[font]\nfile = \"grid.png\"\ncell = [8, 8]\nchars = \" -\"\n\
                [ramps.r]\nglyphs = \" -\"\n[edges]\nvertical = \"-\"\ndiagonal = \"-\"\nantidiagonal = \"-\"\n";
    let set = vfs::scoped(Arc::new(src), || {
        GlyphSet::from_toml("grid", toml, Path::new("fonts"))
    })
    .unwrap();
    assert_eq!(set.atlas().coverage('-'), Some(1.0 / 8.0));
    assert_eq!(set.atlas().sample('-', 3, 4), Some(255));
}

#[test]
fn a_set_can_be_drawn_by_hand_in_pixel_text() {
    let mut src = MemorySource::new();
    src.insert(
        "fonts/dots.txt",
        "== space\n...\n...\n...\n...\n== o\n...\n.#.\n...\n...\n== -\n...\n###\n...\n...\n\
         == |\n.#.\n.#.\n.#.\n.#.\n",
    );
    let toml = "[font]\nfile = \"dots.txt\"\ncell = [3, 4]\n[ramps.r]\nglyphs = \" o\"\n\
                [edges]\ndiagonal = \"|\"\nantidiagonal = \"|\"\n";
    let set = vfs::scoped(Arc::new(src.clone()), || {
        GlyphSet::from_toml("dots", toml, Path::new("fonts"))
    })
    .unwrap();
    assert_eq!(set.atlas().cell_size(), [3, 4]);
    assert_eq!(set.atlas().sample('o', 1, 1), Some(255));
    assert_eq!(set.ramp("r").unwrap().glyphs(), &[' ', 'o']);
    let no_cell = vfs::scoped(Arc::new(src), || {
        GlyphSet::from_toml("dots", "[font]\nfile = \"dots.txt\"\n", Path::new("fonts"))
    });
    assert!(no_cell.unwrap_err().to_string().contains("cell"));
}

#[test]
fn atlases_stack_top_to_bottom_without_touching() {
    let a = GlyphAtlas::builtin();
    let b = GlyphAtlas::from_pixel_text("== x\n#.#\n.#.\n#.#\n", [3, 3]).unwrap();
    let (pixels, size, tops) = stack_pixels(&[&a, &b]);
    assert_eq!(tops, vec![0, a.size()[1]]);
    assert_eq!(size, [a.size()[0], a.size()[1] + b.size()[1]]);
    // Each atlas's pixels where its top says, the narrower one padded
    let at = |x: u32, y: u32| pixels[(y * size[0] + x) as usize];
    for y in 0..a.size()[1] {
        for x in 0..a.size()[0] {
            assert_eq!(at(x, y), a.pixels()[(y * a.size()[0] + x) as usize]);
        }
    }
    let [ox, oy] = b.cell_origin(0);
    assert_eq!(at(ox, tops[1] + oy), 255, "x's top-left ink");
    assert_eq!(at(ox + 1, tops[1] + oy), 0);
    assert_eq!(at(b.size()[0], tops[1] + oy), 0, "past b's width is empty");
    // The gutters between them are empty, so no glyph reads its neighbour
    assert!((0..size[0]).all(|x| at(x, tops[1]) == 0 && at(x, tops[1] - 1) == 0));
}

#[test]
fn the_cache_finds_sets_by_name_and_remembers_failures() {
    vfs::scoped(Arc::new(game_files()), || {
        let mut cache = GlyphSetCache::for_scene(Path::new("game/scenes/level.scene.toml"));
        let a = cache.get("terminal").unwrap();
        let b = cache.get("terminal").unwrap();
        assert!(Arc::ptr_eq(&a, &b), "loaded once");
        let by_path = cache.get("fonts/terminal.glyphs.toml").unwrap();
        assert_eq!(by_path.name(), "terminal");
        assert!(Arc::ptr_eq(
            &cache.get("builtin").unwrap(),
            &GlyphSet::builtin()
        ));
        let err = cache.get("nowhere").unwrap_err();
        assert!(err.to_string().contains("nowhere"));
        assert_eq!(cache.get("nowhere").unwrap_err(), err);
        // The miss names every place it looked, and what is built in
        let text = err.to_string();
        assert!(text.contains("only `builtin` is built in"), "{text}");
        let tried: Vec<&str> = text.lines().skip(1).map(str::trim).collect();
        assert_eq!(tried[0], "game/scenes/fonts/nowhere.glyphs.toml", "{text}");
        assert!(tried.contains(&"game/fonts/nowhere.glyphs.toml"), "{text}");
    });
}

/// ADR 0114: a scene two folders below the project root finds the
/// project's `fonts/`, which the scene-then-parent search never reached.
#[test]
fn a_scene_two_folders_down_finds_the_projects_set() {
    let mut files = game_files();
    files.insert("game/flint.toml", "");
    files.insert("game/scenes/act1/cave/deep.scene.toml", "");
    vfs::scoped(Arc::new(files), || {
        let mut cache =
            GlyphSetCache::for_scene(Path::new("game/scenes/act1/cave/deep.scene.toml"));
        assert_eq!(cache.get("terminal").unwrap().name(), "terminal");
        // And the path spelling, from the project's root
        assert!(cache.get("fonts/terminal.glyphs.toml").is_ok());
    });
}

/// The web player serves a game from a `.flintpack` through the vfs; a set
/// and its font in a bundle load exactly as they do from disk.
#[test]
fn a_set_loads_from_a_game_bundle() {
    let files = vec![
        (
            "fonts/terminal.glyphs.toml".to_string(),
            TERMINAL.as_bytes().to_vec(),
        ),
        ("fonts/Hack-Regular.ttf".to_string(), HACK.to_vec()),
        (
            "scenes/level.scene.toml".to_string(),
            b"[scene]\nname = \"level\"\n".to_vec(),
        ),
    ];
    let manifest = BundleManifest {
        name: "glyphs".into(),
        scene: Some("scenes/level.scene.toml".into()),
        ..Default::default()
    };
    let bytes = Bundle::write(manifest, files);
    let bundle = Bundle::parse(bytes).unwrap();
    vfs::scoped(Arc::new(bundle.to_memory_source()), || {
        let mut cache = GlyphSetCache::for_scene(Path::new("scenes/level.scene.toml"));
        let set = cache.get("terminal").unwrap();
        assert_eq!(set.ramp("rain").unwrap().pick(1.0), 'I');
        assert!(set.atlas().coverage('@').unwrap() > 0.1);
    });
}

#[test]
fn the_atlas_uploads_and_reads_back() {
    let Ok(ctx) = pollster::block_on(crate::HeadlessContext::new(4, 4)) else {
        crate::gpu_select::skip_without_gpu("no GPU adapter");
        return;
    };
    let atlas = GlyphAtlas::builtin();
    let gpu = GpuGlyphAtlas::upload(&ctx.device, &ctx.queue, &atlas, "glyph atlas test");
    assert_eq!(gpu.size, atlas.size());
    let [w, h] = gpu.size;
    let padded = w.div_ceil(256) * 256;
    let buffer = ctx.device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("glyph readback"),
        size: (padded * h) as u64,
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    let mut encoder = ctx
        .device
        .create_command_encoder(&wgpu::CommandEncoderDescriptor::default());
    encoder.copy_texture_to_buffer(
        gpu.texture.as_image_copy(),
        wgpu::ImageCopyBuffer {
            buffer: &buffer,
            layout: wgpu::ImageDataLayout {
                offset: 0,
                bytes_per_row: Some(padded),
                rows_per_image: Some(h),
            },
        },
        wgpu::Extent3d {
            width: w,
            height: h,
            depth_or_array_layers: 1,
        },
    );
    ctx.queue.submit(Some(encoder.finish()));
    let slice = buffer.slice(..);
    slice.map_async(wgpu::MapMode::Read, |_| {});
    ctx.device.poll(wgpu::Maintain::Wait);
    let data = slice.get_mapped_range();
    for y in 0..h {
        let row = &data[(y * padded) as usize..(y * padded + w) as usize];
        assert_eq!(
            row,
            &atlas.pixels()[(y * w) as usize..((y + 1) * w) as usize]
        );
    }
}

/// #323: on disk, an edited ramp, a font file that appears (a typo fixed),
/// and an edited font file each clear the cache at the next poll, and the
/// next get hands out a new set.
#[test]
fn the_cache_reloads_what_changes_on_disk() {
    let dir = std::env::temp_dir().join(format!("flint-glyphs-poll-{}", std::process::id()));
    std::fs::create_dir_all(dir.join("fonts")).unwrap();
    let plain = dir.join("fonts/plain.glyphs.toml");
    std::fs::write(&plain, "[ramps.dots]\nglyphs = \" .:\"\n").unwrap();
    std::fs::write(dir.join("flint.toml"), "").unwrap();
    let mut cache = GlyphSetCache::new(AssetScope::for_dir(&dir));
    let before = cache.get("plain").unwrap();
    assert!(!cache.poll(), "nothing changed yet");
    assert!(Arc::ptr_eq(&before, &cache.get("plain").unwrap()));

    // A ramp edited
    std::fs::write(&plain, "[ramps.dots]\nglyphs = \" .:#@\"\n").unwrap();
    assert!(cache.poll());
    let after = cache.get("plain").unwrap();
    assert!(
        !Arc::ptr_eq(&before, &after),
        "a new Arc re-uploads the atlas"
    );
    assert_eq!(after.default_ramp().glyphs(), &[' ', '.', ':', '#', '@']);
    assert!(!cache.poll(), "re-read, so settled again");

    // A set whose font file is missing fails, and loads once it's there
    std::fs::write(
        dir.join("fonts/grid.glyphs.toml"),
        "[font]\nfile = \"grid.png\"\ncell = [8, 8]\nchars = \" -\"\n\
         [ramps.r]\nglyphs = \" -\"\n[edges]\nvertical = \"-\"\ndiagonal = \"-\"\nantidiagonal = \"-\"\n",
    )
    .unwrap();
    assert!(cache.get("grid").is_err());
    let png = |row: u32| {
        let mut img = image::GrayImage::new(16, 8);
        for x in 0..8 {
            img.put_pixel(x + 8, row, image::Luma([255]));
        }
        img.save(dir.join("fonts/grid.png")).unwrap();
    };
    png(4);
    assert!(cache.poll());
    assert_eq!(
        cache.get("grid").unwrap().atlas().sample('-', 3, 4),
        Some(255)
    );

    // The font file itself redrawn (same size: the stamp's time must see it)
    png(2);
    assert!(cache.poll());
    let grid = cache.get("grid").unwrap();
    assert_eq!(grid.atlas().sample('-', 3, 4), Some(0));
    assert_eq!(grid.atlas().sample('-', 3, 2), Some(255));
    std::fs::remove_dir_all(&dir).ok();
}

/// ADR 0114: every place nearer than the set that loaded is watched, so a
/// set that appears beside the scene later takes over from the project's.
#[test]
fn a_nearer_set_that_appears_later_takes_over() {
    let root = std::env::temp_dir().join(format!("flint-glyphs-near-{}", std::process::id()));
    let scene_dir = root.join("scenes/act1");
    std::fs::create_dir_all(&scene_dir).unwrap();
    std::fs::create_dir_all(root.join("fonts")).unwrap();
    std::fs::write(root.join("flint.toml"), "").unwrap();
    std::fs::write(
        root.join("fonts/plain.glyphs.toml"),
        "[ramps.dots]\nglyphs = \" .:\"\n",
    )
    .unwrap();
    let mut cache = GlyphSetCache::new(AssetScope::for_dir(&scene_dir));
    let far = cache.get("plain").unwrap();
    assert_eq!(far.default_ramp().glyphs(), &[' ', '.', ':']);
    assert!(!cache.poll());

    std::fs::create_dir_all(scene_dir.join("fonts")).unwrap();
    std::fs::write(
        scene_dir.join("fonts/plain.glyphs.toml"),
        "[ramps.dots]\nglyphs = \" #\"\n",
    )
    .unwrap();
    assert!(cache.poll(), "a nearer file appeared");
    let near = cache.get("plain").unwrap();
    assert_eq!(near.default_ramp().glyphs(), &[' ', '#']);
    assert!(!cache.poll());
    std::fs::remove_dir_all(&root).ok();
}

/// Counts WARN events on this thread, so a test can hold "said once".
struct WarnCount(Arc<std::sync::atomic::AtomicUsize>);

impl tracing::Subscriber for WarnCount {
    fn enabled(&self, _: &tracing::Metadata<'_>) -> bool {
        true
    }
    fn new_span(&self, _: &tracing::span::Attributes<'_>) -> tracing::span::Id {
        tracing::span::Id::from_u64(1)
    }
    fn record(&self, _: &tracing::span::Id, _: &tracing::span::Record<'_>) {}
    fn record_follows_from(&self, _: &tracing::span::Id, _: &tracing::span::Id) {}
    fn event(&self, event: &tracing::Event<'_>) {
        if *event.metadata().level() == tracing::Level::WARN {
            self.0.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        }
    }
    fn enter(&self, _: &tracing::span::Id) {}
    fn exit(&self, _: &tracing::span::Id) {}
}

/// #286: a broken edit to a set that loaded keeps its last good glyphs (the
/// same `Arc`, so the atlas isn't re-uploaded) and warns once however many
/// frames ask; fixing the file loads the new glyphs.
#[test]
fn a_broken_edit_keeps_the_last_good_glyphs() {
    let dir = std::env::temp_dir().join(format!("flint-glyphs-broken-{}", std::process::id()));
    std::fs::create_dir_all(dir.join("fonts")).unwrap();
    let plain = dir.join("fonts/plain.glyphs.toml");
    std::fs::write(&plain, "[ramps.dots]\nglyphs = \" .:\"\n").unwrap();
    let warns = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let count = || warns.load(std::sync::atomic::Ordering::SeqCst);
    std::fs::write(dir.join("flint.toml"), "").unwrap();
    tracing::subscriber::with_default(WarnCount(warns.clone()), || {
        let mut cache = GlyphSetCache::new(AssetScope::for_dir(&dir));
        let good = cache.get("plain").unwrap();
        assert_eq!(count(), 0);

        // Half-typed: not TOML
        std::fs::write(&plain, "[ramps.dots\nglyphs = \" .:#\"\n").unwrap();
        assert!(cache.poll());
        for _ in 0..30 {
            let kept = cache.get("plain").expect("the last good set");
            assert!(Arc::ptr_eq(&good, &kept), "no re-upload");
            assert!(!cache.poll(), "settled on the broken file");
        }
        assert_eq!(count(), 1, "one warning for the broken edit");

        // Fixed
        std::fs::write(&plain, "[ramps.dots]\nglyphs = \" .:#\"\n").unwrap();
        assert!(cache.poll());
        let fixed = cache.get("plain").unwrap();
        assert!(!Arc::ptr_eq(&good, &fixed));
        assert_eq!(fixed.default_ramp().glyphs(), &[' ', '.', ':', '#']);
        assert_eq!(count(), 1);

        // Deleted: still the last good set, and one more warning
        std::fs::remove_file(&plain).unwrap();
        assert!(cache.poll());
        assert!(Arc::ptr_eq(&fixed, &cache.get("plain").unwrap()));
        assert!(Arc::ptr_eq(&fixed, &cache.get("plain").unwrap()));
        assert_eq!(count(), 2);

        // A name that never loaded still fails, as before
        assert!(cache.get("nowhere").is_err());
    });
    std::fs::remove_dir_all(&dir).ok();
}
