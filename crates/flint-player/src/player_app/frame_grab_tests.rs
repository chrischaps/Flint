#![allow(clippy::disallowed_methods)] // writes a game to the disk and reads back what the grabs wrote
//! #583, through the player's own frames, headless: a capture line on the
//! expression socket gets the next presented frame back as a PNG and a
//! reply on its connection, and `--record` writes frames on its grid. The
//! frame is drawn into a `HeadlessContext` the way `render()` draws into
//! the window, through the same `FrameGrabs` calls.
//!
//! These need a GPU adapter: without one they say so and pass, unless
//! `FLINT_REQUIRE_GPU=1`. The line and rate rules are unit tests in
//! `frame_grab.rs`.

use super::expression_feed::{ExpressionFeed, ExpressionSubsystem};
use super::frame_grab::{check_record_dir, GrabClock, GrabPlace};
use super::scene_entry::SceneEntry;
use super::PlayerApp;
use flint_core::project::{SchemaDirs, SchemaSource};
use flint_render::{FrameInputs, HeadlessContext, RendererConfig, SceneRenderer};
use std::io::{BufRead, BufReader, Write};
use std::net::TcpStream;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

const STEP: f64 = 1.0 / 60.0;

const SCENE: &str = r#"
[scene]
name = "Grab Bench"

[entities.card]
[entities.card.transform]
position = [0.0, 0.0, 0.0]
[entities.card.model]
asset = "card"

[entities.sun]
[entities.sun.light]
type = "directional"
direction = [0.3, 1.0, 0.2]
"#;

fn game_on_disk(name: &str) -> PathBuf {
    let root = std::env::temp_dir().join(format!("flint-grab-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(root.join("scenes/models")).unwrap();
    std::fs::write(root.join("flint.toml"), "").unwrap();
    std::fs::write(root.join("scenes/bench.scene.toml"), SCENE).unwrap();
    std::fs::write(
        root.join("scenes/models/card.glb"),
        include_bytes!("../../../../tests/fixtures/models/DoubleSidedCard.glb"),
    )
    .unwrap();
    root
}

fn started(root: &Path, gpu: &'static HeadlessContext, feed: Option<ExpressionFeed>) -> PlayerApp {
    let path = root.join("scenes/bench.scene.toml");
    let path = path.to_string_lossy().into_owned();
    let registry = flint_schema::SchemaRegistry::new();
    let (world, file) = flint_scene::load_scene(&path, &registry).expect("the scene loads");
    let mut app = PlayerApp::new(world, path, false, None, None);
    app.apply_scene_settings(&file);
    app.set_schemas(
        registry,
        SchemaDirs {
            dirs: Vec::new(),
            source: SchemaSource::Flags,
            dropped: Vec::new(),
            packages: None,
        },
    );
    app.scene_renderer = Some(SceneRenderer::new_headless(
        &gpu.device,
        &gpu.queue,
        gpu.format,
        gpu.width,
        gpu.height,
        RendererConfig::default(),
    ));
    app.test_gpu = Some(gpu);
    app.launch.expression_feed = feed;
    app.enter_scene(SceneEntry::Start)
        .expect("the scene starts");
    app.core.clock.frame_step = Some(STEP);
    app.capture_frames(Vec::new(), 0, Some(root.join("captures")));
    app
}

/// One frame: `tick`, then what `render` does for the grabs, into the
/// headless target. Returns how long the grab calls took on the frame.
fn frame(app: &mut PlayerApp, gpu: &'static HeadlessContext) -> Duration {
    app.tick();
    let renderer = app.scene_renderer.as_mut().unwrap();
    let inputs = FrameInputs {
        time: app.core.clock.total_time,
        dt: app.core.clock.delta_time as f32,
        ..Default::default()
    };
    renderer.prepare(&gpu.device, &gpu.queue, &app.core.world, &inputs);
    let mut encoder = gpu
        .device
        .create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("Grab Test Frame"),
        });
    renderer.encode_frame_to(
        &gpu.device,
        &gpu.queue,
        &mut encoder,
        gpu.depth_target(),
        &app.camera,
        &gpu.color_view,
        Some(&app.core.world),
    );
    let started = Instant::now();
    app.take_capture_asks();
    let dir = app.frame_grabs.has_asks().then(|| app.capture_line_dir());
    let at = GrabClock {
        game_time: app.core.clock.total_time,
        stepped: true,
    };
    let place = GrabPlace {
        capture_dir: dir.as_deref(),
        scene_path: &app.core.scene_path,
    };
    app.frame_grabs.before_submit(
        &gpu.device,
        &mut encoder,
        &gpu.color_texture,
        true,
        at,
        &place,
    );
    let mut spent = started.elapsed();
    gpu.queue.submit(std::iter::once(encoder.finish()));
    let started = Instant::now();
    app.frame_grabs.submitted();
    app.frame_grabs.poll(&gpu.device);
    spent += started.elapsed();
    spent
}

fn feed_addr(app: &PlayerApp) -> std::net::SocketAddr {
    app.core
        .subsystems
        .find::<ExpressionSubsystem>()
        .and_then(|f| f.view(0.0).socket)
        .expect("the feed listens")
        .addr
}

/// Run frames until `n` reply lines have come back on `reader`.
fn frames_until_replies(
    app: &mut PlayerApp,
    gpu: &'static HeadlessContext,
    reader: &mut BufReader<TcpStream>,
    n: usize,
) -> Vec<serde_json::Value> {
    let mut got = Vec::new();
    let deadline = Instant::now() + Duration::from_secs(20);
    while got.len() < n && Instant::now() < deadline {
        frame(app, gpu);
        let mut line = String::new();
        match reader.read_line(&mut line) {
            Ok(0) => panic!("the player hung up"),
            Ok(_) => got.push(serde_json::from_str(&line).expect("a reply is JSON")),
            Err(e)
                if matches!(
                    e.kind(),
                    std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                ) => {}
            Err(e) => panic!("{e}"),
        }
    }
    got
}

#[test]
fn a_capture_line_gets_the_next_frame_back_on_its_connection() {
    let Some(gpu) = super::scene_entry_tests::gpu() else {
        flint_render::gpu_select::skip_without_gpu("the socket's frame capture");
        return;
    };
    let root = game_on_disk("socket");
    let feed = ExpressionFeed::open(None, Some("127.0.0.1:0")).unwrap();
    let mut app = started(&root, gpu, feed);
    let stream = TcpStream::connect(feed_addr(&app)).unwrap();
    stream
        .set_read_timeout(Some(Duration::from_millis(5)))
        .unwrap();
    let mut writer = stream.try_clone().unwrap();
    let mut reader = BufReader::new(stream);

    // A state first, heard by the scripts, so the reply can say it
    writer.write_all(b"{\"state\": \"listening\"}\n").unwrap();
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        frame(&mut app, gpu);
        let feed = app.core.subsystems.find::<ExpressionSubsystem>().unwrap();
        if feed.view(0.0).record.latest().is_some() || Instant::now() > deadline {
            break;
        }
    }

    writer
        .write_all(b"{\"capture\": \"look\", \"id\": \"a\"}\n{\"capture\": \"../out.png\", \"id\": 2}\n{\"capture\": true}\n")
        .unwrap();
    let replies = frames_until_replies(&mut app, gpu, &mut reader, 3);
    assert_eq!(replies.len(), 3, "{replies:?}");

    // The refused one is answered by the reader at once, the others by
    // the writer after the frame
    let refused = replies.iter().find(|r| r["id"] == 2).expect("the refusal");
    assert!(
        refused["capture_error"]
            .as_str()
            .unwrap()
            .contains("plain file name"),
        "{refused}"
    );
    let named = replies
        .iter()
        .find(|r| r["id"] == "a")
        .expect("the named reply");
    let path = PathBuf::from(named["captured"].as_str().unwrap());
    assert!(path.is_absolute(), "{path:?}");
    assert_eq!(path.file_name().unwrap(), "look.png");
    assert!(path.starts_with(std::path::absolute(root.join("captures")).unwrap()));
    assert_eq!(named["width"], gpu.width);
    assert_eq!(named["height"], gpu.height);
    assert_eq!(named["state"]["state"], "listening", "{named}");
    assert!(named["t"].as_f64().unwrap() > 0.0);

    let auto = replies
        .iter()
        .find(|r| r.get("id").is_none())
        .expect("the automatic one");
    let auto_path = PathBuf::from(auto["captured"].as_str().unwrap());
    let auto_name = auto_path
        .file_name()
        .unwrap()
        .to_string_lossy()
        .into_owned();
    assert!(auto_name.starts_with("bench-t"), "{auto_name}");
    assert_eq!(
        auto["frame"], named["frame"],
        "both lines took the same frame"
    );

    // The PNG is the frame as drawn: the scene stands still, so the target
    // now holds the same pixels
    let png = image::open(&path).expect("a PNG").to_rgb8();
    assert_eq!(png.dimensions(), (gpu.width, gpu.height));
    let rgba = pollster::block_on(gpu.read_pixels()).unwrap();
    let drawn: Vec<u8> = rgba
        .as_chunks::<4>()
        .0
        .iter()
        .flat_map(|p| [p[0], p[1], p[2]])
        .collect();
    assert_eq!(png.as_raw(), &drawn, "the PNG is what the frame showed");

    // A state line still reaches the scripts: the capture lines didn't
    let feed = app.core.subsystems.find::<ExpressionSubsystem>().unwrap();
    assert_eq!(
        feed.view(0.0).record.delivered(),
        1,
        "only the state was heard"
    );
    app.finish_frame_grabs();
    assert!(app.frame_grabs.failure().is_none());
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn a_recording_writes_its_frames_on_the_game_clocks_grid() {
    let Some(gpu) = super::scene_entry_tests::gpu() else {
        flint_render::gpu_select::skip_without_gpu("the player's recording");
        return;
    };
    let root = game_on_disk("record");
    let dir = root.join("rec");
    check_record_dir(&dir, 8.0).unwrap();
    let mut app = started(&root, gpu, None);
    app.record_frames(dir.clone(), 8.0);
    let mut times = Vec::new();
    let mut spent = Vec::new();
    for _ in 0..60 {
        spent.push(frame(&mut app, gpu));
        times.push(app.core.clock.total_time);
    }
    app.finish_frame_grabs();
    assert!(
        app.frame_grabs.failure().is_none(),
        "{:?}",
        app.frame_grabs.failure()
    );

    // The clock stepped 1/60 a frame throughout: nothing waited on it
    for pair in times.windows(2) {
        assert!((pair[1] - pair[0] - STEP).abs() < 1e-9, "{times:?}");
    }
    // 8 a second from the first frame (1/60): the first frame at or past
    // each eighth of a second after it
    let lines: Vec<serde_json::Value> = std::fs::read_to_string(dir.join("frames.jsonl"))
        .unwrap()
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect();
    let at: Vec<f64> = lines
        .iter()
        .map(|l| l["game_time"].as_f64().unwrap())
        .collect();
    assert_eq!(lines.len(), 8, "{at:?}");
    for (i, line) in lines.iter().enumerate() {
        let file = line["file"].as_str().unwrap();
        assert_eq!(file, format!("frame_{:06}.png", i + 1));
        let png = image::open(dir.join(file)).unwrap();
        assert_eq!((png.width(), png.height()), (gpu.width, gpu.height));
        assert!(line["unix_ms"].as_u64().unwrap() > 1_700_000_000_000);
    }
    for (k, t) in at.iter().enumerate() {
        let due = at[0] + k as f64 * 0.125;
        assert!((due - 1e-6..due + STEP).contains(t), "frame {k}: {at:?}");
    }
    let concat = std::fs::read_to_string(dir.join("frames.ffconcat")).unwrap();
    assert!(concat.starts_with("ffconcat version 1.0\n"));
    assert_eq!(concat.matches("duration ").count(), 8);
    assert!(concat.trim_end().ends_with("file 'frame_000008.png'"));

    let worst = spent.iter().max().unwrap();
    println!(
        "grab calls on the frame: worst {worst:?} of {} frames",
        spent.len()
    );
    let _ = std::fs::remove_dir_all(&root);
}
