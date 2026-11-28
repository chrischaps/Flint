//! `flint render --set` and `--sweep` of an entity's field (#1699), run on
//! the real binary against the frozen `terrain_grass` fixture: a terrain
//! with grass, a sun and a barrel. The renders skip themselves on a machine
//! without a GPU; the refusals need none, since a key is checked against
//! the world before a GPU wakes.

use std::path::{Path, PathBuf};
use std::process::Output;

const FLINT: &str = env!("CARGO_BIN_EXE_flint");

fn repo() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn fixture() -> PathBuf {
    repo().join("tests/fixtures/scenes/terrain_grass.scene.toml")
}

fn out_dir(name: &str) -> PathBuf {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR"))
        .join("render_entity_field_cli")
        .join(name);
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// `flint render <scene> <words> -o <out>`, small, with the fixtures' schemas.
fn render(scene: &Path, out: &Path, words: &[&str]) -> Output {
    std::process::Command::new(FLINT)
        .arg("render")
        .arg(scene)
        .args(["--schemas", "schemas", "--schemas", "schemas/examples"])
        .args(["--width", "160", "--height", "90", "--no-grid", "-o"])
        .arg(out)
        .args(words)
        .current_dir(repo())
        .output()
        .expect("run flint")
}

fn stderr(o: &Output) -> String {
    String::from_utf8_lossy(&o.stderr).into_owned()
}

fn no_gpu(o: &Output) -> bool {
    let missing = !o.status.success() && stderr(o).contains("headless render context");
    if missing {
        eprintln!("no GPU adapter: skipped");
    }
    missing
}

fn ok(o: &Output) {
    assert!(o.status.success(), "flint failed: {}", stderr(o));
}

fn rgb(path: &Path) -> image::RgbImage {
    image::open(path).unwrap().into_rgb8()
}

fn find_cell(sheet: &image::RgbImage, cell: &image::RgbImage) -> Option<(u32, u32)> {
    let (w, h) = cell.dimensions();
    (0..=sheet.height().saturating_sub(h))
        .flat_map(|y| (0..=sheet.width().saturating_sub(w)).map(move |x| (x, y)))
        .find(|&(x, y)| {
            (0..h).all(|cy| {
                (0..w).all(|cx| sheet.get_pixel(x + cx, y + cy) == cell.get_pixel(cx, cy))
            })
        })
}

/// Pixels of `a` and `b` (one size) that differ at all.
fn differing(a: &image::RgbImage, b: &image::RgbImage) -> usize {
    a.pixels().zip(b.pixels()).filter(|(p, q)| p != q).count()
}

/// The ticket's first done-when: a three-cell sheet of the terrain's grass
/// density whose cells differ, each cell the render of `--set` with its
/// value (drawn through the same clocked path, `--report json`). A value
/// written after the renderer bound the scene would draw the grass the
/// scene has in every cell.
#[test]
fn a_sweep_of_the_grass_density_draws_a_cell_a_value() {
    let dir = out_dir("density");
    let densities = ["10", "30", "60"];
    let sheet = dir.join("sheet.png");
    let o = render(
        &fixture(),
        &sheet,
        &["--sweep", "ground.terrain.grass.density=10,30,60"],
    );
    if no_gpu(&o) {
        return;
    }
    ok(&o);
    assert!(
        !stderr(&o).contains("pixel-identical"),
        "the density showed nothing: {}",
        stderr(&o)
    );
    let sheet = rgb(&sheet);
    let mut cells = Vec::new();
    let mut corners = Vec::new();
    for density in densities {
        let alone = dir.join(format!("density_{density}.png"));
        let o = render(
            &fixture(),
            &alone,
            &[
                "--set",
                &format!("ground.terrain.grass.density={density}"),
                "--report",
                "json",
            ],
        );
        ok(&o);
        let cell = rgb(&alone);
        corners
            .push(find_cell(&sheet, &cell).unwrap_or_else(|| {
                panic!("density {density}: no cell of the sheet is its render")
            }));
        cells.push(cell);
    }
    assert!(
        corners
            .windows(2)
            .all(|p| p[0].0 < p[1].0 && p[0].1 == p[1].1),
        "a cell a value, left to right: {corners:?}"
    );
    for pair in cells.windows(2) {
        assert!(
            differing(&pair[0], &pair[1]) > 100,
            "two densities drew alike"
        );
    }
}

/// The ticket's second: `--set sun.light.direction` moves the shadows, and
/// draws what the scene with that direction written in it draws.
#[test]
fn a_set_of_the_suns_direction_draws_what_the_scene_with_it_draws() {
    let dir = out_dir("sun");
    let noon = dir.join("noon.png");
    let o = render(&fixture(), &noon, &[]);
    if no_gpu(&o) {
        return;
    }
    ok(&o);
    let low = "-0.9,0.35,0.2";
    let set = dir.join("set.png");
    ok(&render(
        &fixture(),
        &set,
        &["--set", &format!("sun.light.direction={low}")],
    ));
    // The scene with the direction written, its files named from the fixture
    let folder = fixture().parent().unwrap().canonicalize().unwrap();
    let folder = folder.to_string_lossy().replace('\\', "/");
    let folder = folder.trim_start_matches("//?/");
    let text = std::fs::read_to_string(fixture())
        .unwrap()
        .replace("\"./", &format!("\"{folder}/"))
        .replace(
            "direction = [0.6, 0.8, 0.3]",
            &format!("direction = [{low}]"),
        );
    assert!(text.contains(&format!("direction = [{low}]")));
    let written = dir.join("written.scene.toml");
    std::fs::write(&written, text).unwrap();
    let edited = dir.join("edited.png");
    ok(&render(&written, &edited, &[]));

    let (noon, set, edited) = (rgb(&noon), rgb(&set), rgb(&edited));
    assert!(
        differing(&noon, &set) > 500,
        "a low sun drew the noon frame"
    );
    assert_eq!(
        differing(&set, &edited),
        0,
        "--set drew other than the scene with the value written"
    );
}

/// A key that names nothing is refused before a GPU wakes, naming what is.
#[test]
fn what_a_key_names_that_isnt_there_is_refused_with_what_is() {
    let dir = out_dir("refused");
    let refused = |words: &[&str]| {
        let o = render(&fixture(), &dir.join("x.png"), words);
        assert!(!o.status.success(), "{words:?} drew");
        assert!(!dir.join("x.png").exists(), "{words:?} wrote a picture");
        stderr(&o)
    };
    let e = refused(&["--set", "groud.terrain.grass.density=30"]);
    assert!(e.contains("no entity 'groud'"), "{e}");
    assert!(e.contains("did you mean `ground`?"), "{e}");
    assert!(e.contains("barrel, ground, sky, sun"), "{e}");
    let e = refused(&["--sweep", "ground.terrain.grass.densty=10,30"]);
    assert!(e.contains("no field `grass.densty`"), "{e}");
    assert!(e.contains("did you mean `grass.density`?"), "{e}");
    let e = refused(&["--set", "sun.lihgt.direction=0,1,0"]);
    assert!(e.contains("did you mean `light`?"), "{e}");
    let e = refused(&["--set", "sun.light.direction=0,1"]);
    assert!(e.contains("three numbers"), "{e}");
    let e = refused(&[
        "--set",
        "ground.terrain.texture_tile=8",
        "--sweep",
        "ground.terrain.texture_tile=8,18",
    ]);
    assert!(e.contains("drop the --set"), "{e}");
}
