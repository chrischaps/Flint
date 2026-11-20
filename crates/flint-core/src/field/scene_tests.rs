use super::*;
use crate::field::{FieldKeys, ParamField};

fn value(src: &str) -> toml::Value {
    toml::Value::Table(toml::from_str(src).unwrap())
}

fn table(src: &str) -> toml::Table {
    toml::from_str(src).unwrap()
}

/// The issue's own example (#1014): two keys and a zone.
const WEATHER_SCENE: &str = r#"
[weather]
keys = { wind = { min = 0, default = 1 }, fog = { min = 0, max = 1, default = 0 } }

[[weather.zone]]
name = "storm"
shape = "sphere"
center = [20, 0, 0]
radius = 8
falloff = 4
wind = 6
fog = 0.5
"#;

fn weather() -> SceneFields {
    let (fields, issues) = SceneFields::read(&value(WEATHER_SCENE));
    assert_eq!(issues, Vec::new());
    fields
}

#[test]
fn a_declared_field_reads_its_keys_and_zone() {
    let fields = weather();
    assert_eq!(fields.names(), ["weather"]);
    let field = fields.get("weather").unwrap();
    let names: Vec<&str> = field.keys().iter().map(|k| k.name.as_str()).collect();
    // toml's tables are sorted by key
    assert_eq!(names, ["fog", "wind"]);
    assert_eq!(field.zones().len(), 1);
    assert_eq!(field.zones()[0].name(), "storm");

    let at = |x: f32, key: &str| field.sample_key(key, [x, 0.0, 0.0], 0.0).unwrap();
    assert_eq!(at(0.0, "wind"), 1.0, "the default, out of the zone");
    assert_eq!(at(0.0, "fog"), 0.0);
    assert_eq!(at(20.0, "wind"), 6.0, "the zone's, inside it");
    assert_eq!(at(26.0, "fog"), 0.5, "still inside the radius");
    let edge = at(10.0, "wind");
    assert!(edge > 1.0 && edge < 6.0, "in the falloff: {edge}");
    assert_eq!(at(7.9, "wind"), 1.0, "past the falloff");
    assert_eq!(field.sample_key("rain", [0.0; 3], 0.0), None);
    assert_eq!(field.zones_in([20.0, 0.0, 0.0], 0.0), [("storm", 1.0)]);
}

/// The same keys as a compile-time field, `wind` and `fog` at 0 or more.
static PARAM_WEATHER: FieldKeys<2> = FieldKeys {
    field: "weather",
    names: ["fog", "wind"],
    check: |_, v| {
        if v >= 0.0 && v.is_finite() {
            Ok(v)
        } else {
            Err("negative".to_string())
        }
    },
    preset: None,
};

/// A scene field is the general field over a list: the same zones, ramps
/// and falloff give the same bits, key by key, at every place and time.
#[test]
fn a_scene_field_samples_the_bits_a_param_field_does() {
    let mut scene = SceneField::new(
        "weather",
        vec![
            SceneKey {
                name: "fog".into(),
                min: Some(0.0),
                max: None,
                default: 0.25,
            },
            SceneKey {
                name: "wind".into(),
                min: Some(0.0),
                max: None,
                default: 1.0,
            },
        ],
    );
    let mut param = ParamField::new(&PARAM_WEATHER, [0.25, 1.0]);
    let zones = [
        "name = \"a\"\nshape = \"sphere\"\ncenter = [0, 0, 0]\nradius = 3\nfalloff = 5\nwind = 4",
        "name = \"b\"\nshape = \"box\"\ncenter = [4, 1, 0]\nsize = [2, 3, 4]\nfalloff = 2.5\nfog = 0.9\nwind = 0.2",
    ];
    for zone in zones {
        scene.add_zone(&table(zone)).unwrap();
        param.add_zone(&table(zone)).unwrap();
    }
    // A ramp of a global value, a zone reshaped and eased, one faded in and
    // one faded out, each made the same way in both
    let global = table("wind = 2.5");
    scene.set_global(&global, 1.0, 3.0).unwrap();
    param.set_global(&global, 1.0, 3.0).unwrap();
    let reshape = table("fog = 0.1\nradius = 9\nshape = \"sphere\"");
    scene.set_zone("b", &reshape, 2.0, 1.5).unwrap();
    param.set_zone("b", &reshape, 2.0, 1.5).unwrap();
    let new = table("shape = \"sphere\"\ncenter = [-3, 0, 2]\nradius = 1\nfalloff = 6\nfog = 0.6");
    scene.set_zone("c", &new, 2.5, 2.0).unwrap();
    param.set_zone("c", &new, 2.5, 2.0).unwrap();
    assert!(scene.remove_zone("a", 3.0, 1.0));
    assert!(param.remove_zone("a", 3.0, 1.0));
    for step in 0..120 {
        let time = f64::from(step) * 0.05;
        for x in -10..=10 {
            for y in [-1.0, 0.0, 2.0] {
                let position = [x as f32 * 0.7, y, 0.3];
                let expected = param.sample(position, time);
                for (i, expected) in expected.iter().enumerate() {
                    let got = scene.sample_index(i, position, time);
                    assert_eq!(
                        got.to_bits(),
                        expected.to_bits(),
                        "key {i} at {position:?}, t = {time}"
                    );
                }
            }
        }
    }
}

#[test]
fn a_key_may_be_a_bare_default_and_an_unsaid_default_is_zero_or_its_range() {
    let (fields, issues) = SceneFields::read(&value(
        "[slow]\nkeys = { scale = 0.5, drag = { min = 2 }, lift = { max = -1 }, heat = {} }",
    ));
    assert_eq!(issues, Vec::new());
    let field = fields.get("slow").unwrap();
    let key = |name: &str| field.keys()[field.index(name).unwrap()].clone();
    assert_eq!(key("scale").default, 0.5);
    assert_eq!((key("scale").min, key("scale").max), (None, None));
    assert_eq!(key("drag").default, 2.0);
    assert_eq!(key("lift").default, -1.0);
    assert_eq!(key("heat").default, 0.0);
}

/// Every mistake in the table, each by its field and key or zone, with the
/// zone's messages the wooze field's own.
#[test]
fn every_mistake_is_named_by_its_key_or_zone() {
    let (fields, issues) = SceneFields::read(&value(
        r#"
[weather]
keys = { wind = { min = 0, default = 1 }, fog = { min = 0, max = 1, default = 0 } }
wind = 3

[[weather.zone]]
name = "storm"
shape = "sphere"
center = [0, 0, 0]
radius = 4
rain = 2

[[weather.zone]]
shape = "box"
center = [0, 0, 0]
size = [1, 1, 1]
fog = 2

[[weather.zone]]
name = "calm"
shape = "sphere"
center = [9, 0, 0]
radius = 2
wind = 0

[bad]
keys = { center = 1, drag = { min = 3, max = 1 }, heat = { mn = 0 }, glow = { max = 1, default = 2 }, hue = "red" }

[empty]
keys = {}

[lonely]
"#,
    ));
    let said: Vec<String> = issues.iter().map(ToString::to_string).collect();
    assert_eq!(
        said,
        [
            "[field.bad] key 'center': 'center' can't be a key: a zone's name, shape, center, radius, size, falloff takes that name",
            "[field.bad] key 'drag': min = 3 is more than max = 1",
            "[field.bad] key 'glow': its default is out of range: glow = 2 must be 1 or less",
            "[field.bad] key 'heat': 'mn' isn't a key's setting; it takes min, max, default",
            "[field.bad] key 'hue': hue = \"red\" must be a number or a table of min, max and default",
            "[field.empty]: keys = {} declares no keys",
            "[field.lonely]: it needs its keys: keys = { wind = { min = 0, default = 1 } }",
            "[field.weather]: 'wind' isn't a field setting; it takes keys and zone (a key's value everywhere is its default)",
            "[field.weather] zone 'storm': 'rain' isn't a zone key; it takes name, shape, center, radius, size, falloff, fog, wind",
            "[field.weather] zone 'zone 2': fog = 2 must be from 0 to 1",
        ]
    );
    assert_eq!(issues[0].place(), "bad.keys.center");
    assert_eq!(issues[8].place(), "weather.zone.storm");
    assert_eq!(issues[5].place(), "empty");
    // A field with bad keys is left out; a bad zone is, and its field kept
    assert_eq!(fields.names(), ["weather"]);
    let weather = fields.get("weather").unwrap();
    let zones: Vec<&str> = weather.zones().iter().map(|z| z.name()).collect();
    assert_eq!(zones, ["calm"]);
}

#[test]
fn a_field_table_that_is_not_one_says_so() {
    let (fields, issues) = SceneFields::read(&toml::Value::Integer(3));
    assert!(fields.is_empty());
    assert_eq!(
        issues[0].to_string(),
        "[field]: it holds one table a field: [field.<name>] keys = { .. }"
    );
    let (_, issues) = SceneFields::read(&value("weather = 3"));
    assert_eq!(
        issues[0].to_string(),
        "[field.weather]: 3 must be a table: [field.weather] keys = { .. }"
    );
    let (_, issues) = SceneFields::read(&value("[weather]\nkeys = { wind = 1 }\nzone = 2"));
    assert_eq!(
        issues[0].to_string(),
        "[field.weather]: zone = 2 must be a list of zones: [[field.weather.zone]]"
    );
}

#[test]
fn a_script_change_is_checked_against_the_key_and_eases() {
    let mut fields = weather();
    let field = fields.get_mut("weather").unwrap();
    assert_eq!(
        field.set_global(&table("fog = 3"), 0.0, 0.0),
        Err("fog = 3 must be from 0 to 1".to_string())
    );
    assert_eq!(
        field.set_global(&table("rain = 1"), 0.0, 0.0),
        Err("'rain' isn't a weather key; it takes fog, wind".to_string())
    );
    field.set_global(&table("wind = 3"), 10.0, 2.0).unwrap();
    let wind = field.index("wind").unwrap();
    assert_eq!(field.global(wind, 10.0), 1.0);
    assert_eq!(field.global(wind, 11.0), 2.0, "half way, smoothstepped");
    assert_eq!(field.global(wind, 12.0), 3.0);

    field
        .set_zone(
            "gust",
            &table("shape = \"box\"\ncenter = [0, 0, 0]\nsize = [2, 2, 2]\nwind = 9"),
            20.0,
            1.0,
        )
        .unwrap();
    assert_eq!(field.sample_index(wind, [0.0; 3], 20.0), 3.0, "fading in");
    assert_eq!(field.sample_index(wind, [0.0; 3], 21.0), 9.0);
    assert!(field.remove_zone("gust", 21.0, 0.0));
    assert!(!field.remove_zone("gust", 21.0, 0.0));
}

/// What a sample costs (#1014: "pick by measuring"): one key of a scene's
/// field against `ParamField::sample_key` with the same two keys and three
/// zones, at places in and out of them. `cargo test -p flint-core --release
/// --lib sample_cost -- --ignored --nocapture`.
#[test]
#[ignore]
fn sample_cost() {
    let zones = [
        "name = \"a\"\nshape = \"sphere\"\ncenter = [0, 0, 0]\nradius = 3\nfalloff = 5\nwind = 4",
        "name = \"b\"\nshape = \"box\"\ncenter = [4, 1, 0]\nsize = [2, 3, 4]\nfalloff = 2.5\nfog = 0.9",
        "name = \"c\"\nshape = \"sphere\"\ncenter = [-6, 0, 2]\nradius = 2\nwind = 0.5",
    ];
    let key = |name: &str, default: f32| SceneKey {
        name: name.into(),
        min: Some(0.0),
        max: None,
        default,
    };
    let mut scene = SceneField::new("weather", vec![key("fog", 0.25), key("wind", 1.0)]);
    let mut param = ParamField::new(&PARAM_WEATHER, [0.25, 1.0]);
    for zone in zones {
        scene.add_zone(&table(zone)).unwrap();
        param.add_zone(&table(zone)).unwrap();
    }
    const N: usize = 4_000_000;
    let places: Vec<[f32; 3]> = (0..64).map(|i| [i as f32 * 0.3 - 9.0, 0.5, 0.2]).collect();
    let time = |label: &str, f: &dyn Fn([f32; 3]) -> f32| {
        let start = std::time::Instant::now();
        let mut sum = 0.0f32;
        for i in 0..N {
            sum += f(std::hint::black_box(places[i % places.len()]));
        }
        let ns = start.elapsed().as_nanos() as f64 / N as f64;
        println!("{label}: {ns:.1} ns a sample (sum {sum})");
    };
    let wind = scene.index("wind").unwrap();
    time("SceneField::sample_index", &|p| {
        scene.sample_index(wind, p, 0.5)
    });
    time("SceneField::sample_key  ", &|p| {
        scene.sample_key("wind", p, 0.5).unwrap()
    });
    time("ParamField::sample_key  ", &|p| {
        param.sample_key("wind", p, 0.5).unwrap()
    });
}
