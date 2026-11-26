//! A game's own procgen generators through its extension (ADR 0119, #195):
//! `PlayerExtension::resources`, and the procgen package's registry in the
//! run's resources (#1557). These run where the run's launcher lists the
//! procgen package, which the stock one does in a build with `procgen`; in
//! one without, there is no registry to add to and they pass over.

use flint_core::components as comp;
use flint_core::vfs::{self, MemorySource};
use flint_ecs::FlintWorld;
use flint_procgen::player::recorder::{Event, Recorder};
use flint_procgen::player::{collect_requests, discover_specs, resolve, ProcGenMode};
use flint_procgen::{
    AssetKind, Generator, GeneratorOutput, MeshData, OutputKind, ProcGenSpec, SeededRng,
};
use flint_render::model_loader::ModelLoadConfig;
use std::path::Path;
use std::sync::Arc;

const SCENE: &str = "game/levels/grove.scene.toml";

fn with_procgen() -> bool {
    crate::packages::registry().has("procgen")
}

/// Specs under `game/specs/` (one directory up from the scene, as games keep them).
fn source(specs: &[(&str, &str)]) -> Arc<MemorySource> {
    let mut src = MemorySource::new();
    src.insert(
        SCENE,
        "[scene]
name = \"grove\"
",
    );
    for (name, generator) in specs {
        src.insert(
            format!("game/specs/{name}.procgen.toml"),
            format!(
                "generator = \"{generator}\"
[meta]
name = \"{name}\"
version = \"1.0.0\"
                 [seed]
mode = \"fixed\"
value = 42
"
            ),
        );
    }
    Arc::new(src)
}

/// A world of entities naming `model.asset`s.
fn world(models: &[&str]) -> FlintWorld {
    let mut world = FlintWorld::new();
    for (i, asset) in models.iter().enumerate() {
        let id = world.spawn(format!("thing_{i}")).unwrap();
        let mut model = toml::map::Map::new();
        model.insert("asset".into(), toml::Value::String(asset.to_string()));
        world.set_component(id, comp::MODEL, model.into()).unwrap();
    }
    world
}

fn resolver(app: &mut crate::PlayerApp) -> &mut flint_procgen::ProcGenResolver {
    flint_procgen::player::resolver_mut(&mut app.core.resources)
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
    fn resources(&mut self, resources: &mut flint_package::Resources) {
        if let Some(generators) = flint_procgen::player::generators_mut(resources) {
            generators.register(Box::new(Rock));
        }
    }
}

fn cave_game() -> crate::PlayerApp {
    let mut app = crate::PlayerApp::new(FlintWorld::new(), SCENE.into(), false, None, None);
    app.add_extension(Box::new(CaveGame));
    app
}

/// Resolve `models` through the app's own resolver, as a scene load does.
fn load(app: &mut crate::PlayerApp, src: &Arc<MemorySource>, models: &[&str]) -> Recorder {
    vfs::scoped(src.clone(), || {
        let project = flint_core::project::Project::for_scene(Path::new(SCENE));
        discover_specs(resolver(app), &project, SCENE)
    });
    let world = world(models);
    let mut sink = Recorder::default();
    let config = ModelLoadConfig::from_scene_path(SCENE);
    let requests = collect_requests(&world, &config, resolver(app), &sink);
    resolve(&requests, resolver(app), &mut sink, ProcGenMode::Immediate);
    sink
}

#[test]
fn a_games_generator_resolves_its_specs_in_the_player() {
    // #195's done-when: the game's binary registers `rock_v1` through its
    // extension, a spec in the game names it, and the scene's entity gets
    // the generated mesh. Nothing in the engine knows the name.
    if !with_procgen() {
        return;
    }
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
    if !with_procgen() {
        return;
    }
    let mut app = cave_game();
    let dir = std::env::temp_dir().join(format!("flint-195-disk-{}", std::process::id()));
    flint_procgen::player::use_disk(
        resolver(&mut app),
        flint_procgen::DiskCacheConfig::new(&dir),
    );
    assert!(resolver(&mut app).disk_stats().is_some(), "now on disk");
    assert!(resolver(&mut app).registry().get("rock_v1").is_some());
    assert!(resolver(&mut app).registry().get("tree_v1").is_some());

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
    if !with_procgen() {
        return;
    }
    impl crate::PlayerExtension for MockRock {
        fn resources(&mut self, resources: &mut flint_package::Resources) {
            let Some(registry) = flint_procgen::player::generators_mut(resources) else {
                return;
            };
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
