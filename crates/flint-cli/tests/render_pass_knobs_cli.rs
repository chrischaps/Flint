//! The pass knobs (#1317) end to end through `flint render`: on the frozen
//! ASCII edges fixture, repeating parts that redraw their targets whole
//! draws the same bytes, a part that keeps history refuses and says so,
//! skipping the shared edges takes the edge lines out of the picture and
//! its row out of `--gpu-time`, a label no part has is named, and a word
//! that doesn't read fails the command. They skip themselves without a GPU.

use std::path::{Path, PathBuf};
use std::process::Output;

const FLINT: &str = env!("CARGO_BIN_EXE_flint");

fn repo() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn dir(name: &str) -> PathBuf {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR"))
        .join("render_pass_knobs_cli")
        .join(name);
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// The ASCII edges fixture at 320x180, with `knobs` after the flags.
fn render(dir: &Path, name: &str, knobs: &[&str]) -> (Output, PathBuf) {
    let out = dir.join(format!("{name}.png"));
    let run = std::process::Command::new(FLINT)
        .arg("render")
        .arg(repo().join("tests/fixtures/scenes/ascii_edges.scene.toml"))
        .args(["--width", "320", "--height", "180", "--no-grid"])
        .arg("-o")
        .arg(&out)
        .args(knobs)
        .output()
        .expect("run flint");
    (run, out)
}

fn stderr(o: &Output) -> String {
    String::from_utf8_lossy(&o.stderr).into_owned()
}

fn stdout(o: &Output) -> String {
    String::from_utf8_lossy(&o.stdout).into_owned()
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

fn bytes(png: &Path) -> Vec<u8> {
    image::open(png).unwrap().to_rgba8().into_raw()
}

#[test]
fn a_repeat_draws_the_same_bytes_and_a_part_with_history_refuses() {
    let dir = dir("repeat");
    let (run, plain) = render(&dir, "plain", &[]);
    if no_gpu(&run) {
        return;
    }
    ok(&run);
    let (run, repeated) = render(
        &dir,
        "repeated",
        &[
            "--repeat-pass",
            "Scene:3,Shadows:2",
            "--repeat-pass",
            "Style edges:4",
            "--repeat-pass",
            "Composite:2",
        ],
    );
    ok(&run);
    assert_eq!(
        bytes(&repeated),
        bytes(&plain),
        "a repeat changed the picture"
    );

    // ASCII keeps its cells' history from frame to frame: it draws once,
    // named as the part names itself however the flag spelled it
    let (run, ascii) = render(&dir, "ascii", &["--repeat-pass", "ascii:3"]);
    ok(&run);
    let said = stderr(&run);
    assert!(
        said.contains("`ASCII` draws once: it keeps history"),
        "no refusal said: {said}"
    );
    assert_eq!(bytes(&ascii), bytes(&plain));
}

#[test]
fn skipping_the_edges_takes_their_lines_and_their_row_away() {
    let dir = dir("skip");
    let (run, plain) = render(&dir, "plain", &["--gpu-time", "3"]);
    if no_gpu(&run) {
        return;
    }
    ok(&run);
    let timed = stdout(&run);
    let (run, skipped) = render(
        &dir,
        "skipped",
        &["--skip-pass", "Style edges", "--gpu-time", "3"],
    );
    ok(&run);
    // Without timestamps on the adapter neither run has rows to compare
    let row = |text: &str| {
        text.lines()
            .any(|l| l.trim_start().starts_with("Style edges "))
    };
    if row(&timed) {
        assert!(!row(&stdout(&run)), "a skipped part kept its GPU row");
        assert!(stdout(&run)
            .lines()
            .any(|l| l.trim_start().starts_with("ASCII ")));
    }
    let (a, b) = (bytes(&plain), bytes(&skipped));
    let changed = a.chunks(4).zip(b.chunks(4)).filter(|(p, q)| p != q).count();
    assert!(changed > 0, "skipping the edges changed no pixel");
    // ASCII still drew: the picture is still mostly what it was
    assert!(changed < a.len() / 4 / 4, "{changed} pixels changed");
}

#[test]
fn a_label_no_part_has_is_named_and_a_bad_word_fails() {
    let dir = dir("words");
    let (run, _) = render(&dir, "bogus", &["--skip-pass", "Edges"]);
    if no_gpu(&run) {
        return;
    }
    ok(&run);
    let said = stderr(&run);
    assert!(
        said.contains("no part of the frame is named `Edges`") && said.contains("Style edges"),
        "no warning for a label no part has: {said}"
    );

    for (word, says) in [
        ("Scene", "expected <label>:<N>"),
        ("Scene:0", "skip_pass=Scene"),
        ("Scene:1000", "at most 256"),
    ] {
        let (run, _) = render(&dir, "bad", &["--repeat-pass", word]);
        assert!(!run.status.success(), "--repeat-pass {word} rendered");
        assert!(stderr(&run).contains(says), "{word}: {}", stderr(&run));
    }
}
