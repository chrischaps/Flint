use super::*;
use flint_core::bundle::{Bundle, BundleManifest};
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
    assert_eq!(atlas.glyphs().len(), 95 + 8);
    // 16 columns of 8 px cells with a 1 px gutter.
    assert_eq!(atlas.size(), [1 + 16 * 9, 1 + 7 * 9]);
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
    assert_eq!(ramps, 4);
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
        eprintln!("no GPU adapter; skipping the upload round-trip");
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
    let mut cache = GlyphSetCache::new(vec![dir.clone()]);
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
