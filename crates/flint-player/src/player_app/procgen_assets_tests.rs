//! Procgen resolution order (#32), headless: a recording sink stands in for
//! the renderer and the specs live in a `MemorySource`, the way a bundle
//! brings them on the web.

use super::*;
use flint_core::vfs::MemorySource;
use flint_procgen::{GenerationCost, Generator, OutputKind, ProcGenSpec, SeededRng};
use std::collections::HashMap;
use std::sync::Arc;

/// What the renderer was asked to do, in order.
#[derive(Debug, Clone, PartialEq)]
enum Event {
    Mesh(String, usize),
    Image(String),
    Placeholder(String, AssetKind),
    File(String, AssetKind),
}

#[derive(Default)]
struct Recorder {
    meshes: HashMap<String, usize>,
    files: HashSet<(String, bool)>,
    events: Vec<Event>,
}

impl Recorder {
    fn with_file(mut self, name: &str, kind: AssetKind) -> Self {
        self.files.insert((name.into(), kind == AssetKind::Model));
        self
    }
}

impl AssetSink for Recorder {
    fn has_mesh(&self, name: &str) -> bool {
        self.meshes.contains_key(name)
    }
    fn has_file(&self, name: &str, kind: AssetKind) -> bool {
        self.files
            .contains(&(name.to_string(), kind == AssetKind::Model))
    }
    fn upload_mesh(&mut self, name: &str, mesh: &MeshData) {
        self.meshes.insert(name.into(), mesh.vertices.len());
        self.events
            .push(Event::Mesh(name.into(), mesh.vertices.len()));
    }
    fn upload_image(&mut self, name: &str, _image: &ImageData) {
        self.events.push(Event::Image(name.into()));
    }
    fn upload_placeholder(&mut self, name: &str, kind: AssetKind) {
        if kind == AssetKind::Model {
            self.meshes.insert(name.into(), 0);
        }
        self.events.push(Event::Placeholder(name.into(), kind));
    }
    fn load_file(&mut self, name: &str, kind: AssetKind) -> bool {
        self.events.push(Event::File(name.into(), kind));
        true
    }
}

/// A registered generator that always fails, for errors past the registry check.
struct Broken;
impl Generator for Broken {
    fn type_name(&self) -> &str {
        "broken"
    }
    fn revision(&self) -> u32 {
        1
    }
    fn output_kind(&self) -> OutputKind {
        OutputKind::Mesh
    }
    fn generate(
        &self,
        _spec: &ProcGenSpec,
        _rng: &mut SeededRng,
    ) -> flint_procgen::Result<GeneratorOutput> {
        Err(flint_procgen::ProcGenError::InvalidParameter {
            name: "everything".into(),
            reason: "this generator is broken on purpose".into(),
        })
    }
    fn param_schema(&self) -> serde_json::Value {
        serde_json::json!({})
    }
    fn estimate_cost(&self, _spec: &ProcGenSpec) -> GenerationCost {
        GenerationCost {
            estimated_vertices: 0,
            estimated_triangles: 0,
            estimated_texture_bytes: 0,
            estimated_generation_ms: 0.0,
        }
    }
}

fn spec(name: &str, generator: &str) -> String {
    format!(
        "generator = \"{generator}\"\n[meta]\nname = \"{name}\"\nversion = \"1.0.0\"\n\
         [seed]\nmode = \"fixed\"\nvalue = 42\n"
    )
}

const SCENE: &str = "game/levels/grove.scene.toml";

/// Specs under `game/specs/` (one directory up from the scene, as games keep them).
fn source(specs: &[(&str, &str)]) -> Arc<MemorySource> {
    let mut src = MemorySource::new();
    src.insert(SCENE, "[scene]\nname = \"grove\"\n");
    for (name, generator) in specs {
        src.insert(
            format!("game/specs/{name}.procgen.toml"),
            spec(name, generator),
        );
    }
    Arc::new(src)
}

fn resolver_for(src: &Arc<MemorySource>) -> ProcGenResolver {
    let mut resolver = ProcGenResolver::new();
    resolver.register_generator(Box::new(Broken));
    vfs::scoped(src.clone(), || discover_specs(&mut resolver, SCENE));
    resolver
}

/// A world of entities naming `model.asset`s, and one ground naming a texture.
fn world(models: &[&str], texture: Option<&str>) -> FlintWorld {
    let mut world = FlintWorld::new();
    for (i, asset) in models.iter().enumerate() {
        let id = world.spawn(format!("thing_{i}")).unwrap();
        let mut model = toml::map::Map::new();
        model.insert("asset".into(), toml::Value::String(asset.to_string()));
        world.set_component(id, comp::MODEL, model.into()).unwrap();
    }
    if let Some(tex) = texture {
        let id = world.spawn("ground").unwrap();
        let mut material = toml::map::Map::new();
        material.insert("texture".into(), toml::Value::String(tex.to_string()));
        world
            .set_component(id, comp::MATERIAL, material.into())
            .unwrap();
    }
    world
}

fn config() -> ModelLoadConfig {
    ModelLoadConfig::from_scene_path(SCENE)
}

fn outcomes(results: &[(AssetRequest, Outcome)]) -> Vec<(&str, &Outcome)> {
    results.iter().map(|(r, o)| (r.name.as_str(), o)).collect()
}

#[test]
fn spec_dirs_put_the_scene_after_the_game() {
    let mut src = MemorySource::new();
    src.insert("game/specs/a.procgen.toml", spec("shared", "mock"));
    src.insert("game/levels/specs/b.procgen.toml", spec("shared", "broken"));
    let src = Arc::new(src);
    let dirs = vfs::scoped(src.clone(), || spec_dirs(SCENE));
    assert_eq!(
        dirs,
        vec![
            PathBuf::from("game/specs"),
            PathBuf::from("game/levels/specs")
        ]
    );
    // The scene's own spec wins over the game-wide one of the same name.
    let mut resolver = ProcGenResolver::new();
    resolver.register_generator(Box::new(Broken));
    vfs::scoped(src, || discover_specs(&mut resolver, SCENE));
    assert!(resolver.check_generator("shared").is_ok());
    assert!(
        resolver.generate("shared").is_err(),
        "the broken one, from beside the scene"
    );
}

#[test]
fn procgen_spec_resolves_a_model() {
    let src = source(&[("oak_tree", "mock")]);
    let mut resolver = resolver_for(&src);
    let world = world(&["oak_tree", "oak_tree"], None);
    let mut sink = Recorder::default();

    let requests = collect_requests(&world, &config(), &resolver, &sink);
    assert_eq!(requests.len(), 1, "two entities, one asset: {requests:?}");
    let results = resolve(&requests, &mut resolver, &mut sink, ProcGenMode::Immediate);

    assert_eq!(outcomes(&results), vec![("oak_tree", &Outcome::Generated)]);
    assert_eq!(sink.events, vec![Event::Mesh("oak_tree".into(), 24)]);
}

#[test]
fn catalog_beats_procgen() {
    let src = source(&[("oak_tree", "mock")]);
    let resolver = resolver_for(&src);
    let world = world(&["oak_tree"], Some("oak_tree"));
    let mut config = config();
    config
        .overrides
        .insert("oak_tree".into(), PathBuf::from("store/ab/cd.glb"));

    let requests = collect_requests(&world, &config, &resolver, &Recorder::default());
    assert!(requests.is_empty(), "the catalog entry wins: {requests:?}");
}

#[test]
fn no_spec_leaves_the_name_to_the_file_loader() {
    let src = source(&[("oak_tree", "mock")]);
    let resolver = resolver_for(&src);
    let world = world(&["birch"], Some("bark"));
    let sink = Recorder::default().with_file("birch", AssetKind::Model);

    assert!(collect_requests(&world, &config(), &resolver, &sink).is_empty());
}

#[test]
fn an_uploaded_mesh_is_not_asked_for_again() {
    let src = source(&[("oak_tree", "mock")]);
    let resolver = resolver_for(&src);
    let world = world(&["oak_tree"], None);
    let mut sink = Recorder::default();
    sink.meshes.insert("oak_tree".into(), 24);

    assert!(collect_requests(&world, &config(), &resolver, &sink).is_empty());
}

#[test]
fn reload_is_a_cache_hit() {
    let src = source(&[("oak_tree", "mock")]);
    let mut resolver = resolver_for(&src);
    let world = world(&["oak_tree"], None);

    // Two scene loads with one resolver: the player keeps it across transitions.
    for expected in [Outcome::Generated, Outcome::CacheHit] {
        let mut sink = Recorder::default(); // a fresh renderer each load
        let requests = collect_requests(&world, &config(), &resolver, &sink);
        let results = resolve(&requests, &mut resolver, &mut sink, ProcGenMode::Immediate);
        assert_eq!(outcomes(&results), vec![("oak_tree", &expected)]);
        assert_eq!(sink.events, vec![Event::Mesh("oak_tree".into(), 24)]);
    }
    let stats = resolver.cache_stats();
    assert_eq!((stats.hits, stats.misses), (1, 1), "generated once");
}

#[test]
fn deferred_shows_a_placeholder_then_swaps_it() {
    let src = source(&[("oak_tree", "mock")]);
    let mut resolver = resolver_for(&src);
    let world = world(&["oak_tree"], None);
    let mut sink = Recorder::default();

    let requests = collect_requests(&world, &config(), &resolver, &sink);
    let results = resolve(&requests, &mut resolver, &mut sink, ProcGenMode::Deferred);
    assert_eq!(outcomes(&results), vec![("oak_tree", &Outcome::Deferred)]);
    // Immediately: the placeholder, and nothing generated yet.
    assert_eq!(
        sink.events,
        vec![Event::Placeholder("oak_tree".into(), AssetKind::Model)]
    );
    assert_eq!(resolver.cache_stats().misses, 0);
    assert!(resolver.is_pending("oak_tree"));

    // A frame later the queue finishes it, and it goes in over the placeholder.
    let completed = resolver.process_frame([0.0; 3]);
    apply_completed(&completed, &mut sink);
    apply_failures(&resolver.take_failures(), &mut sink);
    assert_eq!(
        sink.events,
        vec![
            Event::Placeholder("oak_tree".into(), AssetKind::Model),
            Event::Mesh("oak_tree".into(), 24),
        ]
    );
    assert!(!resolver.is_pending("oak_tree"));
}

#[test]
fn deferred_with_a_warm_cache_skips_the_placeholder() {
    let src = source(&[("oak_tree", "mock")]);
    let mut resolver = resolver_for(&src);
    resolver.generate("oak_tree").unwrap();
    let world = world(&["oak_tree"], None);
    let mut sink = Recorder::default();

    let requests = collect_requests(&world, &config(), &resolver, &sink);
    let results = resolve(&requests, &mut resolver, &mut sink, ProcGenMode::Deferred);
    assert_eq!(outcomes(&results), vec![("oak_tree", &Outcome::CacheHit)]);
    assert_eq!(sink.events, vec![Event::Mesh("oak_tree".into(), 24)]);
}

#[test]
fn unknown_generator_falls_back_to_the_file() {
    let src = source(&[("rock", "boulder_v9")]);
    let mut resolver = resolver_for(&src);
    let world = world(&["rock"], None);
    let mut sink = Recorder::default().with_file("rock", AssetKind::Model);

    let requests = collect_requests(&world, &config(), &resolver, &sink);
    let results = resolve(&requests, &mut resolver, &mut sink, ProcGenMode::Immediate);
    let [(_, Outcome::FileFallback(reason))] = results.as_slice() else {
        panic!("{results:?}");
    };
    assert!(reason.contains("boulder_v9"), "{reason}");
    assert!(
        sink.events.is_empty(),
        "the file loader does it: {:?}",
        sink.events
    );
}

#[test]
fn unknown_generator_and_no_file_is_a_placeholder() {
    let src = source(&[("rock", "boulder_v9")]);
    let mut resolver = resolver_for(&src);
    let world = world(&["rock"], Some("rock"));
    let mut sink = Recorder::default();

    let requests = collect_requests(&world, &config(), &resolver, &sink);
    // Deferred or not: a missing generator is known at once, so it isn't queued.
    let results = resolve(&requests, &mut resolver, &mut sink, ProcGenMode::Deferred);
    assert!(results
        .iter()
        .all(|(_, o)| matches!(o, Outcome::Placeholder(_))));
    // Entities come in no fixed order; compare as a set.
    let mut events = sink.events.clone();
    events.sort_by_key(|e| format!("{e:?}"));
    assert_eq!(
        events,
        vec![
            Event::Placeholder("rock".into(), AssetKind::Model),
            Event::Placeholder("rock".into(), AssetKind::Texture),
        ]
    );
    assert_eq!(resolver.queue_stats().pending_count, 0);
}

#[test]
fn a_generator_error_falls_through() {
    let src = source(&[("rock", "broken")]);
    let mut resolver = resolver_for(&src);
    let world = world(&["rock"], None);

    let mut with_file = Recorder::default().with_file("rock", AssetKind::Model);
    let requests = collect_requests(&world, &config(), &resolver, &with_file);
    let results = resolve(
        &requests,
        &mut resolver,
        &mut with_file,
        ProcGenMode::Immediate,
    );
    assert!(
        matches!(results[0].1, Outcome::FileFallback(_)),
        "{results:?}"
    );

    let mut without = Recorder::default();
    let results = resolve(
        &requests,
        &mut resolver,
        &mut without,
        ProcGenMode::Immediate,
    );
    assert!(
        matches!(results[0].1, Outcome::Placeholder(_)),
        "{results:?}"
    );
    assert_eq!(
        without.events,
        vec![Event::Placeholder("rock".into(), AssetKind::Model)]
    );
}

#[test]
fn a_queued_failure_loads_the_file_over_the_placeholder() {
    let src = source(&[("rock", "broken"), ("moss", "broken")]);
    let mut resolver = resolver_for(&src);
    let world = world(&["rock", "moss"], None);
    let mut sink = Recorder::default().with_file("rock", AssetKind::Model);

    let requests = collect_requests(&world, &config(), &resolver, &sink);
    resolve(&requests, &mut resolver, &mut sink, ProcGenMode::Deferred);
    // Drain the queue: each failure is reported once, not dropped.
    let mut failures = Vec::new();
    for _ in 0..4 {
        assert!(resolver.process_frame([0.0; 3]).is_empty());
        failures.extend(resolver.take_failures());
    }
    assert_eq!(failures.len(), 2, "{failures:?}");
    apply_failures(&failures, &mut sink);

    let rock_file = Event::File("rock".into(), AssetKind::Model);
    assert!(sink.events.contains(&rock_file), "{:?}", sink.events);
    // moss has no file: its placeholder is all there is, and stays.
    assert!(!sink
        .events
        .iter()
        .any(|e| matches!(e, Event::File(n, _) | Event::Mesh(n, _) if n == "moss")));
}

#[test]
fn a_mesh_named_as_a_texture_is_refused() {
    let src = source(&[("oak_tree", "mock")]);
    let mut resolver = resolver_for(&src);
    let world = world(&[], Some("oak_tree"));
    let mut sink = Recorder::default();

    let requests = collect_requests(&world, &config(), &resolver, &sink);
    let results = resolve(&requests, &mut resolver, &mut sink, ProcGenMode::Immediate);
    let [(_, Outcome::Placeholder(reason))] = results.as_slice() else {
        panic!("{results:?}");
    };
    assert_eq!(reason, "a mesh can't be drawn as a texture");
}

#[test]
fn image_sets_name_their_maps() {
    assert_eq!(image_set_name("brick", 0, ChannelSemantics::Color), "brick");
    assert_eq!(
        image_set_name("brick", 1, ChannelSemantics::Normal),
        "brick_normal"
    );
    assert_eq!(
        image_set_name("brick", 2, ChannelSemantics::Roughness),
        "brick_roughness"
    );
}
