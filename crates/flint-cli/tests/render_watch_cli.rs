//! `flint render --watch` (#432), end to end: a project with one user style
//! that paints the frame grey. Saving the style's WGSL rewrites the PNG on
//! the renderer that drew it ("same renderer"), at the new grey; saving the
//! scene loads it again; and the render's own writes into the scene's folder
//! never wake it. Skips itself on a machine without a GPU.

use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::mpsc::{channel, Receiver};
use std::time::{Duration, Instant};

const FLINT: &str = env!("CARGO_BIN_EXE_flint");

const SCENE: &str = r#"
[scene]
name = "Watched"

[camera]
position = [0.0, 1.0, 4.0]
target = [0.0, 0.5, 0.0]
"#;

const DESCRIPTION: &str =
    "version = 1\nname = \"utest_watch\"\nstage = \"display\"\nshader = \"utest_watch.wgsl\"\n";

fn grey(level: f32) -> String {
    format!(
        "fn style(p: StylePixel) -> vec4<f32> {{\n    \
         return vec4<f32>({level:?}, {level:?}, {level:?}, 1.0);\n}}\n"
    )
}

fn repo() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

/// The run, its stdout a line at a time, killed when dropped.
struct Watching {
    child: Child,
    lines: Receiver<String>,
    seen: Vec<String>,
}

impl Drop for Watching {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

impl Watching {
    /// The next `watch: render` line, within `timeout`.
    fn render_line(&mut self, timeout: Duration) -> Option<String> {
        let end = Instant::now() + timeout;
        while let Some(left) = end.checked_duration_since(Instant::now()) {
            let Ok(line) = self.lines.recv_timeout(left) else {
                return None;
            };
            self.seen.push(line.clone());
            if line.starts_with("watch: render") {
                return Some(line);
            }
        }
        None
    }
}

fn level(path: &Path) -> u8 {
    image::open(path)
        .expect("the PNG reads")
        .to_rgba8()
        .get_pixel(8, 8)[0]
}

#[test]
fn a_saved_wgsl_rewrites_the_png_on_the_same_renderer() {
    let dir = std::env::temp_dir().join(format!("flint-render-watch-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let styles = dir.join("styles");
    std::fs::create_dir_all(&styles).unwrap();
    std::fs::write(dir.join("watched.scene.toml"), SCENE).unwrap();
    std::fs::write(styles.join("utest_watch.shader.toml"), DESCRIPTION).unwrap();
    let wgsl = styles.join("utest_watch.wgsl");
    std::fs::write(&wgsl, grey(0.25)).unwrap();
    // The output inside the watched folder: its writes must not loop
    let out = dir.join("shot.png");

    let mut child = Command::new(FLINT)
        .args(["render", "watched.scene.toml", "-o", "shot.png"])
        .args(["--width", "64", "--height", "64", "--no-grid"])
        .args(["--style", "utest_watch", "--watch", "--schemas"])
        .arg(repo().join("schemas"))
        .current_dir(&dir)
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .expect("flint runs");
    let stdout = child.stdout.take().unwrap();
    let (tx, lines) = channel();
    std::thread::spawn(move || {
        for line in BufReader::new(stdout).lines().map_while(Result::ok) {
            if tx.send(line).is_err() {
                break;
            }
        }
    });
    let mut run = Watching {
        child,
        lines,
        seen: Vec::new(),
    };

    let first = run
        .render_line(Duration::from_secs(180))
        .unwrap_or_else(|| panic!("no first render: {:#?}", run.seen));
    if first.contains("headless render context") {
        eprintln!("no GPU adapter: skipped");
        return;
    }
    assert!(!first.contains("failed"), "{first}");
    let before = level(&out);
    assert!((before as i32 - 64).abs() < 6, "drew {before} for 0.25");

    // The render's own write woke nothing
    std::thread::sleep(Duration::from_millis(600));
    assert_eq!(run.render_line(Duration::from_millis(400)), None);

    let saved = Instant::now();
    std::fs::write(&wgsl, grey(0.75)).unwrap();
    let line = run
        .render_line(Duration::from_secs(10))
        .unwrap_or_else(|| panic!("the save drew nothing: {:#?}", run.seen));
    eprintln!(
        "{line} ({:.2} s after the save)",
        saved.elapsed().as_secs_f64()
    );
    assert!(line.contains("same renderer"), "{line}");
    let after = level(&out);
    assert!((after as i32 - 191).abs() < 6, "drew {after} for 0.75");

    // A typo keeps the last good build and names the line
    std::fs::write(&wgsl, grey(0.75).replace("1.0);", "1.0)")).unwrap();
    let line = run
        .render_line(Duration::from_secs(10))
        .expect("the typo drew");
    assert!(line.contains("same renderer"), "{line}");
    std::thread::sleep(Duration::from_millis(200));
    while let Ok(more) = run.lines.try_recv() {
        run.seen.push(more);
    }
    assert!(
        run.seen
            .iter()
            .any(|l| l.contains("style 'utest_watch'") && l.contains("utest_watch.wgsl:")),
        "no error line: {:#?}",
        run.seen
    );
    assert_eq!(
        level(&out),
        after,
        "a broken save keeps the last good build"
    );

    // Anything else loads the scene again
    std::fs::write(
        dir.join("watched.scene.toml"),
        format!("{SCENE}\n# moved\n"),
    )
    .unwrap();
    let line = run
        .render_line(Duration::from_secs(60))
        .expect("the scene save drew");
    assert!(line.contains("scene loaded again"), "{line}");
    drop(run);
    let _ = std::fs::remove_dir_all(&dir);
}
