use super::*;
use crate::generator::LivingGenerator;

const LINEAGE: u64 = 11;

fn example() -> ProcGenSpec {
    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../specs/examples/form_living.procgen.toml"
    );
    ProcGenSpec::from_file(std::path::Path::new(path)).unwrap()
}

fn with_params(params: toml::Value) -> ProcGenSpec {
    let mut spec = example();
    spec.params = params;
    spec
}

fn table(spec: &ProcGenSpec) -> toml::map::Map<String, toml::Value> {
    spec.params.as_table().unwrap().clone()
}

/// Everything a spec makes for one lineage, as bits: the rig and two poses.
fn digest(spec: &ProcGenSpec) -> u64 {
    let mut digest = 0xcbf2_9ce4_8422_2325u64;
    let mut mix = |bits: u32| {
        digest ^= bits as u64;
        digest = digest.wrapping_mul(0x0100_0000_01b3);
    };
    let rig = FormGenerator.rig(spec, LINEAGE).unwrap();
    for v in &rig.mesh.vertices {
        v.position
            .iter()
            .chain(&v.normal)
            .chain(&v.joint_weights)
            .for_each(|c| mix(c.to_bits()));
        v.joint_indices.iter().for_each(|j| mix(*j as u32));
    }
    rig.mesh.indices.iter().for_each(|i| mix(*i));
    for m in &rig.mesh.materials {
        m.base_color.iter().for_each(|c| mix(c.to_bits()));
        mix(m.roughness.to_bits());
    }
    let skeleton = FormGenerator.structure(spec, LINEAGE).unwrap();
    for s in &skeleton.segments {
        mix(s.depth);
    }
    for shape_seed in [1, 2] {
        let pose = FormGenerator.pose(spec, &skeleton, shape_seed).unwrap();
        for s in &pose.segments {
            s.rotation.iter().for_each(|c| mix(c.to_bits()));
            mix(s.length_scale.to_bits());
            mix(s.radius_scale.to_bits());
            s.slide.iter().for_each(|c| mix(c.to_bits()));
        }
    }
    digest
}

fn digest_in_new_process() -> String {
    let test = "generators::form::tests::the_same_spec_and_seed_give_the_same_bits_across_runs";
    let out = std::process::Command::new(std::env::current_exe().unwrap())
        .args(["--exact", test, "--nocapture"])
        .env("FLINT_FORM_PRINT_DIGEST", "1")
        .output()
        .unwrap();
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .find_map(|l| l.strip_prefix("digest=").map(str::to_owned))
        .unwrap_or_else(|| panic!("no digest in {out:?}"))
}

#[test]
fn the_same_spec_and_seed_give_the_same_bits_across_runs() {
    let spec = example();
    if std::env::var_os("FLINT_FORM_PRINT_DIGEST").is_some() {
        println!("digest={:x}", digest(&spec));
        return;
    }
    let here = format!("{:x}", digest(&spec));
    assert_eq!(here, format!("{:x}", digest(&spec)));
    assert_eq!(here, digest_in_new_process());
}

#[test]
fn another_lineage_is_another_form() {
    let spec = example();
    let a = FormGenerator.rig(&spec, 1).unwrap();
    let b = FormGenerator.rig(&spec, 2).unwrap();
    assert_ne!(a.mesh.vertices, b.mesh.vertices);
}

#[test]
fn poses_differ_and_hold_the_form_together() {
    let spec = example();
    let skeleton = FormGenerator.structure(&spec, LINEAGE).unwrap();
    let a = FormGenerator.pose(&spec, &skeleton, 1).unwrap();
    let b = FormGenerator.pose(&spec, &skeleton, 2).unwrap();
    assert_ne!(a, b);
    for pose in [&a, &b] {
        let posed = skeleton.posed_segments(pose).unwrap();
        // Joined, and the base where it stands.
        assert_eq!(posed[0].start, skeleton.segments[0].start);
        for w in posed.windows(2) {
            assert!((w[1].start - w[0].end).length() < 1e-5);
        }
        // Held: a pose leans the tip, it doesn't throw it across the stage.
        let tip = posed.last().unwrap().end;
        let rest_tip = skeleton.segments.last().unwrap().end;
        assert!(
            (tip - rest_tip).length() < 0.5,
            "tip moved {}",
            (tip - rest_tip).length()
        );
    }
}

#[test]
fn the_rig_is_valid_and_its_body_faces_out() {
    let spec = example();
    let rig = FormGenerator.rig(&spec, LINEAGE).unwrap();
    rig.mesh.validate().unwrap();
    for v in &rig.mesh.vertices {
        let n = Vec3::from_array(v.normal);
        assert!((n.length() - 1.0).abs() < 1e-3, "normal {n:?}");
        assert!(v.position.iter().all(|c| c.is_finite()));
    }
    // Each body triangle's winding agrees with its vertices' normals.
    let body = &rig.mesh.submeshes[0];
    let at = |i: u32| Vec3::from_array(rig.mesh.vertices[i as usize].position);
    let (mut agree, mut total) = (0, 0);
    for tri in rig.mesh.indices[..body.index_count as usize].chunks(3) {
        let face = (at(tri[1]) - at(tri[0])).cross(&(at(tri[2]) - at(tri[0])));
        if face.length() < 1e-9 {
            continue;
        }
        let normal = Vec3::from_array(rig.mesh.vertices[tri[0] as usize].normal);
        total += 1;
        agree += (face.dot(&normal) > 0.0) as usize;
    }
    assert!(
        agree as f32 > 0.99 * total as f32,
        "{agree} of {total} face out"
    );
}

#[test]
fn cards_are_two_sided_and_each_binds_once() {
    let spec = example();
    let skeleton = FormGenerator.structure(&spec, LINEAGE).unwrap();
    let cards = FormParams::from_toml(&spec.params).unwrap().cards as usize;
    assert_eq!(skeleton.leaf_bindings.len(), cards);
    assert_eq!(skeleton.leaf_mesh.indices.len(), cards * CARD_INDICES);
    assert_eq!(skeleton.leaf_mesh.vertices.len(), cards * CARD_VERTICES);
    for k in [0, cards / 2, cards - 1] {
        let verts = skeleton.card_vertices(k);
        assert_eq!(verts.len(), CARD_VERTICES);
        let front = Vec3::from_array(skeleton.leaf_mesh.vertices[verts[0] as usize].normal);
        let back = Vec3::from_array(skeleton.leaf_mesh.vertices[verts[4] as usize].normal);
        assert!((front + back).length() < 1e-5);
    }
    // Each face is wound to face its normal: counter-clockwise seen from it.
    let mesh = &skeleton.leaf_mesh;
    let at = |i: u32| Vec3::from_array(mesh.vertices[i as usize].position);
    for tri in mesh.indices.chunks(3).take(64) {
        let face = (at(tri[1]) - at(tri[0])).cross(&(at(tri[2]) - at(tri[0])));
        let normal = Vec3::from_array(mesh.vertices[tri[0] as usize].normal);
        assert!(face.dot(&normal) > 0.0);
    }
}

#[test]
fn no_cards_is_a_smooth_form() {
    let mut t = table(&example());
    t.insert("cards".into(), toml::Value::Integer(0));
    let spec = with_params(toml::Value::Table(t));
    let rig = FormGenerator.rig(&spec, LINEAGE).unwrap();
    assert_eq!(rig.first_leaf_vertex, rig.mesh.vertices.len());
    assert!(FormGenerator
        .pose(&spec, &FormGenerator.structure(&spec, LINEAGE).unwrap(), 3)
        .is_ok());
}

#[test]
fn empty_params_take_the_defaults() {
    let spec = with_params(toml::Value::Table(Default::default()));
    assert_eq!(
        FormParams::from_toml(&spec.params).unwrap(),
        FormParams::default()
    );
    let skeleton = FormGenerator.structure(&spec, LINEAGE).unwrap();
    assert_eq!(skeleton.segments.len(), 10);
    FormGenerator
        .rig(&spec, LINEAGE)
        .unwrap()
        .mesh
        .validate()
        .unwrap();
}

#[test]
fn the_most_segments_fit_the_palette() {
    let mut t = table(&example());
    t.insert("segments".into(), toml::Value::Integer(64));
    let spec = with_params(toml::Value::Table(t));
    let rig = FormGenerator.rig(&spec, LINEAGE).unwrap();
    assert!(rig.bone_segments.len() + BOIL_BONES <= MAX_SKINNED_BONES);
}

#[test]
fn bad_values_are_refused_by_name() {
    let cases: &[(&str, toml::Value)] = &[
        ("height", 0.0.into()),
        ("aspect", 0.0.into()),
        ("aspect", 1.5.into()),
        ("curl_deg", 400.0.into()),
        ("segments", 1.into()),
        ("waves", 0.into()),
        ("waves", 40.into()),
        ("rings", 2.into()),
        ("card_tilt", (-0.1).into()),
        ("card_from", 0.99.into()),
        ("card_shades", 9.into()),
        ("swell", 1.0.into()),
        ("profile", toml::Value::Array(vec![0.5.into()])),
        ("profile", toml::Value::Array(vec![0.0.into(), 0.0.into()])),
        (
            "profile",
            toml::Value::Array(vec![1.0.into(), (-1.0).into()]),
        ),
        ("body_color", "not a colour".into()),
        ("body_opacity", 1.5.into()),
        ("card_layout", "grid".into()),
        ("card_layout", 1.into()),
        ("row_faces", "left".into()),
        ("row_margin", 0.95.into()),
        ("row_spacing", 0.0.into()),
    ];
    for (key, value) in cases {
        let mut t = table(&example());
        t.insert((*key).into(), value.clone());
        match FormParams::from_toml(&toml::Value::Table(t)) {
            Err(ProcGenError::InvalidParameter { name, .. }) => {
                assert_eq!(name, *key, "{key} = {value}")
            }
            other => panic!("{key} = {value}: {other:?}"),
        }
    }
}

/// A base where every key has something to act on: a curl for `curl_bias`, a
/// coil for `coil_turns`, a flat section for `section_deg`, a turn for poses.
fn nudge_base() -> toml::map::Map<String, toml::Value> {
    let mut t = table(&example());
    for (key, value) in [
        ("coil", 0.05),
        ("aspect", 0.6),
        ("turn_deg", 10.0),
        ("twist_turns", 0.2),
        ("lift", 0.02),
        ("section_deg", 10.0),
        ("facing_deg", 10.0),
    ] {
        t.insert(key.into(), value.into());
    }
    t.insert("cards".into(), 200.into());
    t.insert("rings".into(), 40.into());
    t.insert("sides".into(), 24.into());
    t
}

/// The nudge base written in rows, on one face, so the row keys have
/// something to act on.
fn rows_base() -> toml::map::Map<String, toml::Value> {
    let mut t = nudge_base();
    t.insert("card_layout".into(), "rows".into());
    t.insert("row_faces".into(), "front".into());
    t.insert("aspect".into(), 0.1.into());
    t.insert("card_size".into(), 0.012.into());
    t.insert("card_aspect".into(), 4.0.into());
    t.insert("cards".into(), 2000.into());
    t
}

/// Every key the schema lists changes what's made, and nothing else is read:
/// a nudge to each moves the digest, in scattered cards or in rows (some keys
/// only mean something in one of them).
#[test]
fn the_schema_says_exactly_which_keys_the_form_reads() {
    let schema = FormGenerator.param_schema();
    let props = schema["properties"].as_object().unwrap();
    let bases = [nudge_base(), rows_base()];
    let befores: Vec<u64> = bases
        .iter()
        .map(|b| digest(&with_params(toml::Value::Table(b.clone()))))
        .collect();
    let defaults = FormParams::default();
    let mut idle = Vec::new();
    for (key, prop) in props {
        let moved = bases.iter().zip(&befores).any(|(base, before)| {
            let current = base.get(key).cloned().unwrap_or_else(|| {
                serde_json::from_value::<toml::Value>(prop["default"].clone())
                    .unwrap_or_else(|_| panic!("{key}: default"))
            });
            let nudged = match (&current, prop.get("enum")) {
                (toml::Value::String(s), Some(names)) => names
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|n| n.as_str().unwrap())
                    .find(|n| n != s)
                    .unwrap()
                    .into(),
                (toml::Value::Integer(n), _) => toml::Value::Integer(n - 1),
                (toml::Value::Float(x), _) if *x == 0.0 => toml::Value::Float(0.05),
                (toml::Value::Float(x), _) => toml::Value::Float(x * 0.9),
                (toml::Value::String(s), _) => toml::Value::String(if s.ends_with('0') {
                    format!("{}8", &s[..s.len() - 1])
                } else {
                    format!("{}0", &s[..s.len() - 1])
                }),
                (toml::Value::Array(a), _) => {
                    let mut a = a.clone();
                    a[1] = toml::Value::Float(a[1].as_float().unwrap() * 0.8);
                    toml::Value::Array(a)
                }
                other => panic!("{key}: can't nudge {other:?}"),
            };
            let mut t = base.clone();
            t.insert(key.clone(), nudged);
            digest(&with_params(toml::Value::Table(t))) != *before
        });
        if !moved {
            idle.push(key.clone());
        }
    }
    assert!(idle.is_empty(), "keys that changed nothing: {idle:?}");
    // And the schema's defaults are the code's.
    for (key, prop) in props {
        let mut t = toml::map::Map::new();
        t.insert(
            key.clone(),
            serde_json::from_value::<toml::Value>(prop["default"].clone()).unwrap(),
        );
        assert_eq!(
            FormParams::from_toml(&toml::Value::Table(t)).unwrap(),
            defaults,
            "{key}'s default"
        );
    }
}

fn rows_spec(faces: &str) -> ProcGenSpec {
    let mut t = rows_base();
    t.insert("row_faces".into(), faces.into());
    t.insert("lobes".into(), 0.0.into());
    t.insert("twist_turns".into(), 0.0.into());
    // A page's outline, so every line has room (a flame's tip has none).
    t.insert(
        "profile".into(),
        toml::Value::Array(vec![1.0.into(), 1.0.into(), 1.0.into()]),
    );
    t.insert("round_base".into(), 0.02.into());
    t.insert("round_tip".into(), 0.02.into());
    // Upright, broad side square to the curl's plane: a line is level.
    t.insert("curl_deg".into(), 0.0.into());
    t.insert("lean".into(), 0.0.into());
    t.insert("coil".into(), 0.0.into());
    t.insert("section_deg".into(), 90.0.into());
    with_params(toml::Value::Table(t))
}

/// Each card's centre and front normal, in the rest pose.
fn card_centres(spec: &ProcGenSpec) -> Vec<(Vec3, Vec3)> {
    let mesh = FormGenerator.structure(spec, LINEAGE).unwrap().leaf_mesh;
    mesh.vertices
        .chunks(CARD_VERTICES)
        .map(|card| {
            let centre = card[..4]
                .iter()
                .fold(Vec3::ZERO, |s, v| s + Vec3::from_array(v.position))
                * 0.25;
            (centre, Vec3::from_array(card[0].normal))
        })
        .collect()
}

#[test]
fn rows_write_one_face_and_stop_short_of_the_edge() {
    let spec = rows_spec("front");
    let p = FormParams::from_toml(&spec.params).unwrap();
    let lin = body::Lineage::new(&mut SeededRng::new(LINEAGE).fork("lineage"));
    let spine = body::Spine::new(&p, &lin);
    let cards = card_centres(&spec);
    assert!(cards.len() > 200, "{} cards", cards.len());
    assert!(cards.len() <= p.cards as usize);
    // Every card faces the same way as the first: one face written.
    let first = cards[0].1;
    assert!(cards.iter().all(|(_, n)| n.dot(&first) > 0.5));
    // None reaches the margin: the outline has no marks standing out of it.
    for (centre, _) in &cards {
        let (near, f) = (0..=200)
            .map(|i| i as f32 / 200.0)
            .map(|u| (u, spine.frame(u)))
            .min_by(|a, b| {
                (a.1.centre - *centre)
                    .length()
                    .total_cmp(&(b.1.centre - *centre).length())
            })
            .unwrap();
        let half = body::radius_along(&p, &lin, near, 0.0);
        let off = (*centre - f.centre).length();
        assert!(
            off <= half * (1.0 - p.row_margin) + 0.02,
            "a card {off} m out where the margin is at {}",
            half * (1.0 - p.row_margin)
        );
    }
    // Both faces is about twice the writing, back to back.
    let both = card_centres(&rows_spec("both"));
    assert!(both.len() as f32 > 1.6 * cards.len() as f32);
    assert!(both.iter().any(|(_, n)| n.dot(&first) < -0.5));
}

#[test]
fn rows_fall_into_lines() {
    let spec = rows_spec("front");
    let p = FormParams::from_toml(&spec.params).unwrap();
    let mut heights: Vec<f32> = card_centres(&spec).iter().map(|(c, _)| c.y).collect();
    heights.sort_by(f32::total_cmp);
    // Cards fall into lines: between two lines there's a gap of most of a
    // line's spacing, and inside one, nearly none.
    let gaps: Vec<f32> = heights.windows(2).map(|w| w[1] - w[0]).collect();
    let lines = gaps
        .iter()
        .filter(|g| **g > 0.5 * p.row_spacing * 0.8)
        .count()
        + 1;
    let expected = ((p.card_to - p.card_from) * p.height / p.row_spacing) as usize;
    assert!(
        lines + 3 >= expected && lines <= expected + 1,
        "{lines} lines, about {expected} expected"
    );
}

#[test]
fn a_slide_moves_the_form_along_its_lines_and_changes_nothing_else() {
    let spec = example();
    let skeleton = FormGenerator.structure(&spec, LINEAGE).unwrap();
    let with = |slide: f32, reflow: f32| {
        let mut t = table(&spec);
        t.insert("slide".into(), f64::from(slide).into());
        t.insert("slide_reflow".into(), f64::from(reflow).into());
        with_params(toml::Value::Table(t))
    };
    let whole = with(0.1, 0.0);
    let built = build(&whole, LINEAGE).unwrap();
    let reach = 0.1 * amplitude(&whole);
    let n = skeleton.segments.len();
    for seed in [1, 2, 3] {
        let plain = FormGenerator.full_pose(&spec, &skeleton, seed).unwrap();
        let slid = FormGenerator.full_pose(&whole, &skeleton, seed).unwrap();
        // The slide is drawn last, so the rest of the pose is the same bits,
        // and a spec without one poses as it did before there was a slide.
        for (a, b) in plain.segments.iter().zip(&slid.segments) {
            assert_eq!(a.slide, [0.0; 3]);
            assert_eq!(
                (a.rotation, a.length_scale, a.radius_scale),
                (b.rotation, b.length_scale, b.radius_scale)
            );
        }
        // With no reflow, one move for the whole form, along the lines.
        let size = Vec3::from_array(slid.segments[0].slide).length();
        assert!(
            size > 0.0 && size <= reach + 1e-6,
            "slid {size}, at most {reach}"
        );
        for (i, s) in slid.segments.iter().enumerate() {
            let axis = body::broad_axis(&built.p, &built.spine, (i as f32 + 0.5) / n as f32);
            let v = Vec3::from_array(s.slide);
            assert!(
                (v - axis * v.dot(&axis)).length() < 1e-6,
                "segment {i} slides off its lines"
            );
            assert!(
                (v.length() - size).abs() < 1e-5,
                "segment {i} slides {} not {size}",
                v.length()
            );
        }
        // Each segment is where it would have been, moved by its own slide
        // and no other: a slide doesn't carry to the segment's children.
        let (a, b) = (
            skeleton.posed_segments(&plain).unwrap(),
            skeleton.posed_segments(&slid).unwrap(),
        );
        for (i, ((x, y), s)) in a.iter().zip(&b).zip(&slid.segments).enumerate() {
            let s = Vec3::from_array(s.slide);
            assert!(
                (y.start - x.start - s).length() < 1e-5,
                "segment {i}'s start"
            );
            assert!((y.end - x.end - s).length() < 1e-5, "segment {i}'s end");
        }
    }
    // With reflow the lines at one height go one way and at another the other.
    let reflow = with(0.1, 1.0);
    let pose = FormGenerator.full_pose(&reflow, &skeleton, 1).unwrap();
    let sizes: Vec<f32> = pose
        .segments
        .iter()
        .map(|s| Vec3::from_array(s.slide).length())
        .collect();
    let (least, most) = sizes
        .iter()
        .fold((f32::MAX, 0.0f32), |(l, m), s| (l.min(*s), m.max(*s)));
    assert!(
        most - least > 0.01,
        "a reflowing slide should change up the form: {sizes:?}"
    );
}

#[test]
fn keep_size_doesnt_read_a_slide_as_stretch() {
    let mut t = table(&example());
    t.insert("slide".into(), 0.3.into());
    t.insert("slide_reflow".into(), 1.0.into());
    let slid = with_params(toml::Value::Table(t));
    let mut t = table(&example());
    t.insert("slide".into(), 0.0.into());
    let plain = with_params(toml::Value::Table(t));
    let skeleton = FormGenerator.structure(&plain, LINEAGE).unwrap();
    for seed in [1, 2, 3] {
        let a = FormGenerator.pose(&plain, &skeleton, seed).unwrap();
        let b = FormGenerator.pose(&slid, &skeleton, seed).unwrap();
        for (x, y) in a.segments.iter().zip(&b.segments) {
            assert_eq!(x.length_scale, y.length_scale);
        }
    }
}

#[test]
fn body_opacity_is_the_body_materials_alpha() {
    let mut t = table(&example());
    t.insert("body_opacity".into(), 0.25.into());
    let spec = with_params(toml::Value::Table(t));
    let rig = FormGenerator.rig(&spec, LINEAGE).unwrap();
    let body = &rig.mesh.materials[rig.mesh.submeshes[0].material_index];
    assert_eq!(body.base_color[3], 0.25);
    // The cards stay solid.
    for sub in &rig.mesh.submeshes[1..] {
        assert_eq!(rig.mesh.materials[sub.material_index].base_color[3], 1.0);
    }
}

#[test]
fn the_registry_finds_the_living_side() {
    let mut registry = crate::GeneratorRegistry::new();
    crate::register_built_in_generators(&mut registry);
    let generator = registry.get("form_v1").unwrap();
    let living = generator.as_living().unwrap();
    living.check_living(&example()).unwrap();
}

#[test]
fn generate_bakes_the_lineage_rest() {
    let spec = example();
    let mut rng = SeededRng::new(5);
    let GeneratorOutput::SkinnedMesh(mesh) = FormGenerator.generate(&spec, &mut rng).unwrap()
    else {
        panic!("a living spec bakes its skinned rest");
    };
    assert_eq!(mesh, FormGenerator.rig(&spec, LINEAGE).unwrap().mesh);

    let mut plain = spec.clone();
    plain.living = None;
    let GeneratorOutput::Mesh(mesh) = FormGenerator.generate(&plain, &mut rng).unwrap() else {
        panic!("a plain spec bakes a mesh");
    };
    assert_eq!(mesh, FormGenerator.rig(&plain, 5).unwrap().mesh.rest_mesh());
}
