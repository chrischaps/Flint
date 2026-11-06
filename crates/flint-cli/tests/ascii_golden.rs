//! Golden image for the full-screen ASCII style (#265): `flint render
//! demo/pbr_test.scene.toml --ascii` against `tests/golden/ascii_pbr_test.png`.
//!
//! The comparison has a tolerance, because GPUs shade the scene a little
//! differently and a cell on a ramp step's border can flip glyph: it fails
//! when more than 2% of pixels differ by more than 48 levels, or when the
//! mean difference passes 3. A broken grid, the wrong ramp, a lost glyph
//! or a colour mode gone wrong moves far more than that.
//!
//! Without a GPU adapter the render fails to start; the test prints a note
//! and passes. `FLINT_BLESS=1 cargo test -p flint-cli --test ascii_golden`
//! rewrites the golden after a deliberate change; look at it before
//! committing.

use std::path::{Path, PathBuf};
use std::process::Command;

const FLINT: &str = env!("CARGO_BIN_EXE_flint");
const WIDTH: u32 = 640;
const HEIGHT: u32 = 360;

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn golden() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/golden/ascii_pbr_test.png")
}

#[test]
fn ascii_pbr_test_matches_its_golden() {
    let out = Path::new(env!("CARGO_TARGET_TMPDIR")).join("ascii_pbr_test.png");
    let _ = std::fs::remove_file(&out);
    let root = repo_root();
    let run = Command::new(FLINT)
        .current_dir(&root)
        .args([
            "render",
            "demo/pbr_test.scene.toml",
            "--ascii",
            "--schemas",
            "schemas",
            "--width",
            &WIDTH.to_string(),
            "--height",
            &HEIGHT.to_string(),
            "--output",
        ])
        .arg(&out)
        .output()
        .expect("run flint");
    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&run.stdout),
        String::from_utf8_lossy(&run.stderr)
    );
    if !run.status.success() {
        if text.to_lowercase().contains("adapter") {
            eprintln!("no GPU adapter; skipping:\n{text}");
            return;
        }
        panic!("flint render failed:\n{text}");
    }

    let got = image::open(&out).expect("read the render").to_rgba8();
    assert_eq!(got.dimensions(), (WIDTH, HEIGHT));
    if std::env::var_os("FLINT_BLESS").is_some() {
        std::fs::create_dir_all(golden().parent().unwrap()).unwrap();
        got.save(golden()).unwrap();
        eprintln!("blessed {}", golden().display());
        return;
    }
    let want = image::open(golden())
        .expect("read the golden (FLINT_BLESS=1 makes it)")
        .to_rgba8();
    assert_eq!(want.dimensions(), got.dimensions());

    let mut far = 0usize;
    let mut total = 0u64;
    for (a, b) in want.pixels().zip(got.pixels()) {
        let d = (0..3).map(|c| a[c].abs_diff(b[c])).max().unwrap();
        total += d as u64;
        far += (d > 48) as usize;
    }
    let pixels = (WIDTH * HEIGHT) as usize;
    let far_share = far as f64 / pixels as f64;
    let mean = total as f64 / pixels as f64;
    if far_share > 0.02 || mean > 3.0 {
        let diff = out.with_file_name("ascii_pbr_test.got.png");
        got.save(&diff).ok();
        panic!(
            "ASCII render drifted from its golden: {:.2}% of pixels differ by more than 48, \
             mean difference {mean:.2} (render kept at {})",
            far_share * 100.0,
            diff.display()
        );
    }
}
