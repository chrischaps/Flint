//! Procgen resolution order (#32), headless: a recording sink stands in for
//! the renderer and the specs live in a `MemorySource`, the way a bundle
//! brings them on the web.

use super::*;
use flint_core::vfs::{self, MemorySource};
use flint_procgen::{GenerationCost, Generator, OutputKind, ProcGenSpec, SeededRng};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// What the renderer was asked to do, in order.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Event {
    Mesh(String, usize),
    Image(String),
    Placeholder(String, AssetKind),
    File(String, AssetKind),
    Living(String),
    Bones(EntityId, String, usize),
}

#[derive(Default)]
pub(crate) struct Recorder {
    pub(crate) meshes: HashMap<String, usize>,
    files: HashSet<(String, bool)>,
    pub(crate) events: Vec<Event>,
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
    fn upload_living(&mut self, name: &str, rig: &LivingRig) {
        self.meshes.insert(name.into(), rig.mesh.vertices.len());
        self.events.push(Event::Living(name.into()));
    }
    fn upload_bones(&mut self, entity: EntityId, name: &str, palette: &[[[f32; 4]; 4]]) {
        self.events
            .push(Event::Bones(entity, name.into(), palette.len()));
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
    vfs::scoped(src.clone(), || discover(&mut resolver, SCENE));
    resolver
}

/// `discover_specs` in the project the scene is found in.
fn discover(resolver: &mut ProcGenResolver, scene: &str) {
    let project = flint_core::project::Project::for_scene(Path::new(scene));
    discover_specs(resolver, &project, scene);
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
fn the_scenes_own_spec_beats_the_games() {
    let mut src = MemorySource::new();
    src.insert("game/specs/a.procgen.toml", spec("shared", "mock"));
    src.insert("game/levels/specs/b.procgen.toml", spec("shared", "broken"));
    // The scene's own spec wins over the game-wide one of the same name,
    // whatever their files are called.
    let mut resolver = ProcGenResolver::new();
    resolver.register_generator(Box::new(Broken));
    vfs::scoped(Arc::new(src), || discover(&mut resolver, SCENE));
    assert!(resolver.check_generator("shared").is_ok());
    assert!(
        resolver.generate("shared").is_err(),
        "the broken one, from beside the scene"
    );
}

#[test]
fn a_scene_two_folders_deep_finds_the_projects_specs() {
    // Before the one resolver a spec was looked for beside the scene and
    // one level up, so a scene at levels/act1/cave/ saw none of the game's.
    let mut src = MemorySource::new();
    src.insert("game/flint.toml", "");
    src.insert("game/specs/oak.procgen.toml", spec("oak_tree", "mock"));
    src.insert("game/specs/props/crate.procgen.toml", spec("crate", "mock"));
    src.insert("game/models/rock.procgen.toml", spec("rock", "mock"));
    src.insert(
        "game/engine/specs/pebble.procgen.toml",
        spec("pebble", "mock"),
    );
    let mut resolver = ProcGenResolver::new();
    vfs::scoped(Arc::new(src), || {
        discover(&mut resolver, "game/levels/act1/cave/a.scene.toml")
    });
    for name in ["oak_tree", "crate", "rock", "pebble"] {
        assert!(resolver.has_spec(name), "{name}");
    }
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

/// With no `living_prop` to move it, a living spec in a scene is a still tree.
#[test]
fn a_skinned_mesh_draws_holding_still() {
    let mesh = flint_procgen::SkinnedMeshData {
        vertices: Vec::new(),
        indices: Vec::new(),
        materials: Vec::new(),
        submeshes: Vec::new(),
        bounding_box: flint_procgen::BoundingBox::from_positions(&[]),
        skeleton: flint_procgen::SkeletonDef { bones: Vec::new() },
    };
    let mut sink = Recorder::default();
    upload_output(
        "oak",
        AssetKind::Model,
        &GeneratorOutput::SkinnedMesh(mesh.clone()),
        &mut sink,
    )
    .unwrap();
    assert!(sink.meshes.contains_key("oak"));

    let refused = upload_output(
        "oak",
        AssetKind::Texture,
        &GeneratorOutput::SkinnedMesh(mesh),
        &mut sink,
    );
    assert_eq!(
        refused.unwrap_err(),
        "a skinned mesh can't be drawn as a texture"
    );
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

/// A game's own generator, which no built-in shares a name with: one
/// triangle, so its three vertices say whose mesh was uploaded.
struct Rock;
impl Generator for Rock {
    fn type_name(&self) -> &str {
        "rock_v1"
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
        rng: &mut SeededRng,
    ) -> flint_procgen::Result<GeneratorOutput> {
        let top = 1.0 + rng.next_f32();
        let vertex = |position: [f32; 3]| flint_procgen::Vertex {
            position,
            normal: [0.0, 0.0, 1.0],
            tangent: [1.0, 0.0, 0.0, 1.0],
            uv: [0.0, 0.0],
        };
        let vertices = vec![
            vertex([-1.0, 0.0, 0.0]),
            vertex([1.0, 0.0, 0.0]),
            vertex([0.0, top, 0.0]),
        ];
        let positions: Vec<[f32; 3]> = vertices.iter().map(|v| v.position).collect();
        Ok(GeneratorOutput::Mesh(MeshData {
            vertices,
            indices: vec![0, 1, 2],
            materials: vec![Default::default()],
            submeshes: vec![],
            bounding_box: flint_procgen::BoundingBox::from_positions(&positions),
        }))
    }
    fn param_schema(&self) -> serde_json::Value {
        serde_json::json!({ "type": "object", "properties": {} })
    }
}

/// The game's extension: no script API, no frame hook, only its generator.
struct CaveGame;
impl crate::PlayerExtension for CaveGame {
    fn register_generators(&mut self, registry: &mut flint_procgen::GeneratorRegistry) {
        registry.register(Box::new(Rock));
    }
}

fn cave_game() -> crate::PlayerApp {
    let mut app = crate::PlayerApp::new(FlintWorld::new(), SCENE.into(), false, None, None);
    app.add_extension(Box::new(CaveGame));
    app
}

/// Resolve `models` through the app's own resolver, as a scene load does.
fn load(app: &mut crate::PlayerApp, src: &Arc<MemorySource>, models: &[&str]) -> Recorder {
    vfs::scoped(src.clone(), || discover(&mut app.procgen_resolver, SCENE));
    let world = world(models, None);
    let mut sink = Recorder::default();
    let requests = collect_requests(&world, &config(), &app.procgen_resolver, &sink);
    resolve(
        &requests,
        &mut app.procgen_resolver,
        &mut sink,
        ProcGenMode::Immediate,
    );
    sink
}

#[test]
fn a_games_generator_resolves_its_specs_in_the_player() {
    // #195's done-when: the game's binary registers `rock_v1` through its
    // extension, a spec in the game names it, and the scene's entity gets
    // the generated mesh. Nothing in the engine knows the name.
    let src = source(&[("cave_rock", "rock_v1"), ("oak_tree", "mock")]);
    let mut plain = crate::PlayerApp::new(FlintWorld::new(), SCENE.into(), false, None, None);
    let without = load(&mut plain, &src, &["cave_rock"]);
    assert_eq!(
        without.events,
        vec![Event::Placeholder("cave_rock".into(), AssetKind::Model)],
        "the stock player has no rock_v1"
    );

    let mut app = cave_game();
    let sink = load(&mut app, &src, &["cave_rock", "oak_tree"]);
    assert_eq!(
        sink.events,
        vec![
            Event::Mesh("cave_rock".into(), 3),
            Event::Mesh("oak_tree".into(), 24),
        ],
        "the game's generator, and the built-ins beside it"
    );
}

#[test]
fn a_games_generators_survive_the_disk_tier() {
    // The player rebuilds its resolver when it opens the disk cache, after
    // extensions are added: the game's generator has to move across.
    let mut app = cave_game();
    let dir = std::env::temp_dir().join(format!("flint-195-disk-{}", std::process::id()));
    app.use_procgen_disk(flint_procgen::DiskCacheConfig::new(&dir));
    assert!(app.procgen_resolver.disk_stats().is_some(), "now on disk");
    assert!(app.procgen_resolver.registry().get("rock_v1").is_some());
    assert!(app.procgen_resolver.registry().get("tree_v1").is_some());

    let src = source(&[("cave_rock", "rock_v1")]);
    let sink = load(&mut app, &src, &["cave_rock"]);
    assert_eq!(sink.events, vec![Event::Mesh("cave_rock".into(), 3)]);
    #[allow(clippy::disallowed_methods)] // a test's own temp folder
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_games_generator_can_replace_a_built_in() {
    // Registered under `mock`'s name, the game's generator is the one used.
    struct MockRock;
    impl crate::PlayerExtension for MockRock {
        fn register_generators(&mut self, registry: &mut flint_procgen::GeneratorRegistry) {
            struct Renamed(Rock);
            impl Generator for Renamed {
                fn type_name(&self) -> &str {
                    "mock"
                }
                fn revision(&self) -> u32 {
                    self.0.revision()
                }
                fn output_kind(&self) -> OutputKind {
                    self.0.output_kind()
                }
                fn generate(
                    &self,
                    spec: &ProcGenSpec,
                    rng: &mut SeededRng,
                ) -> flint_procgen::Result<GeneratorOutput> {
                    self.0.generate(spec, rng)
                }
                fn param_schema(&self) -> serde_json::Value {
                    self.0.param_schema()
                }
            }
            registry.register(Box::new(Renamed(Rock)));
        }
    }
    let mut app = crate::PlayerApp::new(FlintWorld::new(), SCENE.into(), false, None, None);
    app.add_extension(Box::new(MockRock));
    let sink = load(&mut app, &source(&[("oak_tree", "mock")]), &["oak_tree"]);
    assert_eq!(sink.events, vec![Event::Mesh("oak_tree".into(), 3)]);
}
