//! End-to-end tests for `flint gen --batch` and `--register` (#29): run the
//! real binary in a scratch project directory (the content store lives at
//! `./.flint/assets`) and check the files, sidecars, store and messages.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

const FLINT: &str = env!("CARGO_BIN_EXE_flint");

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn oak_spec() -> PathBuf {
    repo_root().join("specs/oak_tree.procgen.toml")
}

/// A fresh, empty project directory for one test.
fn project(name: &str) -> PathBuf {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR"))
        .join("gen_batch_register")
        .join(name);
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// Run `flint gen` with `dir` as the working directory.
fn gen_in(dir: &Path, args: &[&str]) -> Output {
    Command::new(FLINT)
        .current_dir(dir)
        .arg("gen")
        .args(args)
        .output()
        .expect("run flint")
}

fn text(o: &Output) -> String {
    format!(
        "{}{}",
        String::from_utf8_lossy(&o.stdout),
        String::from_utf8_lossy(&o.stderr)
    )
}

fn ok(o: Output) -> String {
    let t = text(&o);
    assert!(o.status.success(), "flint gen failed:\n{t}");
    t
}

fn s(p: &Path) -> &str {
    p.to_str().unwrap()
}

/// The `[asset.properties]` table of a sidecar.
fn props(sidecar: &Path) -> toml::Table {
    let text = std::fs::read_to_string(sidecar)
        .unwrap_or_else(|e| panic!("no sidecar {}: {e}", sidecar.display()));
    let t: toml::Table = toml::from_str(&text).unwrap();
    t["asset"]["properties"].as_table().unwrap().clone()
}

fn files_in(dir: &Path) -> Vec<String> {
    let mut v: Vec<String> = std::fs::read_dir(dir)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    v.sort();
    v
}

fn store_blobs(project: &Path) -> usize {
    let root = project.join(".flint/assets");
    std::fs::read_dir(&root)
        .map(|dirs| {
            dirs.map(|d| std::fs::read_dir(d.unwrap().path()).unwrap().count())
                .sum()
        })
        .unwrap_or(0)
}

/// A copy of the stone wall spec at 64x64, so a debug build makes it fast.
fn small_wall_spec(dir: &Path) -> PathBuf {
    let src = std::fs::read_to_string(repo_root().join("specs/stone_wall.procgen.toml")).unwrap();
    let small = src.replace("= 512", "= 64");
    assert_ne!(src, small);
    let path = dir.join("wall.procgen.toml");
    std::fs::write(&path, small).unwrap();
    path
}

#[test]
fn a_batch_of_five_writes_five_files_named_by_seed_in_order() {
    let dir = project("batch5");
    let t = ok(gen_in(
        &dir,
        &[
            s(&oak_spec()),
            "-o",
            "out/tree.glb",
            "--batch",
            "5",
            "--seed-start",
            "100",
        ],
    ));
    assert_eq!(
        files_in(&dir.join("out")),
        [
            "tree_100.glb",
            "tree_101.glb",
            "tree_102.glb",
            "tree_103.glb",
            "tree_104.glb"
        ]
    );
    // Progress is reported per variant, in order.
    let mut at = 0;
    for (i, seed) in (100..105).enumerate() {
        let line = format!("[{}/5] Generating seed {seed}...", i + 1);
        at += t[at..]
            .find(&line)
            .unwrap_or_else(|| panic!("no '{line}' in\n{t}"));
    }
    assert!(t.contains("Batch complete: 5 generated, 0 cached"), "{t}");
    // Different seeds make different trees.
    let a = std::fs::read(dir.join("out/tree_100.glb")).unwrap();
    let b = std::fs::read(dir.join("out/tree_101.glb")).unwrap();
    assert_ne!(a, b);
}

#[test]
fn the_seed_start_defaults_to_zero() {
    let dir = project("batch_default_start");
    ok(gen_in(
        &dir,
        &[s(&oak_spec()), "-o", "t.glb", "--batch", "2"],
    ));
    assert_eq!(files_in(&dir), ["t_0.glb", "t_1.glb"]);
}

#[test]
fn a_batch_and_register_records_each_variant_on_its_own() {
    let dir = project("batch_register");
    ok(gen_in(
        &dir,
        &[
            s(&oak_spec()),
            "-o",
            "out/tree.glb",
            "--batch",
            "5",
            "--seed-start",
            "100",
            "--register",
        ],
    ));
    let spec_hash = flint_core::ContentHash::from_file(oak_spec())
        .unwrap()
        .to_prefixed_hex();
    for seed in 100..105i64 {
        let out = dir.join(format!("out/tree_{seed}.glb"));
        let sidecar = dir.join(format!("out/tree_{seed}.glb.asset.toml"));
        let p = props(&sidecar);
        assert_eq!(p["generator"].as_str(), Some("tree_v1"));
        assert_eq!(p["spec_hash"].as_str(), Some(spec_hash.as_str()));
        assert_eq!(p["seed"].as_integer(), Some(seed));
        assert_eq!(p["flint_version"].as_str(), Some(env!("CARGO_PKG_VERSION")));
        // ISO 8601 UTC, e.g. 2026-10-31T21:12:48Z.
        let ts = p["timestamp"].as_str().unwrap();
        assert!(
            ts.len() == 20 && ts.as_bytes()[10] == b'T' && ts.ends_with('Z'),
            "timestamp {ts}"
        );

        // The sidecar's hash is the file's, and the store holds those bytes
        // at the hash-derived path.
        let t: toml::Table = toml::from_str(&std::fs::read_to_string(&sidecar).unwrap()).unwrap();
        let hash = flint_core::ContentHash::from_file(&out).unwrap();
        assert_eq!(
            t["asset"]["hash"].as_str(),
            Some(hash.to_prefixed_hex().as_str())
        );
        let hex = hash.to_hex();
        let blob = dir.join(format!(".flint/assets/{}/{hex}.glb", &hex[..2]));
        assert_eq!(std::fs::read(blob).unwrap(), std::fs::read(&out).unwrap());
    }
    assert_eq!(store_blobs(&dir), 5);
}

#[test]
fn a_second_registered_run_is_a_cache_hit_and_force_regenerates() {
    let dir = project("dedup");
    let oak = oak_spec();
    let args = [
        s(&oak),
        "-o",
        "tree.glb",
        "--batch",
        "1",
        "--seed-start",
        "10",
        "--register",
    ];
    let first = ok(gen_in(&dir, &args));
    assert!(first.contains("Generating seed 10"), "{first}");
    let sidecar = dir.join("tree_10.glb.asset.toml");
    let stamp = props(&sidecar)["timestamp"].clone();

    let second = ok(gen_in(&dir, &args));
    assert!(second.contains("Cached: "), "{second}");
    assert!(!second.contains("Generating"), "{second}");
    assert!(second.contains("0 generated, 1 cached"), "{second}");
    // A hit leaves the provenance as registration wrote it.
    assert_eq!(props(&sidecar)["timestamp"], stamp);

    let mut forced = args.to_vec();
    forced.push("--force");
    let third = ok(gen_in(&dir, &forced));
    assert!(third.contains("Generating seed 10"), "{third}");
    assert!(third.contains("1 generated, 0 cached"), "{third}");
}

#[test]
fn a_plain_register_records_the_specs_own_seed_and_hits_the_cache() {
    // The oak spec pins seed 42. A plain --register used to record 0, and
    // never found a cache hit without --seed.
    let dir = project("plain_register");
    let oak = oak_spec();
    let args = [s(&oak), "-o", "tree.glb", "--register"];
    let first = ok(gen_in(&dir, &args));
    assert!(first.contains("Seed:       42 (fixed)"), "{first}");
    assert_eq!(
        props(&dir.join("tree.glb.asset.toml"))["seed"].as_integer(),
        Some(42)
    );
    let second = ok(gen_in(&dir, &args));
    assert!(second.contains("Cached: "), "{second}");
}

/// The one provenance entry a single registered run left in `dir`.
fn only_entry(dir: &Path) -> PathBuf {
    let specs: Vec<_> = std::fs::read_dir(dir.join(".flint/procgen"))
        .unwrap()
        .map(|e| e.unwrap().path())
        .collect();
    assert_eq!(specs.len(), 1, "{specs:?}");
    let entries: Vec<_> = std::fs::read_dir(&specs[0])
        .unwrap()
        .map(|e| e.unwrap().path())
        .collect();
    assert_eq!(entries.len(), 1, "{entries:?}");
    entries[0].clone()
}

#[test]
fn an_entry_from_another_generator_revision_is_stale_and_regenerates() {
    // #178: the spec hash doesn't cover the generator, so an entry an older
    // tree_v1 made used to come back `Cached:` after the generator changed.
    let dir = project("stale_revision");
    let oak = oak_spec();
    let args = [s(&oak), "-o", "tree.glb", "--register"];
    ok(gen_in(&dir, &args));
    let sidecar = dir.join("tree.glb.asset.toml");
    let rev = props(&sidecar)["generator_revision"]
        .as_integer()
        .expect("the sidecar records the generator revision");
    assert!(rev >= 1);
    let entry = only_entry(&dir);
    let line = format!("generator_revision = {rev}");
    let text = std::fs::read_to_string(&entry).unwrap();
    assert!(text.contains(&line), "{text}");

    // As if the files were made by the revision before this one.
    std::fs::write(
        &entry,
        text.replace(&line, &format!("generator_revision = {}", rev - 1)),
    )
    .unwrap();
    let t = ok(gen_in(&dir, &args));
    assert!(
        t.contains(&format!(
            "Stale: tree.glb (seed 42, tree_v1 rev {} -> {rev}), regenerating",
            rev - 1
        )),
        "{t}"
    );
    assert!(t.contains("Registered: "), "{t}");
    assert!(!t.contains("Cached: "), "{t}");
    // The rewrite records the revision that made the file, and is a hit.
    assert_eq!(
        props(&sidecar)["generator_revision"].as_integer(),
        Some(rev)
    );
    assert!(std::fs::read_to_string(&entry).unwrap().contains(&line));
    let t = ok(gen_in(&dir, &args));
    assert!(
        t.contains(&format!("spec and tree_v1 rev {rev} unchanged")),
        "{t}"
    );

    // An entry from before revisions were recorded is stale too.
    let text = std::fs::read_to_string(&entry).unwrap();
    std::fs::write(&entry, text.replace(&format!("{line}\n"), "")).unwrap();
    let t = ok(gen_in(&dir, &args));
    assert!(
        t.contains(&format!("tree_v1 rev unrecorded -> {rev}), regenerating")),
        "{t}"
    );
}

#[test]
fn the_cache_restores_a_deleted_or_altered_output_from_the_store() {
    let dir = project("restore");
    let oak = oak_spec();
    let args = [s(&oak), "-o", "tree.glb", "--register"];
    ok(gen_in(&dir, &args));
    let good = std::fs::read(dir.join("tree.glb")).unwrap();

    std::fs::remove_file(dir.join("tree.glb")).unwrap();
    let t = ok(gen_in(&dir, &args));
    assert!(t.contains("restored 1 missing or changed file"), "{t}");
    assert_eq!(std::fs::read(dir.join("tree.glb")).unwrap(), good);

    // A file edited since registration is not a hit for what's on disk.
    std::fs::write(dir.join("tree.glb"), b"junk").unwrap();
    let t = ok(gen_in(&dir, &args));
    assert!(t.contains("restored 1"), "{t}");
    assert_eq!(std::fs::read(dir.join("tree.glb")).unwrap(), good);

    // With the store emptied too, it generates again.
    std::fs::remove_dir_all(dir.join(".flint/assets")).unwrap();
    std::fs::remove_file(dir.join("tree.glb")).unwrap();
    let t = ok(gen_in(&dir, &args));
    assert!(t.contains("Registered: "), "{t}");
    assert_eq!(std::fs::read(dir.join("tree.glb")).unwrap(), good);
}

#[test]
fn a_texture_spec_registers_every_map() {
    let dir = project("texture");
    let spec = small_wall_spec(&dir);
    let args = [
        s(&spec),
        "-o",
        "tex/wall.png",
        "--batch",
        "2",
        "--seed-start",
        "5",
        "--register",
    ];
    ok(gen_in(&dir, &args));
    for seed in [5i64, 6] {
        for map in ["color", "normal", "roughness"] {
            let sidecar = dir.join(format!("tex/wall_{seed}_{map}.png.asset.toml"));
            let p = props(&sidecar);
            assert_eq!(p["seed"].as_integer(), Some(seed));
            assert_eq!(p["map"].as_str(), Some(map));
            assert_eq!(p["generator"].as_str(), Some("texture_v1"));
        }
    }
    assert_eq!(store_blobs(&dir), 6);

    // One lost map comes back from the store; the others are untouched.
    std::fs::remove_file(dir.join("tex/wall_6_normal.png")).unwrap();
    let t = ok(gen_in(&dir, &args));
    assert!(t.contains("0 generated, 2 cached"), "{t}");
    assert!(dir.join("tex/wall_6_normal.png").is_file());
}

#[test]
fn flags_that_would_be_ignored_are_refused() {
    let dir = project("refused");
    let spec = oak_spec();
    for (args, why) in [
        (&["--seed-start", "5"][..], "--seed-start without --batch"),
        (&["--batch", "0"][..], "--batch 0"),
        (&["--batch", "2", "--seed", "7"][..], "--seed with --batch"),
        (&["--force"][..], "--force without --register"),
    ] {
        let mut all = vec![s(&spec), "-o", "t.glb"];
        all.extend_from_slice(args);
        let o = gen_in(&dir, &all);
        assert!(!o.status.success(), "{why} was accepted:\n{}", text(&o));
    }
    assert!(files_in(&dir).is_empty(), "wrote {:?}", files_in(&dir));
}
