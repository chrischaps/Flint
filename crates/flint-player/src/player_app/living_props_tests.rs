//! Living props in the player, headless: the recording sink stands in for the
//! renderer, as it does for the rest of procgen's resolution.

use super::super::procgen_assets::tests::{Event, Recorder};
use super::*;
use flint_core::components as comp;
use flint_core::EntityId;
use flint_procgen::ProcGenSpec;

const SAPLING: &str = r#"
generator = "tree_v1"
[meta]
name = "sapling"
version = "1.0.0"
[seed]
mode = "fixed"
value = 1
[params]
branch_method = "lsystem"
lsystem_iterations = 2
leaf_density = 0.0
[living]
lineage_seed = 7
pose_ring = 4
variance = { amount = 0.5, steady = 0.5, keep_size = false }
"#;

fn no_catalog() -> ModelLoadConfig {
    ModelLoadConfig::from_scene_path("levels/grove.scene.toml")
}

fn resolver() -> ProcGenResolver {
    let mut resolver = ProcGenResolver::new();
    resolver.index_spec(ProcGenSpec::parse(SAPLING).unwrap());
    resolver
}

fn table(fields: toml::Table) -> toml::Value {
    toml::Value::Table(fields)
}

/// A world with one entity per `(name, asset, living_prop fields)`.
fn world_of(entities: &[(&str, &str, Option<toml::Table>)]) -> FlintWorld {
    let mut world = FlintWorld::new();
    for (name, asset, living) in entities {
        let id = world.spawn(*name).unwrap();
        let mut model = toml::Table::new();
        model.insert("asset".into(), toml::Value::String((*asset).into()));
        world.set_component(id, comp::MODEL, table(model)).unwrap();
        if let Some(fields) = living {
            world
                .set_component(id, comp::LIVING_PROP, table(fields.clone()))
                .unwrap();
        }
    }
    world
}

fn model_of(world: &FlintWorld, name: &str) -> toml::Value {
    let id = world.get_id(name).unwrap();
    world
        .get_components(id)
        .unwrap()
        .get(comp::MODEL)
        .unwrap()
        .clone()
}

fn id_of(world: &FlintWorld, name: &str) -> EntityId {
    world.get_id(name).unwrap()
}

fn request_of(fields: toml::Table) -> Result<LivingPropRequest, String> {
    read_request(
        EntityId::from_raw(1),
        "oak",
        [0.0; 3],
        "sapling",
        &table(fields),
    )
}

#[test]
fn a_bare_component_leaves_everything_to_the_spec() {
    let request = request_of(toml::Table::new()).unwrap();
    assert_eq!(request.entity_name, "oak");
    assert_eq!(request.spec_name, "sapling");
    assert_eq!(request.lineage_seed, None);
    assert_eq!(request.pose_ring, None);
    assert_eq!(request.phase_offset, None);
    assert_eq!(request.stagger, None);
}

#[test]
fn a_component_reads_every_field() {
    let request = request_of(toml::toml! {
        lineage_seed = 12
        pose_ring = 6
        phase_offset = 0.5
        stagger = "per_branch"
    })
    .unwrap();
    assert_eq!(request.lineage_seed, Some(12));
    assert_eq!(request.pose_ring, Some(6));
    assert_eq!(request.phase_offset, Some(0.5));
    assert_eq!(request.stagger, Some(Stagger::PerBranch));

    // An integer is a fine phase: TOML numbers can be either.
    let whole = request_of(toml::toml! { phase_offset = 1 }).unwrap();
    assert_eq!(whole.phase_offset, Some(1.0));
}

#[test]
fn a_bad_field_is_named_and_not_defaulted() {
    let error = |fields: toml::Table| request_of(fields).unwrap_err();
    assert!(error(toml::toml! { lineage_seed = -1 }).contains("lineage_seed"));
    assert!(error(toml::toml! { lineage_seed = "seven" }).contains("lineage_seed"));
    assert!(error(toml::toml! { pose_ring = 1 }).contains("pose_ring"));
    assert!(error(toml::toml! { phase_offset = "soon" }).contains("phase_offset"));
    assert!(error(toml::toml! { stagger = "sideways" }).contains("stagger"));
}

#[test]
fn immediate_mode_uploads_one_mesh_a_lineage_and_bones_a_prop() {
    let mut other = toml::Table::new();
    other.insert("lineage_seed".into(), toml::Value::Integer(8));
    let mut world = world_of(&[
        ("a", "sapling", Some(toml::Table::new())),
        ("b", "sapling", Some(toml::Table::new())),
        ("c", "sapling", Some(other)),
    ]);
    let (a, b, c) = (id_of(&world, "a"), id_of(&world, "b"), id_of(&world, "c"));
    let mut resolver = resolver();
    let mut sink = Recorder::default();
    let config = no_catalog();

    resolve(
        &mut world,
        &config,
        &mut resolver,
        &mut sink,
        ProcGenMode::Immediate,
    );

    // Each model now asks for its lineage's mesh, and nothing else will load it.
    assert_eq!(
        model_of(&world, "a").get("asset").unwrap().as_str(),
        Some("sapling@7")
    );
    assert_eq!(
        model_of(&world, "b").get("asset").unwrap().as_str(),
        Some("sapling@7")
    );
    assert_eq!(
        model_of(&world, "c").get("asset").unwrap().as_str(),
        Some("sapling@8")
    );
    assert!(flint_render::model_loader::is_runtime_model(&model_of(
        &world, "a"
    )));

    let living: Vec<_> = sink
        .events
        .iter()
        .filter(|e| matches!(e, Event::Living(_)))
        .collect();
    assert_eq!(
        living,
        [
            &Event::Living("sapling@7".into()),
            &Event::Living("sapling@8".into())
        ]
    );
    let owners: Vec<(EntityId, String)> = sink
        .events
        .iter()
        .filter_map(|e| match e {
            Event::Bones(entity, name, _) => Some((*entity, name.clone())),
            _ => None,
        })
        .collect();
    assert_eq!(
        owners,
        [
            (a, "sapling@7".to_string()),
            (b, "sapling@7".to_string()),
            (c, "sapling@8".to_string())
        ],
        "every prop gets its own bones"
    );
    // The first frame finds every ring full.
    for prop in resolver.living_props().props() {
        assert_eq!(prop.ready(), prop.ring_len());
    }
}

#[test]
fn deferred_mode_shows_a_placeholder_until_the_lineage_is_built() {
    let mut world = world_of(&[
        ("a", "sapling", Some(toml::Table::new())),
        ("b", "sapling", Some(toml::Table::new())),
    ]);
    let mut resolver = resolver();
    let mut sink = Recorder::default();
    let config = no_catalog();

    resolve(
        &mut world,
        &config,
        &mut resolver,
        &mut sink,
        ProcGenMode::Deferred,
    );
    assert_eq!(
        sink.events,
        [Event::Placeholder("sapling@7".into(), AssetKind::Model)],
        "one placeholder a lineage, and nothing generated"
    );
    assert_eq!(resolver.living_props().props()[0].ready(), 0);

    // The first frame builds the lineage and swaps it in.
    let done = resolver.process_living(1.0 / 60.0);
    assert_eq!(done.len(), 1);
    apply_lineages(&done, &mut sink);
    assert!(sink.events.contains(&Event::Living("sapling@7".into())));

    // Later frames fill the rings.
    for _ in 0..600 {
        assert!(resolver.process_living(0.0).is_empty());
    }
    for prop in resolver.living_props().props() {
        assert_eq!(prop.ready(), prop.ring_len());
    }
}

#[test]
fn a_prop_that_cannot_live_stays_a_still_model() {
    let mut bad = toml::Table::new();
    bad.insert("pose_ring".into(), toml::Value::Integer(1));
    let mut world = world_of(&[
        ("no_spec", "boulder", Some(toml::Table::new())),
        ("bad_field", "sapling", Some(bad)),
    ]);
    let mut resolver = resolver();
    let mut sink = Recorder::default();

    resolve(
        &mut world,
        &no_catalog(),
        &mut resolver,
        &mut sink,
        ProcGenMode::Immediate,
    );

    assert!(resolver.living_props().is_empty());
    assert!(sink.events.is_empty());
    for (name, asset) in [("no_spec", "boulder"), ("bad_field", "sapling")] {
        let model = model_of(&world, name);
        assert_eq!(model.get("asset").unwrap().as_str(), Some(asset));
        assert!(!flint_render::model_loader::is_runtime_model(&model));
    }
}

#[test]
fn a_catalog_entry_wins_over_living() {
    let mut world = world_of(&[("a", "sapling", Some(toml::Table::new()))]);
    let mut config = no_catalog();
    config
        .overrides
        .insert("sapling".into(), std::path::PathBuf::from("sapling.glb"));
    let mut resolver = resolver();
    let mut sink = Recorder::default();

    resolve(
        &mut world,
        &config,
        &mut resolver,
        &mut sink,
        ProcGenMode::Immediate,
    );

    assert!(resolver.living_props().is_empty());
    assert_eq!(
        model_of(&world, "a").get("asset").unwrap().as_str(),
        Some("sapling")
    );
}

#[test]
fn a_scene_with_no_living_props_touches_nothing() {
    let mut world = world_of(&[("a", "sapling", None)]);
    let before = model_of(&world, "a");
    let mut resolver = resolver();
    let mut sink = Recorder::default();

    resolve(
        &mut world,
        &no_catalog(),
        &mut resolver,
        &mut sink,
        ProcGenMode::Immediate,
    );

    assert!(resolver.living_props().is_empty());
    assert!(sink.events.is_empty());
    assert_eq!(model_of(&world, "a"), before);
    assert!(resolver.process_living(1.0 / 60.0).is_empty());
    assert_eq!(
        resolver.living_props().stats().frames,
        0,
        "no per-frame work"
    );
}

fn built_rig(spec: &str) -> std::sync::Arc<LivingRig> {
    let mut resolver = ProcGenResolver::new();
    let spec = ProcGenSpec::parse(spec).unwrap();
    let name = spec.meta.name.clone();
    resolver.index_spec(spec);
    resolver
        .add_living_prop(LivingPropRequest {
            entity: EntityId::from_raw(1),
            entity_name: "a".into(),
            position: [0.0; 3],
            spec_name: name,
            lineage_seed: None,
            pose_ring: None,
            phase_offset: None,
            stagger: None,
        })
        .unwrap();
    resolver.make_living_now().remove(0).rig.unwrap()
}

#[test]
fn an_import_holds_the_rig_part_by_part() {
    let spec = SAPLING.replace("leaf_density = 0.0", "leaf_density = 1.0\nleaf_shades = 2");
    let rig = built_rig(&spec);
    let mesh = &rig.mesh;
    assert!(mesh.submeshes.len() >= 2, "bark and leaves");

    let import = import_of("sapling@7", &rig);
    assert_eq!(import.meshes.len(), mesh.submeshes.len());

    let triangles: usize = import.meshes.iter().map(|m| m.indices.len()).sum();
    assert_eq!(triangles, mesh.indices.len());
    let vertices: usize = import.meshes.iter().map(|m| m.positions.len()).sum();
    assert_eq!(
        vertices,
        mesh.vertices.len(),
        "no part drags the whole buffer along"
    );

    for (part, sub) in import.meshes.iter().zip(&mesh.submeshes) {
        let joints = part.joint_indices.as_ref().unwrap();
        assert_eq!(joints.len(), part.positions.len());
        assert_eq!(
            part.joint_weights.as_ref().unwrap().len(),
            part.positions.len()
        );
        assert_eq!(part.skin_index, Some(0));
        // The same triangles, in the same place, with the same bones.
        let start = sub.index_start as usize;
        let lo = mesh.indices[start..start + sub.index_count as usize]
            .iter()
            .min()
            .unwrap();
        for (i, ix) in part.indices.iter().enumerate() {
            let source = &mesh.vertices[mesh.indices[start + i] as usize];
            assert_eq!(part.positions[*ix as usize], source.position);
            // A leaf card's first bone is the same; the rest are its jitter.
            assert_eq!(joints[*ix as usize][0], source.joint_indices[0]);
            assert_eq!(
                joints[*ix as usize],
                rig.runtime_joints(mesh.indices[start + i] as usize).0
            );
            assert_eq!(mesh.indices[start + i], ix + lo);
        }
        let material = &import.materials[part.material_index.unwrap()];
        assert_eq!(material.name, mesh.materials[sub.material_index].name);
    }
}

#[test]
fn an_import_carries_the_bark_normal_map_as_data() {
    let rig = built_rig(SAPLING);
    let import = import_of("sapling@7", &rig);
    let source = rig.mesh.materials.iter().find(|m| m.normal_map.is_some());
    let Some(source) = source else {
        assert!(import.textures.is_empty());
        return;
    };
    let material = import
        .materials
        .iter()
        .find(|m| m.name == source.name)
        .unwrap();
    let name = material.normal_texture.as_ref().unwrap();
    let texture = import.textures.iter().find(|t| &t.name == name).unwrap();
    let normal = source.normal_map.as_ref().unwrap();
    assert_eq!(
        (texture.width, texture.height),
        (normal.width, normal.height)
    );
    assert_eq!(texture.data, normal.pixels);
    // The texture cache's name for it: anything else loses the map.
    assert_eq!(texture.format, "rgba8");
}

#[test]
fn the_renderer_gets_the_jitter_influences_and_the_glb_does_not() {
    let spec = SAPLING.replace("leaf_density = 0.0", "leaf_density = 1.0\nleaf_shades = 2");
    let rig = built_rig(&spec);
    let import = import_of("sapling@7", &rig);
    let leaf = rig.leaf_parts[0];
    let mesh = &import.meshes[leaf];

    let sum = |w: &[f32; 4]| w.iter().sum::<f32>();
    let joints = mesh.joint_indices.as_ref().unwrap();
    let weights = mesh.joint_weights.as_ref().unwrap();
    assert!(
        joints.iter().any(|j| j[1..]
            .iter()
            .all(|&b| b as usize >= rig.bone_segments.len())),
        "no leaf vertex reaches the jitter bones"
    );
    assert!(
        weights.iter().any(|w| (sum(w) - 1.0).abs() > 1e-3),
        "a card's weights say how far it turns, so they don't sum to 1"
    );

    // The bark has no use for them, and the rig's own mesh, which bakes to a
    // GLB, is still valid glTF.
    let bark = import.meshes[0].joint_weights.as_ref().unwrap();
    assert!(bark.iter().all(|w| (sum(w) - 1.0).abs() < 1e-3));
    rig.mesh.validate().unwrap();
}

#[test]
fn a_scene_living_table_reads_as_defaults() {
    use flint_procgen::{LivingDef, LivingEchoesDef};
    use flint_procgen::{LivingDefaults, LivingShadows};

    assert_eq!(defaults_of(None), LivingDefaults::default());
    let read = defaults_of(Some(&LivingDef {
        shadows: Some("drift".into()),
        echoes: Some(LivingEchoesDef {
            count: Some(0),
            spread: None,
            alpha: Some(0.5),
        }),
        stagger: Some("per_branch".into()),
        variance: Some(flint_procgen::LivingVarianceDef {
            amount: Some(0.5),
            steady: None,
            keep_size: Some(false),
        }),
        ..Default::default()
    }));
    assert_eq!(read.stagger, Some(flint_procgen::Stagger::PerBranch));
    assert_eq!(read.variance.amount, Some(0.5));
    assert_eq!(read.variance.steady, None);
    assert_eq!(read.variance.keep_size, Some(false));
    assert_eq!(read.shadows, LivingShadows::Drift);
    assert_eq!(read.echoes.count, Some(0));
    assert_eq!(read.echoes.spread, None);
    assert_eq!(read.echoes.alpha, Some(0.5));

    // A bad knob means its default, with a warning, and doesn't stop the scene.
    let bad = defaults_of(Some(&LivingDef {
        shadows: Some("yes".into()),
        echoes: Some(LivingEchoesDef {
            count: Some(99),
            spread: Some(-1.0),
            alpha: Some(2.0),
        }),
        stagger: Some("sideways".into()),
        variance: Some(flint_procgen::LivingVarianceDef {
            amount: Some(1.5),
            steady: Some(-0.5),
            keep_size: None,
        }),
        ..Default::default()
    }));
    assert_eq!(bad, LivingDefaults::default());
}

#[test]
fn a_scene_without_wooze_has_no_field_and_one_with_it_does() {
    assert!(field_of(None).is_none());

    let scene: flint_scene::SceneFile = toml::from_str(
        r#"
[scene]
name = "grove"
[wooze]
preset = "boil"
[[wooze.zone]]
name = "broken"
shape = "sphere"
[[wooze.zone]]
name = "good"
shape = "sphere"
center = [0, 0, 0]
radius = 1
glide = 0.5
"#,
    )
    .unwrap();
    // The player goes on without what's wrong; `flint validate` is what fails.
    let field = field_of(
        flint_procgen::LivingTables::of(&scene)
            .unwrap()
            .wooze
            .as_ref(),
    )
    .unwrap();
    assert_eq!(field.zones().len(), 1);
    assert_eq!(field.zones()[0].name(), "good");
}

#[test]
fn the_wooze_line_says_how_far_apart_the_field_put_the_props() {
    let mut resolver = resolver();
    for (n, x) in [(1, 0.0), (2, 50.0)] {
        let mut request = request_of(toml::Table::new()).unwrap();
        request.entity = EntityId::from_raw(n);
        request.entity_name = format!("oak_{n}");
        request.position = [x, 0.0, 0.0];
        resolver.add_living_prop(request).unwrap();
    }
    resolver.make_living_now();
    let scene: flint_scene::SceneFile = toml::from_str(
        r#"
[scene]
name = "grove"
[wooze]
glide = 0.3
[[wooze.zone]]
name = "pocket"
shape = "sphere"
center = [0, 0, 0]
radius = 5
glide = 0.8
"#,
    )
    .unwrap();
    let field = field_of(
        flint_procgen::LivingTables::of(&scene)
            .unwrap()
            .wooze
            .as_ref(),
    )
    .unwrap();
    resolver.process_living_in(1.0 / 60.0, Some((&field, 0.0)));
    let line = wooze_report(&field, resolver.living_props());
    assert!(line.contains("1 zones over 2 props"), "{line}");
    assert!(line.contains("glide 0.30..0.80"), "{line}");
}

#[test]
fn the_report_says_how_it_went() {
    let stats = LivingStats {
        frames: 3600,
        frames_full: 3590,
        most_short: 1,
        frames_starved: 0,
        poses_made: 4800,
        poses_from_cache: 2,
        pose_failures: 0,
        total_ms: 7200.0,
        worst_ms: 3.5,
        frames_over_budget: 0,
        blend_frames: 3600,
        blend_total_ms: 1800.0,
        blend_worst_ms: 0.9,
        bone_writes: 72000,
        ..Default::default()
    };
    let [rings, poses, blending] = report(&stats, 40, 5, 8, 4.0);
    assert!(blending.contains("avg 0.500 ms/frame"), "{blending}");
    assert!(blending.contains("over 1 ms on 0 frames"), "{blending}");
    assert!(blending.contains("20.0 bone writes a frame"), "{blending}");
    assert!(rings.contains("40 props on 5 lineages"), "{rings}");
    assert!(rings.contains("3590 of 3600 frames"), "{rings}");
    assert!(poses.contains("avg 2.00 ms/frame"), "{poses}");
    assert!(poses.contains("over it on 0 frames"), "{poses}");
}

/// The shading boil's clock (#214) keeps the field's time: it runs in a scene with
/// a field, stands still with `dt` at 0 (animation paused), and never starts in a
/// scene with none, which is what keeps that scene's passes from boiling.
#[test]
fn the_boil_clock_runs_with_the_field_and_only_with_it() {
    let mut app = super::super::PlayerApp::new(
        flint_ecs::FlintWorld::new(),
        "boil_test.scene.toml".into(),
        false,
        None,
        None,
    );
    app.tick_procgen(1.0);
    assert_eq!(app.living.boil_clock.drawing(1), 0, "no field, no clock");

    app.living.wooze = Some(WoozeField::new(flint_core::wooze::WoozeSample {
        step: 0.25,
        ..flint_core::wooze::WoozeSample::NEUTRAL
    }));
    app.tick_procgen(0.5);
    // Half a second is two beats, twelve film frames, six drawings on twos.
    assert_eq!(app.living.boil_clock.drawing(2), 6);
    app.tick_procgen(0.0);
    assert_eq!(
        app.living.boil_clock.drawing(2),
        6,
        "a paused clock holds its drawing"
    );
}
