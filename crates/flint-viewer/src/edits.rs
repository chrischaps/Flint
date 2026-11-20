//! What the human changed in the viewer, and what of it is saved (#679).
//!
//! Every field edit goes through [`FieldEdits::set`]: the inspector's, a
//! gizmo drag's, undo's and redo's. The first time a field is touched after
//! a load or a save, `set` keeps the value it had, so the scene is dirty
//! exactly when some field now differs from that value: undo after a save
//! makes it dirty again, and undoing back to what the file says makes it
//! clean. A save writes only those fields, through
//! [`SceneDocument::write_entity_field`] (an entity a prefab makes goes into
//! its instance's overrides), and each one written is a line appended to the
//! project's `.flint/viewer-edits.jsonl`, so an agent can read what the human
//! changed without diffing the file.

use flint_core::EntityId;
use flint_ecs::FlintWorld;
use flint_scene::{FieldHome, SceneDocument};
use std::collections::BTreeMap;
use std::io::Write as _;
use std::path::{Path, PathBuf};

/// (entity name, component, field)
pub type FieldKey = (String, String, String);

/// Where the change list goes, under the project root.
pub const EDIT_LOG: &str = ".flint/viewer-edits.jsonl";

/// The fields edited since the file was read or last written.
#[derive(Debug, Default)]
pub struct FieldEdits {
    /// Each touched field and the value it had when the file was last read
    /// or written (`None`: the entity didn't have it)
    saved: BTreeMap<FieldKey, Option<toml::Value>>,
    /// Why a field couldn't be written at the last save
    problems: BTreeMap<FieldKey, String>,
}

/// A field that differs from what the file was last given.
#[derive(Debug, Clone, PartialEq)]
pub struct FieldChange {
    pub key: FieldKey,
    pub old: Option<toml::Value>,
    pub new: Option<toml::Value>,
}

/// A save's fields: the ones written into the document (and where), and the
/// ones that couldn't be, with why.
#[derive(Debug, Default)]
pub struct SavePlan {
    pub written: Vec<(FieldChange, FieldHome)>,
    pub failed: Vec<(FieldKey, String)>,
}

impl FieldEdits {
    /// Set a field in the world, remembering what it was if this is the
    /// field's first edit since the file was written. Nothing happens for an
    /// entity the world doesn't have.
    pub fn set(
        &mut self,
        world: &mut FlintWorld,
        id: EntityId,
        component: &str,
        field: &str,
        value: toml::Value,
    ) {
        let Some(name) = world.get_name(id) else {
            return;
        };
        let key = (name.to_string(), component.to_string(), field.to_string());
        let Some(mut components) = world.get_components_mut(id) else {
            return;
        };
        let current = components.get_field(component, field).cloned();
        self.saved.entry(key).or_insert(current);
        components.set_field(component, field, value);
    }

    /// The fields that now differ from what the file was last given, in
    /// (entity, component, field) order.
    pub fn changes(&self, world: &FlintWorld) -> Vec<FieldChange> {
        self.saved
            .iter()
            .filter_map(|(key, old)| {
                let new = current(world, key);
                (new != *old).then(|| FieldChange {
                    key: key.clone(),
                    old: old.clone(),
                    new,
                })
            })
            .collect()
    }

    /// Whether the world differs from the file.
    pub fn is_dirty(&self, world: &FlintWorld) -> bool {
        self.saved
            .iter()
            .any(|(key, old)| current(world, key) != *old)
    }

    /// Why each field that the last save couldn't write wasn't, while it
    /// still differs from the file.
    pub fn problems(&self, world: &FlintWorld) -> Vec<String> {
        self.problems
            .iter()
            .filter(|(key, _)| {
                self.saved
                    .get(*key)
                    .is_some_and(|old| current(world, key) != *old)
            })
            .map(|(_, why)| why.clone())
            .collect()
    }

    /// Write every changed field into `doc`. The edits don't count as saved
    /// until [`saved`](Self::saved) is called with the plan, once the
    /// document is on disk.
    pub fn plan(&self, world: &FlintWorld, doc: &mut SceneDocument) -> SavePlan {
        let mut plan = SavePlan::default();
        for change in self.changes(world) {
            let (entity, component, field) = &change.key;
            match doc.write_entity_field(entity, component, field, change.new.as_ref()) {
                Ok(home) => plan.written.push((change, home)),
                Err(why) => plan.failed.push((change.key, why)),
            }
        }
        plan
    }

    /// The plan's document is on disk: what it wrote is what the file says
    /// now, and what it couldn't write is still unsaved, with why.
    pub fn saved(&mut self, plan: &SavePlan) {
        for (change, _) in &plan.written {
            self.saved.remove(&change.key);
            self.problems.remove(&change.key);
        }
        for (key, why) in &plan.failed {
            self.problems.insert(key.clone(), why.clone());
        }
        // A field edited and put back is no change: forget it
        self.saved.retain(|key, _| self.problems.contains_key(key));
    }

    /// The file was read anew (an external change): nothing is edited.
    pub fn clear(&mut self) {
        self.saved.clear();
        self.problems.clear();
    }
}

fn current(world: &FlintWorld, (entity, component, field): &FieldKey) -> Option<toml::Value> {
    let id = world.get_id(entity)?;
    world
        .get_components(id)?
        .get_field(component, field)
        .cloned()
}

/// The change list's path for a scene: `.flint/viewer-edits.jsonl` under
/// the scene's project root.
pub fn edit_log_path(scene_path: &Path) -> PathBuf {
    let root = flint_core::project::Project::for_path(scene_path)
        .map(|p| p.root)
        .unwrap_or_else(|_| {
            scene_path
                .parent()
                .map(Path::to_path_buf)
                .unwrap_or_default()
        });
    root.join(EDIT_LOG)
}

/// The scene as the log names it: its path from the project root (the
/// folder `.flint/` is in), with `/`, or as given if it's elsewhere.
pub fn scene_in_log(log: &Path, scene_path: &str) -> String {
    let scene = Path::new(scene_path);
    let root = log.parent().and_then(Path::parent);
    let canonical = |p: &Path| std::fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf());
    root.and_then(|root| {
        canonical(scene)
            .strip_prefix(canonical(root))
            .ok()
            .map(Path::to_path_buf)
    })
    .unwrap_or_else(|| scene.to_path_buf())
    .to_string_lossy()
    .replace('\\', "/")
}

/// One JSON line a written field: when, which scene, the field, the value
/// the file had and the one it has now (`null`: none), and the table it
/// went into.
pub fn log_lines(plan: &SavePlan, scene: &str, saved_at_unix: u64) -> String {
    let mut out = String::new();
    for (change, home) in &plan.written {
        let (entity, component, field) = &change.key;
        let line = serde_json::json!({
            "saved_at": saved_at_unix,
            "scene": scene,
            "entity": entity,
            "component": component,
            "field": field,
            "old": change.old.as_ref().map(to_json),
            "new": change.new.as_ref().map(to_json),
            "table": home.table(entity, component),
        });
        out.push_str(&line.to_string());
        out.push('\n');
    }
    out
}

/// A key of the scene's `[post_process]` that F4's Commit to scene changed
/// (#1046): the table it's in (`post_process`,
/// `post_process.styles.ascii`), the key, and its value before and after
/// (`None`: the table didn't have it).
#[derive(Debug, Clone, PartialEq)]
pub struct PostChange {
    pub table: String,
    pub key: String,
    pub old: Option<toml::Value>,
    pub new: Option<toml::Value>,
}

/// Every `[post_process]` key that differs between two texts of a scene,
/// in (table, key) order. A table that isn't a value (`styles.<name>`) is
/// walked into, so a style's key is named in its own table. A text that
/// doesn't parse reads as no `[post_process]`.
pub fn post_process_changes(before: &str, after: &str) -> Vec<PostChange> {
    fn leaves(text: &str) -> BTreeMap<(String, String), toml::Value> {
        fn walk(
            table: &toml::Table,
            path: &str,
            out: &mut BTreeMap<(String, String), toml::Value>,
        ) {
            for (key, value) in table {
                match value {
                    toml::Value::Table(inner) => walk(inner, &format!("{path}.{key}"), out),
                    _ => {
                        out.insert((path.to_string(), key.clone()), value.clone());
                    }
                }
            }
        }
        let mut out = BTreeMap::new();
        let parsed: Option<toml::Table> = toml::from_str(text).ok();
        if let Some(post) = parsed
            .as_ref()
            .and_then(|t| t.get("post_process"))
            .and_then(|p| p.as_table())
        {
            walk(post, "post_process", &mut out);
        }
        out
    }
    let (old, new) = (leaves(before), leaves(after));
    let mut keys: Vec<&(String, String)> = old.keys().chain(new.keys()).collect();
    keys.sort();
    keys.dedup();
    keys.into_iter()
        .filter(|k| old.get(*k) != new.get(*k))
        .map(|k| PostChange {
            table: k.0.clone(),
            key: k.1.clone(),
            old: old.get(k).cloned(),
            new: new.get(k).cloned(),
        })
        .collect()
}

/// One JSON line a key a Commit to scene wrote, in a save's shape (#1046):
/// `entity` null, `component` `post_process`, the key as `field`, and the
/// table it went into.
pub fn commit_log_lines(changes: &[PostChange], scene: &str, saved_at_unix: u64) -> String {
    let mut out = String::new();
    for change in changes {
        let line = serde_json::json!({
            "saved_at": saved_at_unix,
            "scene": scene,
            "entity": serde_json::Value::Null,
            "component": "post_process",
            "field": change.key,
            "old": change.old.as_ref().map(to_json),
            "new": change.new.as_ref().map(to_json),
            "table": change.table,
        });
        out.push_str(&line.to_string());
        out.push('\n');
    }
    out
}

/// Append the plan's lines to the log, making `.flint/` if it isn't there.
#[allow(clippy::disallowed_methods)] // a write, which the vfs doesn't do
pub fn append_log(path: &Path, lines: &str) -> std::io::Result<()> {
    if lines.is_empty() {
        return Ok(());
    }
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)?
        .write_all(lines.as_bytes())
}

fn to_json(value: &toml::Value) -> serde_json::Value {
    match value {
        toml::Value::String(s) => s.clone().into(),
        toml::Value::Integer(i) => (*i).into(),
        toml::Value::Float(f) => serde_json::Number::from_f64(*f)
            .map(serde_json::Value::Number)
            .unwrap_or_else(|| f.to_string().into()),
        toml::Value::Boolean(b) => (*b).into(),
        toml::Value::Datetime(d) => d.to_string().into(),
        toml::Value::Array(a) => a.iter().map(to_json).collect(),
        toml::Value::Table(t) => t
            .iter()
            .map(|(k, v)| (k.clone(), to_json(v)))
            .collect::<serde_json::Map<_, _>>()
            .into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SCENE: &str = r#"[scene]
name = "Edits"

[prefabs.props]
template = "stylize_props"
prefix = "props"

[entities.crate]
archetype = "furniture"

[entities.crate.transform]
position = [0.0, 0.0, 0.0] # by the door
"#;

    fn vec3(x: f64, y: f64, z: f64) -> toml::Value {
        toml::Value::Array(vec![x.into(), y.into(), z.into()])
    }

    /// The world as the viewer holds it: the file's entities, and one a
    /// prefab instance made
    fn world() -> (FlintWorld, EntityId, EntityId) {
        let mut world = FlintWorld::new();
        let crate_id = world.spawn("crate").unwrap();
        world.get_components_mut(crate_id).unwrap().set_field(
            "transform",
            "position",
            vec3(0.0, 0.0, 0.0),
        );
        let barrel = world.spawn("props_barrel").unwrap();
        world.get_components_mut(barrel).unwrap().set_field(
            "transform",
            "position",
            vec3(4.0, 0.0, 0.0),
        );
        (world, crate_id, barrel)
    }

    fn save(edits: &mut FieldEdits, world: &FlintWorld, doc: &mut SceneDocument) -> SavePlan {
        let plan = edits.plan(world, doc);
        edits.saved(&plan);
        plan
    }

    fn position(doc: &SceneDocument) -> toml::Value {
        let file: toml::Table = toml::from_str(&doc.to_string()).unwrap();
        file["entities"]["crate"]["transform"]["position"].clone()
    }

    #[test]
    fn undo_after_a_save_is_dirty_and_saving_writes_the_reverted_value() {
        let (mut world, id, _) = world();
        let mut doc = SceneDocument::from_str(SCENE).unwrap();
        let mut edits = FieldEdits::default();
        assert!(!edits.is_dirty(&world));

        // Edit, save: clean, and the file has the edit
        edits.set(&mut world, id, "transform", "position", vec3(1.0, 2.0, 3.0));
        assert!(edits.is_dirty(&world));
        let plan = save(&mut edits, &world, &mut doc);
        assert_eq!(plan.written.len(), 1);
        assert!(!edits.is_dirty(&world));
        assert_eq!(position(&doc), vec3(1.0, 2.0, 3.0));

        // Undo goes through the same path: dirty, and the save reverts it
        edits.set(&mut world, id, "transform", "position", vec3(0.0, 0.0, 0.0));
        assert!(edits.is_dirty(&world), "undo after a save left it clean");
        let plan = save(&mut edits, &world, &mut doc);
        assert_eq!(plan.written[0].0.old, Some(vec3(1.0, 2.0, 3.0)));
        assert_eq!(position(&doc), vec3(0.0, 0.0, 0.0));
        assert!(doc.to_string().contains("# by the door"));

        // Redo: dirty again, and written
        edits.set(&mut world, id, "transform", "position", vec3(1.0, 2.0, 3.0));
        assert!(edits.is_dirty(&world), "redo left it clean");
        save(&mut edits, &world, &mut doc);
        assert_eq!(position(&doc), vec3(1.0, 2.0, 3.0));
    }

    #[test]
    fn undoing_back_to_the_file_is_clean_and_saves_nothing() {
        let (mut world, id, _) = world();
        let mut doc = SceneDocument::from_str(SCENE).unwrap();
        let mut edits = FieldEdits::default();
        edits.set(&mut world, id, "transform", "position", vec3(5.0, 0.0, 0.0));
        edits.set(&mut world, id, "transform", "position", vec3(0.0, 0.0, 0.0));
        assert!(!edits.is_dirty(&world));
        let plan = save(&mut edits, &world, &mut doc);
        assert!(plan.written.is_empty() && plan.failed.is_empty());
        assert_eq!(doc.to_string(), SCENE);
    }

    #[test]
    fn a_prefab_entity_saves_into_its_instance_overrides() {
        let (mut world, _, barrel) = world();
        let mut doc = SceneDocument::from_str(SCENE).unwrap();
        let mut edits = FieldEdits::default();
        edits.set(
            &mut world,
            barrel,
            "transform",
            "position",
            vec3(4.0, 1.0, 0.0),
        );
        let plan = save(&mut edits, &world, &mut doc);
        assert!(plan.failed.is_empty(), "{:?}", plan.failed);
        assert_eq!(
            plan.written[0].1.table("props_barrel", "transform"),
            "prefabs.props.overrides.barrel.transform"
        );
        let file: flint_scene::SceneFile = toml::from_str(&doc.to_string()).unwrap();
        assert_eq!(
            file.prefabs["props"].overrides["barrel"]["transform"]["position"],
            vec3(4.0, 1.0, 0.0)
        );
        assert!(!edits.is_dirty(&world));
    }

    #[test]
    fn a_field_that_cannot_be_written_stays_dirty_and_says_why() {
        let (mut world, _, _) = world();
        let lamp = world.spawn("lamp").unwrap();
        let mut doc = SceneDocument::from_str(SCENE).unwrap();
        let mut edits = FieldEdits::default();
        edits.set(&mut world, lamp, "light", "intensity", 3.0.into());
        let plan = save(&mut edits, &world, &mut doc);
        assert!(plan.written.is_empty());
        assert_eq!(plan.failed.len(), 1);
        assert!(edits.is_dirty(&world));
        let problems = edits.problems(&world);
        assert_eq!(problems.len(), 1);
        assert!(problems[0].contains("'lamp'"), "{problems:?}");
        assert_eq!(doc.to_string(), SCENE);

        // Taken away again (an undo): no longer a change, so no longer a problem
        world.remove_component(lamp, "light").unwrap();
        assert!(!edits.is_dirty(&world));
        assert!(edits.problems(&world).is_empty());
    }

    /// The whole path on a real scene: the loader expands the prefab, the
    /// barrel it makes is moved, the save goes into the instance's
    /// overrides, and the scene read back from the saved text has the barrel
    /// where it was put.
    #[test]
    fn a_prefab_entity_moved_in_a_real_scene_is_there_when_it_is_read_back() {
        let path = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../tests/fixtures/scenes/stylize_props.scene.toml");
        let scene_path = path.to_string_lossy().into_owned();
        let (registry, _, _) = crate::app::load_schemas(&scene_path, &[]).unwrap();
        let (mut world, _) = flint_scene::load_scene(&path, &registry).unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        let mut doc = SceneDocument::from_str(&text).unwrap();
        assert!(!doc.has_entity("props_barrel"), "the fixture changed");

        // Every entity in this scene is the prefab's
        let barrel = world.get_id("props_barrel").expect("the prefab's barrel");
        let mut edits = FieldEdits::default();
        let moved = vec3(-2.0, 0.48, 3.0);
        edits.set(&mut world, barrel, "transform", "position", moved.clone());
        let plan = save(&mut edits, &world, &mut doc);
        assert!(plan.failed.is_empty(), "{:?}", plan.failed);
        assert_eq!(plan.written.len(), 1);
        assert!(!edits.is_dirty(&world));

        let saved = doc.to_string();
        let (back, _) = flint_scene::load_scene_text(&saved, &path, &registry).unwrap();
        let field = |w: &FlintWorld, name: &str, c: &str, f: &str| {
            w.get_components(w.get_id(name).unwrap())
                .unwrap()
                .get_field(c, f)
                .cloned()
        };
        assert_eq!(
            field(&back, "props_barrel", "transform", "position"),
            Some(moved)
        );
        // The barrel's other fields are still the template's, and its style
        // the instance's override
        assert_eq!(
            field(&back, "props_barrel", "transform", "scale"),
            field(&world, "props_barrel", "transform", "scale")
        );
        assert_eq!(
            field(&back, "props_barrel", "style_mask", "style"),
            Some("ascii".into())
        );
        // Every line the file had is still there, and the new ones are the
        // override's header and its field
        let old: Vec<&str> = text.lines().collect();
        let new: Vec<&str> = saved.lines().collect();
        assert!(old.iter().all(|l| new.contains(l)));
        let added: Vec<&&str> = new.iter().filter(|l| !old.contains(l)).collect();
        assert_eq!(
            added,
            [
                &"[prefabs.props.overrides.barrel.transform]",
                &"position = [-2.0, 0.48, 3.0]"
            ],
            "{saved}"
        );
    }

    /// #1046: a commit's keys, each in its own table: a value changed, a
    /// key put back at its default (gone), a folded old key removed, and a
    /// style turned off whose whole table went
    #[test]
    fn a_commits_post_process_keys_are_named_in_their_tables() {
        let before = "[post_process]\nbloom_intensity = 0.04\nkuwahara_radius = 3\n\n\
                      [post_process.styles.ascii]\ncell = 8\nlevels = 6\n\n\
                      [post_process.styles.pencil]\nenabled = true\n";
        let after = "[post_process]\nbloom_intensity = 0.04\n\n\
                     [post_process.styles.ascii]\ncell = 12\n";
        let changes = post_process_changes(before, after);
        let named: Vec<(&str, &str)> = changes
            .iter()
            .map(|c| (c.table.as_str(), c.key.as_str()))
            .collect();
        assert_eq!(
            named,
            [
                ("post_process", "kuwahara_radius"),
                ("post_process.styles.ascii", "cell"),
                ("post_process.styles.ascii", "levels"),
                ("post_process.styles.pencil", "enabled"),
            ]
        );
        assert_eq!(changes[1].old, Some(8.into()));
        assert_eq!(changes[1].new, Some(12.into()));
        assert_eq!(changes[3].new, None);

        let lines: Vec<serde_json::Value> = commit_log_lines(&changes, "a.scene.toml", 7)
            .lines()
            .map(|l| serde_json::from_str(l).unwrap())
            .collect();
        assert_eq!(lines.len(), 4);
        assert_eq!(lines[1]["entity"], serde_json::Value::Null);
        assert_eq!(lines[1]["component"], "post_process");
        assert_eq!(lines[1]["field"], "cell");
        assert_eq!(lines[1]["table"], "post_process.styles.ascii");
        assert_eq!(lines[3]["new"], serde_json::Value::Null);
        // Nothing changed is no line
        assert!(post_process_changes(before, before).is_empty());
    }

    #[test]
    fn each_save_appends_its_fields_as_json_lines() {
        let (mut world, id, barrel) = world();
        let mut doc = SceneDocument::from_str(SCENE).unwrap();
        let mut edits = FieldEdits::default();
        edits.set(&mut world, id, "transform", "position", vec3(1.0, 2.0, 3.0));
        edits.set(
            &mut world,
            barrel,
            "transform",
            "scale",
            vec3(2.0, 2.0, 2.0),
        );
        let first = save(&mut edits, &world, &mut doc);
        edits.set(&mut world, id, "transform", "position", vec3(0.0, 0.0, 0.0));
        let second = save(&mut edits, &world, &mut doc);

        let dir = std::env::temp_dir().join(format!("flint-679-{}", std::process::id()));
        let log = dir.join(EDIT_LOG);
        let _ = std::fs::remove_dir_all(&dir);
        append_log(&log, &log_lines(&first, "scenes/a.scene.toml", 100)).unwrap();
        append_log(&log, &log_lines(&second, "scenes/a.scene.toml", 200)).unwrap();
        let text = std::fs::read_to_string(&log).unwrap();
        let _ = std::fs::remove_dir_all(&dir);

        let lines: Vec<serde_json::Value> = text
            .lines()
            .map(|l| serde_json::from_str(l).unwrap())
            .collect();
        assert_eq!(lines.len(), 3, "{text}");
        assert_eq!(lines[0]["entity"], "crate");
        assert_eq!(lines[0]["field"], "position");
        assert_eq!(lines[0]["old"], serde_json::json!([0.0, 0.0, 0.0]));
        assert_eq!(lines[0]["new"], serde_json::json!([1.0, 2.0, 3.0]));
        assert_eq!(lines[0]["table"], "entities.crate.transform");
        assert_eq!(lines[0]["saved_at"], 100);
        // A field the entity didn't have was null
        assert_eq!(lines[1]["entity"], "props_barrel");
        assert_eq!(lines[1]["old"], serde_json::Value::Null);
        assert_eq!(
            lines[1]["table"],
            "prefabs.props.overrides.barrel.transform"
        );
        // The second save: the undo, as its own line
        assert_eq!(lines[2]["saved_at"], 200);
        assert_eq!(lines[2]["old"], serde_json::json!([1.0, 2.0, 3.0]));
        assert_eq!(lines[2]["new"], serde_json::json!([0.0, 0.0, 0.0]));
    }
}
