use super::*;
use crate::format::{BoilDef, LivingEchoesDef, LivingVarianceDef, WoozeKnobs};
use crate::SceneFile;

const GROVE: &str = r#"# A grove, by hand.
[scene]
name = "grove"

[wooze]
preset = "linklater"
step = 0.50   # slower than the preset

[[wooze.zone]]
name = "hush"
shape = "sphere"
center = [0, 0, 0]
radius = 4
step = 1.0

[living]
# Every tree, unless its species says otherwise.
echoes = { count = 2, alpha = 0.2 }

[living.species.grove_thorn]
step = 0.25
echoes = { count = 1 }   # thorns barely echo

[post_process]
kuwahara_boil = 0.5

[entities.oak_1]
archetype = "living_tree"

[entities.oak_1.model]
asset = "grove_oak"
"#;

/// The look `GROVE` gives, in the shape the player reads back.
fn grove_look() -> WoozePreset {
    let scene: SceneFile = toml::from_str(GROVE).unwrap();
    let wooze = scene.wooze.unwrap();
    WoozePreset {
        wooze: WoozeKnobs {
            step: wooze.step,
            ..Default::default()
        },
        living: scene.living.unwrap(),
        boil: Some(BoilDef {
            enabled: Some(true),
            kuwahara: Some(0.5),
            contour: Some(0.0),
            on: Some(2),
        }),
    }
}

fn commit(text: &str, base: &WoozePreset, now: &WoozePreset) -> String {
    let mut doc = SceneDocument::from_str(text).unwrap();
    let mut edits = look_edits(base, now);
    if !doc.has_table(&["post_process"]) {
        edits.retain(|edit| edit.table != ["post_process"]);
    }
    for edit in &edits {
        doc.apply_edit(edit).unwrap();
    }
    doc.to_string()
}

/// The lines of `after` that aren't the lines of `before`, both ways, as a diff
/// would show them.
fn changed(before: &str, after: &str) -> (Vec<String>, Vec<String>) {
    let was: Vec<&str> = before.lines().collect();
    let is: Vec<&str> = after.lines().collect();
    let gone = (was.iter())
        .filter(|line| !is.contains(line))
        .map(|line| line.to_string())
        .collect();
    let new = (is.iter())
        .filter(|line| !was.contains(line))
        .map(|line| line.to_string())
        .collect();
    (gone, new)
}

fn thorn(look: &mut WoozePreset) -> &mut LivingSpeciesDef {
    look.living.species.entry("grove_thorn".into()).or_default()
}

#[test]
fn nothing_changed_writes_nothing() {
    assert!(look_edits(&grove_look(), &grove_look()).is_empty());
    assert_eq!(commit(GROVE, &grove_look(), &grove_look()), GROVE);
}

#[test]
fn one_species_value_changes_exactly_its_line_and_keeps_its_comment() {
    let mut now = grove_look();
    thorn(&mut now).echoes.as_mut().unwrap().count = Some(4);
    let after = commit(GROVE, &grove_look(), &now);
    let (gone, new) = changed(GROVE, &after);
    assert_eq!(gone, ["echoes = { count = 1 }   # thorns barely echo"]);
    assert_eq!(new, ["echoes = { count = 4 }   # thorns barely echo"]);
    assert_eq!(GROVE.lines().count(), after.lines().count());
}

#[test]
fn a_value_that_reads_the_same_is_left_as_it_was_written() {
    // 0.50 in the file is 0.5 in the look: nothing to write, even if another
    // value in the same table changes.
    let mut now = grove_look();
    now.wooze.amplitude = Some(0.3);
    let after = commit(GROVE, &grove_look(), &now);
    assert!(
        after.contains("step = 0.50   # slower than the preset"),
        "{after}"
    );
    let (gone, new) = changed(GROVE, &after);
    assert!(gone.is_empty(), "{gone:?}");
    assert_eq!(new, ["amplitude = 0.3"]);
}

#[test]
fn a_new_field_goes_into_the_inline_table_that_holds_its_neighbours() {
    let mut now = grove_look();
    thorn(&mut now).echoes.as_mut().unwrap().spread = Some(1.0 / 3.0);
    let after = commit(GROVE, &grove_look(), &now);
    let scene: SceneFile = toml::from_str(&after).unwrap();
    let echoes = scene.living.unwrap().species["grove_thorn"].echoes.unwrap();
    assert_eq!(echoes.spread, Some(1.0 / 3.0), "the same bits back");
    assert_eq!(echoes.count, Some(1));
    let (gone, new) = changed(GROVE, &after);
    assert_eq!(gone.len(), 1, "{gone:?}");
    assert!(new[0].starts_with("echoes = { count = 1"), "{new:?}");
}

#[test]
fn a_new_species_table_sits_with_the_others_and_brings_no_empty_living() {
    let mut now = grove_look();
    now.living.species.insert(
        "grove_birch".into(),
        LivingSpeciesDef {
            glide: Some(0.75),
            variance: Some(LivingVarianceDef {
                keep_size: Some(true),
                ..Default::default()
            }),
            ..Default::default()
        },
    );
    let after = commit(GROVE, &grove_look(), &now);
    let birch = after.find("[living.species.grove_birch]").expect(&after);
    assert!(birch > after.find("[living.species.grove_thorn]").unwrap());
    assert!(birch < after.find("[post_process]").unwrap(), "{after}");
    assert!(
        after.contains("glide = 0.75\nvariance = { keep_size = true }"),
        "{after}"
    );
    let scene: SceneFile = toml::from_str(&after).unwrap();
    assert_eq!(scene.living.unwrap().species, now.living.species);
}

#[test]
fn a_scene_with_no_living_gets_one_after_its_wooze_not_after_its_entities() {
    let text = "[scene]\nname = \"bare\"\n\n[wooze]\nstep = 0.4\n\n[[wooze.zone]]\nname = \"a\"\nshape = \"sphere\"\ncenter = [0, 0, 0]\nradius = 1\n\n[entities.oak]\narchetype = \"x\"\n";
    let base = WoozePreset {
        wooze: WoozeKnobs {
            step: Some(0.4),
            ..Default::default()
        },
        ..Default::default()
    };
    let mut now = base.clone();
    thorn(&mut now).step = Some(0.125);
    let after = commit(text, &base, &now);
    assert!(!after.contains("[living]\n"), "no empty [living]: {after}");
    let species = after.find("[living.species.grove_thorn]").expect(&after);
    assert!(species > after.find("[[wooze.zone]]").unwrap(), "{after}");
    assert!(species < after.find("[entities.oak]").unwrap(), "{after}");
    let scene: SceneFile = toml::from_str(&after).unwrap();
    assert_eq!(
        scene.living.unwrap().species["grove_thorn"].step,
        Some(0.125)
    );
}

#[test]
fn a_cleared_value_is_taken_out_and_a_species_left_with_none_goes() {
    let mut now = grove_look();
    thorn(&mut now).step = None;
    let after = commit(GROVE, &grove_look(), &now);
    let (gone, new) = changed(GROVE, &after);
    assert_eq!(gone, ["step = 0.25"]);
    assert!(new.is_empty(), "{new:?}");

    now.living.species.remove("grove_thorn");
    let after = commit(GROVE, &grove_look(), &now);
    assert!(!after.contains("grove_thorn"), "{after}");
    assert!(!after.contains("thorns barely echo"), "{after}");
    let scene: SceneFile = toml::from_str(&after).unwrap();
    assert!(scene.living.unwrap().species.is_empty());
}

#[test]
fn a_cleared_echo_field_takes_its_inline_table_with_it_once_empty() {
    let mut now = grove_look();
    thorn(&mut now).echoes = None;
    let after = commit(GROVE, &grove_look(), &now);
    let (gone, new) = changed(GROVE, &after);
    assert_eq!(gone, ["echoes = { count = 1 }   # thorns barely echo"]);
    assert!(new.is_empty(), "{new:?}");
}

#[test]
fn global_values_land_in_wooze_and_living_and_the_boil_in_post_process() {
    let mut now = grove_look();
    now.living.stagger = Some("per_level".into());
    now.living.echoes = Some(LivingEchoesDef {
        count: Some(2),
        alpha: Some(0.35),
        spread: None,
    });
    now.boil = Some(BoilDef {
        enabled: Some(false),
        ..now.boil.unwrap()
    });
    let after = commit(GROVE, &grove_look(), &now);
    let scene: SceneFile = toml::from_str(&after).unwrap();
    let living = scene.living.unwrap();
    assert_eq!(living.stagger.as_deref(), Some("per_level"));
    assert_eq!(living.echoes.unwrap().alpha, Some(0.35));
    assert_eq!(scene.post_process.unwrap().kuwahara_boil, 0.0, "off is 0");
    assert!(after.contains("# Every tree, unless its species says otherwise."));
}

#[test]
fn every_edit_names_its_table_and_key() {
    let mut now = grove_look();
    thorn(&mut now).echoes.as_mut().unwrap().count = Some(4);
    thorn(&mut now).step = None;
    let said: Vec<String> = (look_edits(&grove_look(), &now).iter())
        .map(|edit| edit.to_string())
        .collect();
    assert_eq!(
        said,
        [
            "[living.species.grove_thorn] echoes.count = 4",
            "[living.species.grove_thorn] step taken out",
        ]
    );
    let edits = look_edits(&grove_look(), &now);
    assert!(edits.iter().all(|e| e.species() == Some("grove_thorn")));
}

/// A folder of its own, gone when dropped.
struct Folder(std::path::PathBuf);

impl Folder {
    fn new(name: &str) -> Self {
        let dir = std::env::temp_dir().join(format!("flint_commit_{name}_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        Self(dir)
    }
}

impl Drop for Folder {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[test]
fn a_commit_writes_the_file_and_says_what_a_preset_still_sets() {
    let folder = Folder::new("preset");
    let path = folder.0.join("grove.scene.toml");
    let text = GROVE.replace("preset = \"linklater\"", "preset = \"misty\"");
    std::fs::write(&path, &text).unwrap();
    let preset = WoozePreset {
        living: LivingDef {
            species: [(
                "grove_thorn".to_string(),
                LivingSpeciesDef {
                    jitter: Some(0.4),
                    ..Default::default()
                },
            )]
            .into(),
            ..Default::default()
        },
        ..Default::default()
    };
    crate::write_preset(&path, "misty", &preset).unwrap();

    // As loaded, the thorns have the preset's jitter under the scene's own.
    let mut base = grove_look();
    thorn(&mut base).jitter = Some(0.4);
    let mut now = base.clone();
    thorn(&mut now).jitter = None;
    thorn(&mut now).glide = Some(0.5);
    let report = commit_look(&path, &base, &now).unwrap();
    assert_eq!(report.edits.len(), 2, "{:?}", report.edits);
    assert_eq!(
        report.preset_still_sets,
        ["[living.species.grove_thorn] jitter"]
    );
    let written = std::fs::read_to_string(&path).unwrap();
    let (gone, new) = changed(&text, &written);
    assert!(gone.is_empty(), "{gone:?}");
    assert_eq!(new, ["glide = 0.5"]);
}

#[test]
fn with_no_post_process_the_boil_is_left_and_said() {
    let folder = Folder::new("noboil");
    let path = folder.0.join("grove.scene.toml");
    let text = GROVE.replace("[post_process]\nkuwahara_boil = 0.5\n", "");
    std::fs::write(&path, &text).unwrap();
    let mut now = grove_look();
    now.boil.as_mut().unwrap().kuwahara = Some(0.9);
    let report = commit_look(&path, &grove_look(), &now).unwrap();
    assert!(report.boil_skipped);
    assert!(report.edits.is_empty());
    assert_eq!(std::fs::read_to_string(&path).unwrap(), text);
}
