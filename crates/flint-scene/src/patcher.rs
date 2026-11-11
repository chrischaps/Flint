//! Structure-preserving TOML scene patcher
//!
//! Uses `toml_edit` to modify only the changed fields in a scene file,
//! preserving comments, formatting, and ordering of unchanged content.
//! This produces minimal diffs when saving from the viewer.

use flint_ecs::FlintWorld;
use std::collections::HashSet;
use std::fs;
use std::path::Path;

/// A parsed scene document that can be patched field-by-field
pub struct SceneDocument {
    doc: toml_edit::DocumentMut,
    /// The file's lines end in `\r\n` (a Windows checkout). `toml_edit` writes
    /// `\n`, which would change every line of the file on save (#677).
    crlf: bool,
}

/// A dirty field identifier: (entity_name, component_name, field_name)
pub type DirtyField = (String, String, String);

impl SceneDocument {
    /// Parse a scene file into an editable document
    pub fn from_file<P: AsRef<Path>>(path: P) -> Result<Self, String> {
        let content = flint_core::vfs::read_to_string(path.as_ref())
            .map_err(|e| format!("Failed to read scene file: {}", e))?;
        Self::from_str(&content)
    }

    /// Parse a TOML string into an editable document
    pub fn from_str(content: &str) -> Result<Self, String> {
        let doc: toml_edit::DocumentMut = content
            .parse()
            .map_err(|e| format!("Failed to parse TOML: {}", e))?;
        Ok(Self {
            doc,
            crlf: content.contains("\r\n"),
        })
    }

    /// Patch a single field in the document.
    /// Navigates to `entities.<entity_name>.<component>.<field>` and sets the value.
    pub fn patch_field(
        &mut self,
        entity_name: &str,
        component: &str,
        field: &str,
        value: &toml::Value,
    ) -> Result<(), String> {
        // Ensure entities table exists
        let entities = self
            .doc
            .get_mut("entities")
            .and_then(|v| v.as_table_like_mut())
            .ok_or_else(|| "No [entities] table in scene file".to_string())?;

        // Ensure entity table exists
        let entity = entities
            .get_mut(entity_name)
            .and_then(|v| v.as_table_like_mut())
            .ok_or_else(|| format!("Entity '{}' not found in scene file", entity_name))?;

        // Ensure component table exists (create if needed)
        if entity.get(component).is_none() {
            entity.insert(component, toml_edit::Item::Table(toml_edit::Table::new()));
        }

        let comp = entity
            .get_mut(component)
            .and_then(|v| v.as_table_like_mut())
            .ok_or_else(|| format!("Component '{}' is not a table", component))?;

        // A value that already says the same is left as written (#677), and
        // one that's replaced keeps its comment.
        match comp.get_mut(field).and_then(|item| item.as_value_mut()) {
            Some(there) if says_same(there, value) => {}
            Some(there) => {
                let decor = there.decor().clone();
                *there = toml_to_edit_value(value);
                *there.decor_mut() = decor;
            }
            None => {
                comp.insert(field, toml_edit::Item::Value(toml_to_edit_value(value)));
            }
        }

        Ok(())
    }

    /// Take `field` out of an entity's component table. Nothing to do if the
    /// file doesn't write it (an archetype default the fix took away).
    pub fn remove_field(&mut self, entity_name: &str, component: &str, field: &str) {
        if let Some(comp) = self
            .entity_mut(entity_name)
            .and_then(|e| e.get_mut(component))
            .and_then(|c| c.as_table_like_mut())
        {
            comp.remove(field);
        }
    }

    /// Whether `[entities.<name>]` is written in the file. An entity a prefab
    /// instance expands into is in the world but not here.
    pub fn has_entity(&self, name: &str) -> bool {
        self.has_table(&["entities", name])
    }

    fn entity_mut(&mut self, name: &str) -> Option<&mut dyn toml_edit::TableLike> {
        self.doc
            .get_mut("entities")
            .and_then(|v| v.as_table_like_mut())
            .and_then(|e| e.get_mut(name))
            .and_then(|v| v.as_table_like_mut())
    }

    /// Write a new entity at the end of the file: `[entities.<name>]` with its
    /// archetype and parent, then a table per component, in name order.
    pub fn add_entity(
        &mut self,
        name: &str,
        archetype: Option<&str>,
        parent: Option<&str>,
        components: &std::collections::HashMap<String, toml::Value>,
    ) -> Result<(), String> {
        let root = self.doc.as_table_mut();
        if root.get("entities").is_none() {
            let mut table = toml_edit::Table::new();
            table.set_implicit(true);
            root.insert("entities", toml_edit::Item::Table(table));
        }
        let entities = root
            .get_mut("entities")
            .and_then(|v| v.as_table_like_mut())
            .ok_or_else(|| "entities in the scene is not a table".to_string())?;
        if entities.get(name).is_some() {
            return Err(format!("Entity '{name}' is already in the scene file"));
        }
        let mut entity = toml_edit::Table::new();
        entity.decor_mut().set_prefix("\n");
        if let Some(archetype) = archetype {
            entity.insert("archetype", toml_edit::value(archetype));
        }
        if let Some(parent) = parent {
            entity.insert("parent", toml_edit::value(parent));
        }
        let mut names: Vec<&String> = components.keys().collect();
        names.sort();
        for component in names {
            let item = match &components[component] {
                toml::Value::Table(fields) => {
                    let mut table = toml_edit::Table::new();
                    for (key, value) in fields {
                        table.insert(key, toml_edit::Item::Value(toml_to_edit_value(value)));
                    }
                    toml_edit::Item::Table(table)
                }
                other => toml_edit::Item::Value(toml_to_edit_value(other)),
            };
            entity.insert(component, item);
        }
        entities.insert(name, toml_edit::Item::Table(entity));
        Ok(())
    }

    /// Write into the file what changed between two worlds loaded from it
    /// (#677): the fields `after` sets differently, the fields and components
    /// it took away, the entities it added and the ones it removed. Nothing
    /// else in the file changes, so its `[scene]`, `[camera]`, `[environment]`,
    /// `[post_process]`, `[living]`, `[wooze]`, prefabs, comments and key
    /// order stay as they were. Entities are matched by name, and so are their
    /// parents.
    ///
    /// Returns what couldn't be written: a change to an entity the file doesn't
    /// hold (one a prefab instance expands into), a renamed parent.
    pub fn apply_world_diff(&mut self, before: &FlintWorld, after: &FlintWorld) -> Vec<String> {
        let mut problems = Vec::new();
        for info in before.all_entities() {
            if !after.contains_name(&info.name) {
                if let Some(entities) = self
                    .doc
                    .get_mut("entities")
                    .and_then(|v| v.as_table_like_mut())
                {
                    if entities.remove(&info.name).is_none() {
                        problems.push(format!(
                            "'{}' was removed, but the file doesn't write it (a prefab's?)",
                            info.name
                        ));
                    }
                }
            }
        }
        let empty = std::collections::HashMap::new();
        for info in after.all_entities() {
            let now = after.get_components(info.id);
            let now_data = now.map(|c| &c.data).unwrap_or(&empty);
            let Some(old_id) = before.get_id(&info.name) else {
                let archetype = now.and_then(|c| c.archetype.as_deref());
                if let Err(e) =
                    self.add_entity(&info.name, archetype, info.parent.as_deref(), now_data)
                {
                    problems.push(e);
                }
                continue;
            };
            let then = before.get_components(old_id);
            let then_data = then.map(|c| &c.data).unwrap_or(&empty);
            let mut changes: Vec<FieldChange> = Vec::new();
            if then.and_then(|c| c.archetype.as_ref()) != now.and_then(|c| c.archetype.as_ref()) {
                changes.push(FieldChange::Entity("archetype"));
            }
            if before.get_parent(old_id).and_then(|p| before.get_name(p))
                != after.get_parent(info.id).and_then(|p| after.get_name(p))
            {
                changes.push(FieldChange::Entity("parent"));
            }
            diff_components(then_data, now_data, &mut changes);
            if changes.is_empty() {
                continue;
            }
            if !self.has_entity(&info.name) {
                problems.push(format!(
                    "'{}' changed, but the file doesn't write it (a prefab's?): not saved",
                    info.name
                ));
                continue;
            }
            for change in changes {
                let result = match change {
                    FieldChange::Entity(key) => {
                        let value = match key {
                            "archetype" => now.and_then(|c| c.archetype.clone()),
                            _ => info.parent.clone(),
                        };
                        let entity = self.entity_mut(&info.name).expect("checked above");
                        match value {
                            Some(v) => {
                                entity.insert(key, toml_edit::value(v));
                            }
                            None => {
                                entity.remove(key);
                            }
                        }
                        Ok(())
                    }
                    FieldChange::Set(component, field, value) => {
                        self.patch_field(&info.name, component, field, value)
                    }
                    FieldChange::Remove(component, field) => {
                        self.remove_field(&info.name, component, field);
                        Ok(())
                    }
                    FieldChange::Component(component, value) => {
                        let entity = self.entity_mut(&info.name).expect("checked above");
                        match value {
                            Some(v) => {
                                entity.insert(
                                    component,
                                    toml_edit::Item::Value(toml_to_edit_value(v)),
                                );
                            }
                            None => {
                                entity.remove(component);
                            }
                        }
                        Ok(())
                    }
                };
                if let Err(e) = result {
                    problems.push(e);
                }
            }
        }
        problems
    }

    /// Patch multiple dirty fields at once
    pub fn patch_fields(
        &mut self,
        dirty: &HashSet<DirtyField>,
        get_value: impl Fn(&str, &str, &str) -> Option<toml::Value>,
    ) -> Vec<String> {
        let mut errors = Vec::new();

        for (entity_name, component, field) in dirty {
            if let Some(value) = get_value(entity_name, component, field) {
                if let Err(e) = self.patch_field(entity_name, component, field, &value) {
                    errors.push(e);
                }
            }
        }

        errors
    }

    /// Rewrite style `name`'s `[post_process.styles.<name>]` table to say
    /// what `block` says (#364). `block` is a style's TOML as
    /// `flint_render::stylize::style_toml` writes it: the table header and
    /// only the values that differ from the defaults. `owned` names every
    /// key the style can take (its options, params, colour keys, `enabled`,
    /// `order`, `feeds_bloom`).
    ///
    /// Key by key, so the table's comments and the order of the keys that
    /// stay survive: a key in `block` replaces the table's value in place
    /// (its comments kept), an owned key `block` leaves out is back at its
    /// default and is removed, and a key the style doesn't own (a typo the
    /// loader warns about) is left alone. A style that is off with nothing
    /// else set needs no table, so its table goes. Old `[post_process]`
    /// keys that fold into this style (`kuwahara_radius`) are removed, or
    /// they would fill back in a value the table now leaves at its default.
    pub fn set_style_table(
        &mut self,
        name: &str,
        block: &str,
        owned: &[&str],
    ) -> Result<(), String> {
        let parsed: toml_edit::DocumentMut = block
            .parse()
            .map_err(|e| format!("Style block doesn't parse: {e}"))?;
        let fresh = parsed
            .get("post_process")
            .and_then(|p| p.get("styles"))
            .and_then(|s| s.get(name))
            .and_then(|t| t.as_table_like())
            .ok_or_else(|| format!("The block has no [post_process.styles.{name}] table"))?;
        let entries: Vec<(String, toml_edit::Value)> = fresh
            .iter()
            .filter_map(|(k, v)| Some((k.to_string(), v.as_value()?.clone())))
            .collect();
        // Off and nothing else: the same as no table at all
        let needs_table = !(entries.len() == 1
            && entries[0].0 == "enabled"
            && entries[0].1.as_bool() == Some(false));

        let root = self.doc.as_table_mut();
        if root.get("post_process").is_none() {
            if !needs_table {
                return Ok(());
            }
            let mut table = toml_edit::Table::new();
            table.set_implicit(true);
            root.insert("post_process", toml_edit::Item::Table(table));
        }
        let post = root
            .get_mut("post_process")
            .and_then(|p| p.as_table_like_mut())
            .ok_or_else(|| "post_process in the scene is not a table".to_string())?;
        for &(old, style, _) in crate::format::LEGACY_STYLE_KEYS {
            if style == name {
                post.remove(old);
            }
        }
        if post.get("styles").is_none() {
            if !needs_table {
                return Ok(());
            }
            let mut table = toml_edit::Table::new();
            table.set_implicit(true);
            post.insert("styles", toml_edit::Item::Table(table));
        }
        let styles = post
            .get_mut("styles")
            .and_then(|s| s.as_table_like_mut())
            .ok_or_else(|| "post_process.styles in the scene is not a table".to_string())?;
        if !needs_table {
            styles.remove(name);
            return Ok(());
        }
        if styles.get(name).is_none() {
            styles.insert(name, toml_edit::Item::Table(toml_edit::Table::new()));
        }
        let table = styles
            .get_mut(name)
            .and_then(|t| t.as_table_like_mut())
            .ok_or_else(|| format!("post_process.styles.{name} in the scene is not a table"))?;

        let stale: Vec<String> = table
            .iter()
            .map(|(k, _)| k.to_string())
            .filter(|k| owned.contains(&k.as_str()) && !entries.iter().any(|(e, _)| e == k))
            .collect();
        for key in stale {
            table.remove(&key);
        }
        for (key, mut value) in entries {
            match table.get_mut(&key).and_then(|item| item.as_value_mut()) {
                Some(existing) => {
                    // The new value in the old one's place, with its comments
                    *value.decor_mut() = existing.decor().clone();
                    *existing = value;
                }
                None => {
                    table.insert(&key, toml_edit::Item::Value(value));
                }
            }
        }
        Ok(())
    }

    /// Whether the file has this table, as a header or inline: `["post_process"]`.
    pub fn has_table(&self, path: &[&str]) -> bool {
        let mut at: &dyn toml_edit::TableLike = self.doc.as_table();
        for segment in path {
            match at.get(segment).and_then(|item| item.as_table_like()) {
                Some(next) => at = next,
                None => return false,
            }
        }
        true
    }

    /// Set one scene-level value, or take it out (#249): `edit.table` is a header
    /// such as `living.species.grove_thorn`, made if it isn't there, and
    /// `edit.key` the path inside it, through inline tables, as
    /// `echoes.count`. Only that value's text changes: a value that already says
    /// the same is left as written, and one that's replaced keeps its comment.
    /// A species table left with nothing in it goes.
    pub fn apply_edit(&mut self, edit: &crate::commit::SceneEdit) -> Result<(), String> {
        let (leaf, inside) = edit
            .key
            .split_last()
            .ok_or_else(|| format!("{}: no key", edit.label()))?;
        if edit.value.is_none()
            && !self.has_table(&edit.table.iter().map(String::as_str).collect::<Vec<_>>())
        {
            return Ok(());
        }
        let anchor = self.new_table_position(&edit.table[0]);
        let table = header_table(self.doc.as_table_mut(), &edit.table, anchor)?;

        // The inline tables on the way, made only to set something.
        let mut at: &mut dyn toml_edit::TableLike = table;
        for segment in inside {
            if at.get(segment).is_none() {
                if edit.value.is_none() {
                    return Ok(());
                }
                at.insert(
                    segment,
                    toml_edit::Item::Value(toml_edit::Value::InlineTable(Default::default())),
                );
            }
            at = at
                .get_mut(segment)
                .and_then(|item| item.as_table_like_mut())
                .ok_or_else(|| format!("{}: '{segment}' isn't a table", edit.label()))?;
        }

        match &edit.value {
            Some(value) => match at.get_mut(leaf).and_then(|item| item.as_value_mut()) {
                Some(there) if value.is_in(there) => {}
                Some(there) => {
                    let decor = there.decor().clone();
                    *there = value.to_edit();
                    *there.decor_mut() = decor;
                }
                None => {
                    at.insert(leaf, toml_edit::Item::Value(value.to_edit()));
                }
            },
            None => {
                at.remove(leaf);
            }
        }
        if edit.value.is_none() {
            self.drop_empty(&edit.table, inside);
        }
        Ok(())
    }

    /// After taking a value out: an inline table it leaves empty goes, and so does
    /// a species table.
    fn drop_empty(&mut self, table: &[String], inside: &[String]) {
        let Ok(header) = header_table(self.doc.as_table_mut(), table, None) else {
            return;
        };
        if let Some(container) = inside.first() {
            let empty = header
                .get(container)
                .and_then(|item| item.as_table_like())
                .is_some_and(|t| t.is_empty());
            if empty {
                header.remove(container);
            }
        }
        let is_species = table.len() == 3 && table[0] == "living" && table[1] == "species";
        if is_species && header.is_empty() {
            let species = (self.doc.get_mut("living"))
                .and_then(|item| item.as_table_like_mut())
                .and_then(|living| living.get_mut("species"))
                .and_then(|item| item.as_table_like_mut());
            if let Some(species) = species {
                species.remove(&table[2]);
            }
        }
    }

    /// Where a new top-level table goes: after the tables of `[wooze]`, its zones
    /// included, or after `[scene]`'s, so that `[living]` sits with the scene's
    /// other settings rather than after its last entity. `None` if `name` is
    /// already there.
    fn new_table_position(&self, name: &str) -> Option<usize> {
        if self.doc.get(name).is_some() {
            return None;
        }
        ["wooze", "scene"].iter().find_map(|anchor| {
            let item = self.doc.get(anchor)?;
            let mut last = None;
            last_position(item, &mut last);
            last
        })
    }

    /// Serialize the document back to a string (preserving formatting, line
    /// endings included)
    pub fn to_string(&self) -> String {
        let text = self.doc.to_string();
        if self.crlf {
            text.replace("\r\n", "\n").replace('\n', "\r\n")
        } else {
            text
        }
    }

    /// Write the document to a file
    pub fn save<P: AsRef<Path>>(&self, path: P) -> Result<(), String> {
        fs::write(path, self.to_string()).map_err(|e| format!("Failed to write scene file: {}", e))
    }
}

/// The header table at `path` from the root, made where it's missing: the last
/// one as a `[header]` with a blank line before it, the ones on the way implicit,
/// so that a new `[living.species.x]` doesn't bring an empty `[living]` with it.
/// A new top-level table takes `anchor` as its place in the file.
fn header_table<'a>(
    root: &'a mut toml_edit::Table,
    path: &[String],
    anchor: Option<usize>,
) -> Result<&'a mut dyn toml_edit::TableLike, String> {
    let mut at: &mut dyn toml_edit::TableLike = root;
    for (depth, segment) in path.iter().enumerate() {
        if at.get(segment).is_none() {
            let mut table = toml_edit::Table::new();
            if depth + 1 < path.len() {
                table.set_implicit(true);
            } else {
                table.decor_mut().set_prefix("\n");
            }
            if let (0, Some(position)) = (depth, anchor) {
                table.set_position(position);
            }
            at.insert(segment, toml_edit::Item::Table(table));
        }
        at = at
            .get_mut(segment)
            .and_then(|item| item.as_table_like_mut())
            .ok_or_else(|| format!("[{}] isn't a table", path[..=depth].join(".")))?;
    }
    Ok(at)
}

/// The last place any table under `item` has in the file.
fn last_position(item: &toml_edit::Item, last: &mut Option<usize>) {
    match item {
        toml_edit::Item::Table(table) => {
            if let Some(position) = table.position() {
                *last = Some(last.map_or(position, |l| l.max(position)));
            }
            for (_, child) in table.iter() {
                last_position(child, last);
            }
        }
        toml_edit::Item::ArrayOfTables(array) => {
            for table in array.iter() {
                if let Some(position) = table.position() {
                    *last = Some(last.map_or(position, |l| l.max(position)));
                }
            }
        }
        _ => {}
    }
}

/// One difference between an entity in two worlds.
enum FieldChange<'a> {
    /// Its `archetype` or `parent` key
    Entity(&'static str),
    /// A field of a component table, set
    Set(&'a str, &'a str, &'a toml::Value),
    /// A field of a component table, taken away
    Remove(&'a str, &'a str),
    /// A component that isn't a table of fields, or a whole component taken
    /// away (`None`)
    Component(&'a str, Option<&'a toml::Value>),
}

fn diff_components<'a>(
    then: &'a std::collections::HashMap<String, toml::Value>,
    now: &'a std::collections::HashMap<String, toml::Value>,
    out: &mut Vec<FieldChange<'a>>,
) {
    let mut names: Vec<&String> = then.keys().chain(now.keys()).collect();
    names.sort();
    names.dedup();
    for name in names {
        match (then.get(name), now.get(name)) {
            (Some(_), None) => out.push(FieldChange::Component(name, None)),
            (old, Some(toml::Value::Table(fields))) => {
                let old_fields = old.and_then(|v| v.as_table());
                let mut keys: Vec<&String> = fields.keys().collect();
                keys.sort();
                for key in keys {
                    let value = &fields[key];
                    if old_fields.and_then(|t| t.get(key)) != Some(value) {
                        out.push(FieldChange::Set(name, key, value));
                    }
                }
                if let Some(old_fields) = old_fields {
                    let mut gone: Vec<&String> = old_fields
                        .keys()
                        .filter(|k| !fields.contains_key(*k))
                        .collect();
                    gone.sort();
                    for key in gone {
                        out.push(FieldChange::Remove(name, key));
                    }
                }
            }
            (old, Some(value)) => {
                if old != Some(value) {
                    out.push(FieldChange::Component(name, Some(value)));
                }
            }
            (None, None) => {}
        }
    }
}

/// Whether the file's value already says `value`. A number written `1` is the
/// same as `1.0`: the loader reads either.
fn says_same(there: &toml_edit::Value, value: &toml::Value) -> bool {
    let mut bare = there.clone();
    bare.decor_mut().clear();
    let Ok(table) = toml::from_str::<toml::Table>(&format!("x = {bare}")) else {
        return false;
    };
    table
        .get("x")
        .is_some_and(|written| same_value(written, value))
}

fn same_value(a: &toml::Value, b: &toml::Value) -> bool {
    use toml::Value::{Array, Float, Integer, Table};
    match (a, b) {
        (Integer(i), Float(f)) | (Float(f), Integer(i)) => *i as f64 == *f,
        (Array(x), Array(y)) => {
            x.len() == y.len() && x.iter().zip(y).all(|(p, q)| same_value(p, q))
        }
        (Table(x), Table(y)) => {
            x.len() == y.len()
                && x.iter()
                    .all(|(k, v)| y.get(k).is_some_and(|w| same_value(v, w)))
        }
        _ => a == b,
    }
}

/// Convert a `toml::Value` to a `toml_edit::Value`, using inline formatting
/// for arrays (Vec3, Color) to keep them compact on a single line.
fn toml_to_edit_value(value: &toml::Value) -> toml_edit::Value {
    match value {
        toml::Value::String(s) => toml_edit::Value::from(s.as_str()),
        toml::Value::Integer(i) => toml_edit::Value::from(*i),
        toml::Value::Float(f) => {
            // Ensure float renders with decimal point
            let formatted = format_float(*f);
            // Parse as a toml_edit value to get correct repr
            let s = format!("x = {}", formatted);
            if let Ok(doc) = s.parse::<toml_edit::DocumentMut>() {
                if let Some(v) = doc.get("x") {
                    return v
                        .as_value()
                        .cloned()
                        .unwrap_or_else(|| toml_edit::Value::from(*f));
                }
            }
            toml_edit::Value::from(*f)
        }
        toml::Value::Boolean(b) => toml_edit::Value::from(*b),
        toml::Value::Array(arr) => {
            let mut edit_arr = toml_edit::Array::new();
            for item in arr {
                edit_arr.push(toml_to_edit_value(item));
            }
            // Force inline formatting for compact Vec3/Color arrays
            edit_arr.set_trailing("");
            edit_arr.set_trailing_comma(false);
            toml_edit::Value::Array(edit_arr)
        }
        toml::Value::Table(table) => {
            let mut edit_table = toml_edit::InlineTable::new();
            for (key, val) in table {
                edit_table.insert(key, toml_to_edit_value(val));
            }
            toml_edit::Value::InlineTable(edit_table)
        }
        toml::Value::Datetime(dt) => {
            // Convert datetime to string representation
            toml_edit::Value::from(dt.to_string())
        }
    }
}

/// Format a float value to always include a decimal point
fn format_float(f: f64) -> String {
    if f.fract() == 0.0 {
        format!("{:.1}", f)
    } else {
        // Trim trailing zeros but keep at least one decimal digit
        let s = format!("{}", f);
        if s.contains('.') {
            s
        } else {
            format!("{:.1}", f)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_patch_preserves_comments() {
        let original = r#"# Scene header comment
[scene]
name = "Test Scene"

# Player entity
[entities.player]
archetype = "player"

[entities.player.transform]
position = [0, 1, 0]
rotation = [0, 0, 0]
scale = [1, 1, 1]

# Room entity
[entities.room]
archetype = "room"

[entities.room.bounds]
min = [0, 0, 0]
max = [10, 4, 8]
"#;

        let mut doc = SceneDocument::from_str(original).unwrap();

        // Patch the player position
        let new_pos = toml::Value::Array(vec![
            toml::Value::Float(5.0),
            toml::Value::Float(2.0),
            toml::Value::Float(3.0),
        ]);
        doc.patch_field("player", "transform", "position", &new_pos)
            .unwrap();

        let result = doc.to_string();

        // Comments should be preserved
        assert!(result.contains("# Scene header comment"));
        assert!(result.contains("# Player entity"));
        assert!(result.contains("# Room entity"));

        // The position should be updated
        assert!(result.contains("5.0"));
        assert!(result.contains("2.0"));
        assert!(result.contains("3.0"));

        // Room should be unchanged
        assert!(result.contains("[entities.room.bounds]"));
    }

    #[test]
    fn test_patch_single_field() {
        let original = r#"[scene]
name = "Test"

[entities.box]
archetype = "furniture"

[entities.box.transform]
position = [0, 0, 0]
rotation = [0, 0, 0]
scale = [1, 1, 1]

[entities.box.material]
color = [0.5, 0.5, 0.5, 1.0]
roughness = 0.8
"#;

        let mut doc = SceneDocument::from_str(original).unwrap();
        doc.patch_field("box", "material", "roughness", &toml::Value::Float(0.3))
            .unwrap();

        let result = doc.to_string();
        assert!(result.contains("0.3"));
        // Other fields should still be present
        assert!(result.contains("color = [0.5, 0.5, 0.5, 1.0]"));
    }

    const ASCII_KEYS: &[&str] = &[
        "enabled",
        "order",
        "feeds_bloom",
        "ramp",
        "palette",
        "cell",
        "glow",
        "tint",
        "tint_r",
        "tint_g",
        "tint_b",
    ];

    const COMMENTED: &str = r#"# The lantern room
[scene]
name = "Lantern"

# How it looks
[post_process]
bloom_intensity = 0.1 # soft
kuwahara_radius = 4

# The glyph look
[post_process.styles.ascii]
# big cells read from across the room
cell = 12 # tuned by eye
ramp = "blocks"
glow = 0.6
mystery = 3

[entities.lamp]
archetype = "light"
"#;

    #[test]
    fn a_style_table_is_rewritten_key_by_key_keeping_comments() {
        let mut doc = SceneDocument::from_str(COMMENTED).unwrap();
        // cell changed, ramp unchanged, glow back at its default, tint new
        let block =
            "[post_process.styles.ascii]\ncell = 16\nramp = \"blocks\"\ntint = [0.35, 1.0, 0.45]\n";
        doc.set_style_table("ascii", block, ASCII_KEYS).unwrap();
        let out = doc.to_string();
        assert!(out.contains("# The lantern room"), "{out}");
        assert!(out.contains("# How it looks"), "{out}");
        assert!(out.contains("bloom_intensity = 0.1 # soft"), "{out}");
        assert!(out.contains("# The glyph look"), "{out}");
        assert!(
            out.contains("# big cells read from across the room\ncell = 16 # tuned by eye"),
            "{out}"
        );
        assert!(out.contains("ramp = \"blocks\""), "{out}");
        assert!(!out.contains("glow"), "back at its default: {out}");
        assert!(out.contains("tint = [0.35, 1.0, 0.45]"), "{out}");
        assert!(
            out.contains("mystery = 3"),
            "a key the style doesn't own stays: {out}"
        );
        assert!(
            out.contains("kuwahara_radius = 4"),
            "another style's old keys stay: {out}"
        );
        // Still parses, and the entities came through
        let reread: toml::Table = toml::from_str(&out).unwrap();
        assert!(reread["entities"]["lamp"].is_table());
        assert_eq!(
            reread["post_process"]["styles"]["ascii"]["cell"].as_integer(),
            Some(16)
        );
    }

    #[test]
    fn a_new_style_table_lands_with_the_post_process_tables() {
        let mut doc = SceneDocument::from_str(COMMENTED).unwrap();
        doc.set_style_table(
            "pencil",
            "[post_process.styles.pencil]\nspacing = 5.0\n",
            &["enabled", "spacing"],
        )
        .unwrap();
        let out = doc.to_string();
        let pencil = out.find("[post_process.styles.pencil]").expect(&out);
        let lamp = out.find("[entities.lamp]").unwrap();
        assert!(
            pencil < lamp,
            "beside the other styles, not after the entities:\n{out}"
        );
        assert!(
            out.contains("[post_process.styles.pencil]\nspacing = 5.0"),
            "{out}"
        );
    }

    #[test]
    fn a_scene_without_post_process_gets_only_the_style_header() {
        let mut doc = SceneDocument::from_str("[scene]\nname = \"Bare\"\n").unwrap();
        doc.set_style_table(
            "ascii",
            "[post_process.styles.ascii]\ncell = 6\n",
            ASCII_KEYS,
        )
        .unwrap();
        let out = doc.to_string();
        assert!(!out.contains("[post_process]\n"), "{out}");
        assert!(!out.contains("[post_process.styles]\n"), "{out}");
        assert!(
            out.contains("[post_process.styles.ascii]\ncell = 6"),
            "{out}"
        );
        // An on style with nothing changed is an empty table: it switches on
        let mut doc = SceneDocument::from_str("[scene]\nname = \"Bare\"\n").unwrap();
        doc.set_style_table("ascii", "[post_process.styles.ascii]\n", ASCII_KEYS)
            .unwrap();
        let reread: toml::Table = toml::from_str(&doc.to_string()).unwrap();
        assert!(reread["post_process"]["styles"]["ascii"]
            .as_table()
            .unwrap()
            .is_empty());
    }

    #[test]
    fn an_off_style_with_nothing_set_loses_its_table() {
        let mut doc = SceneDocument::from_str(COMMENTED).unwrap();
        doc.set_style_table(
            "ascii",
            "[post_process.styles.ascii]\nenabled = false\n",
            ASCII_KEYS,
        )
        .unwrap();
        let out = doc.to_string();
        assert!(!out.contains("styles.ascii"), "{out}");
        assert!(out.contains("bloom_intensity = 0.1 # soft"), "{out}");
        // Nothing to remove and no table to make: the file is untouched
        let mut bare = SceneDocument::from_str("[scene]\nname = \"Bare\"\n").unwrap();
        bare.set_style_table(
            "ascii",
            "[post_process.styles.ascii]\nenabled = false\n",
            ASCII_KEYS,
        )
        .unwrap();
        assert_eq!(bare.to_string(), "[scene]\nname = \"Bare\"\n");
    }

    #[test]
    fn committing_kuwahara_drops_its_old_keys() {
        let mut doc = SceneDocument::from_str(COMMENTED).unwrap();
        doc.set_style_table(
            "kuwahara",
            "[post_process.styles.kuwahara]\nsharpness = 9.0\n",
            &["enabled", "radius", "sharpness"],
        )
        .unwrap();
        let out = doc.to_string();
        assert!(!out.contains("kuwahara_radius"), "{out}");
        assert!(
            out.contains("[post_process.styles.kuwahara]\nsharpness = 9.0"),
            "{out}"
        );
    }

    /// A scene carrying every top-level table the format has, and comments
    /// (#677): what each writer must leave alone.
    const EVERY_TABLE: &str = r#"# The grove at dusk. Keep this comment.
[scene]
name = "Grove at dusk"
version = "1.0"
description = "Every top-level table"
input_config = "grove.input.toml"
preload_audio = false

[camera]
position = [0.0, 4.0, 12.0]
target = [0.0, 1.0, 0.0]
fov = 50.0

[environment]
ambient = 0.3

[post_process]
bloom_intensity = 0.05 # soft
look = "ink_wash"

[post_process.styles.kuwahara]
radius = 5

[living]
stagger = "per_level"

[wooze]
preset = "linklater"
glide = 0.8

[[wooze.zone]]
name = "pocket"
shape = "sphere"
center = [0, 0, 12]
radius = 6
falloff = 4

[prefabs.porch]
template = "lamp"
prefix = "porch"

# The door, which the fix touches
[entities.door]
archetype = "door"

[entities.door.door]
open_angle = 200.0 # too far
locked = false

[entities.door.transform]
position = [1, 0, 0] # written as integers

[entities.lamp]
parent = "door"

[entities.lamp.light]
intensity = 2.0
"#;

    const TOP_LEVEL: [&str; 8] = [
        "scene",
        "camera",
        "environment",
        "post_process",
        "living",
        "wooze",
        "prefabs",
        "entities",
    ];

    fn every_table_world() -> FlintWorld {
        let registry = flint_schema::SchemaRegistry::new();
        crate::loader::load_scene_string(EVERY_TABLE, &registry)
            .unwrap()
            .0
    }

    /// Every line of `before` but `except` is in `after`, in order.
    fn kept_lines(before: &str, after: &str, except: &[&str]) {
        let mut rest = after.lines();
        for line in before.lines().filter(|l| !except.contains(l)) {
            assert!(rest.any(|l| l == line), "lost or moved {line:?}:\n{after}");
        }
    }

    #[test]
    fn a_world_with_nothing_changed_writes_the_file_byte_for_byte() {
        let world = every_table_world();
        let mut doc = SceneDocument::from_str(EVERY_TABLE).unwrap();
        let problems = doc.apply_world_diff(&world, &every_table_world());
        assert!(problems.is_empty(), "{problems:?}");
        assert_eq!(doc.to_string(), EVERY_TABLE);
    }

    #[test]
    fn patching_a_field_to_what_it_says_changes_nothing() {
        // The viewer patches every field it touched, an undone edit included
        let mut doc = SceneDocument::from_str(EVERY_TABLE).unwrap();
        let same = toml::Value::Array(vec![
            toml::Value::Float(1.0),
            toml::Value::Float(0.0),
            toml::Value::Float(0.0),
        ]);
        doc.patch_field("door", "transform", "position", &same)
            .unwrap();
        doc.patch_field("door", "door", "locked", &toml::Value::Boolean(false))
            .unwrap();
        assert_eq!(doc.to_string(), EVERY_TABLE);
    }

    #[test]
    fn a_crlf_file_keeps_its_line_endings() {
        // A Windows checkout (core.autocrlf): toml_edit alone writes \n
        let crlf = EVERY_TABLE.replace('\n', "\r\n");
        let mut doc = SceneDocument::from_str(&crlf).unwrap();
        assert_eq!(doc.to_string(), crlf);
        doc.patch_field("lamp", "light", "intensity", &toml::Value::Float(3.0))
            .unwrap();
        let out = doc.to_string();
        assert!(out.contains("intensity = 3.0\r\n"), "{out:?}");
        assert!(!out.replace("\r\n", "").contains('\n'), "{out:?}");
    }

    #[test]
    fn a_patched_field_keeps_its_comment() {
        let mut doc = SceneDocument::from_str(EVERY_TABLE).unwrap();
        doc.patch_field("door", "door", "open_angle", &toml::Value::Float(90.0))
            .unwrap();
        let out = doc.to_string();
        assert!(out.contains("open_angle = 90.0 # too far"), "{out}");
        kept_lines(EVERY_TABLE, &out, &["open_angle = 200.0 # too far"]);
    }

    #[test]
    fn every_writer_keeps_every_top_level_table() {
        // The viewer's writer: field patches
        let mut viewer = SceneDocument::from_str(EVERY_TABLE).unwrap();
        viewer
            .patch_field("lamp", "light", "intensity", &toml::Value::Float(3.5))
            .unwrap();
        // validate --fix's writer: what changed between two worlds
        let before = every_table_world();
        let mut after = every_table_world();
        let door = after.get_id("door").unwrap();
        let comps = after.get_components_mut(door).unwrap();
        comps.set_field("door", "open_angle", toml::Value::Float(90.0));
        let mut fixer = SceneDocument::from_str(EVERY_TABLE).unwrap();
        assert!(fixer.apply_world_diff(&before, &after).is_empty());

        for (writer, doc, changed) in [
            ("viewer", viewer, "intensity = 2.0"),
            ("validate --fix", fixer, "open_angle = 200.0 # too far"),
        ] {
            let out = doc.to_string();
            let parsed: toml::Table = toml::from_str(&out).unwrap();
            for table in TOP_LEVEL {
                assert!(
                    parsed.contains_key(table),
                    "{writer} lost [{table}]:\n{out}"
                );
            }
            kept_lines(EVERY_TABLE, &out, &[changed]);
            let file: crate::SceneFile = toml::from_str(&out).unwrap();
            assert_eq!(file.scene.name, "Grove at dusk", "{writer}");
            assert_eq!(
                file.scene.description.as_deref(),
                Some("Every top-level table")
            );
            assert!(!file.scene.preload_audio, "{writer}");
            assert!(file.prefabs.contains_key("porch"), "{writer}");
        }
    }

    #[test]
    fn a_world_diff_writes_removed_fields_components_and_new_entities() {
        let before = every_table_world();
        let mut after = every_table_world();
        let door = after.get_id("door").unwrap();
        let comps = after.get_components_mut(door).unwrap();
        // remove_invalid: the field goes
        comps
            .get_mut("door")
            .and_then(|v| v.as_table_mut())
            .unwrap()
            .remove("open_angle");
        // add_child: a new entity, parented
        let handle = after.spawn("door_handle").unwrap();
        let comps = after.get_components_mut(handle).unwrap();
        comps.archetype = Some("handle".into());
        comps.set_field("transform", "position", toml::Value::Array(vec![]));
        after.set_parent(handle, door).unwrap();
        // a lamp despawned
        after.despawn_by_name("lamp").unwrap();

        let mut doc = SceneDocument::from_str(EVERY_TABLE).unwrap();
        let problems = doc.apply_world_diff(&before, &after);
        assert!(problems.is_empty(), "{problems:?}");
        let out = doc.to_string();
        assert!(!out.contains("open_angle"), "{out}");
        assert!(out.contains("locked = false"), "{out}");
        assert!(!out.contains("[entities.lamp]"), "{out}");
        let file: crate::SceneFile = toml::from_str(&out).unwrap();
        let handle = &file.entities["door_handle"];
        assert_eq!(handle.archetype.as_deref(), Some("handle"));
        assert_eq!(handle.parent.as_deref(), Some("door"));
        assert!(handle.components.contains_key("transform"));
        assert!(out.starts_with("# The grove at dusk. Keep this comment.\n"));
        assert!(file.wooze.is_some() && file.camera.is_some() && file.living.is_some());
    }

    #[test]
    fn a_change_to_an_entity_the_file_does_not_write_is_reported() {
        // An entity a prefab instance expands into is in the world only
        let mut before = every_table_world();
        before.spawn("porch_bulb").unwrap();
        let mut after = every_table_world();
        let bulb = after.spawn("porch_bulb").unwrap();
        after.get_components_mut(bulb).unwrap().set_field(
            "light",
            "intensity",
            toml::Value::Float(9.0),
        );
        let mut doc = SceneDocument::from_str(EVERY_TABLE).unwrap();
        let problems = doc.apply_world_diff(&before, &after);
        assert_eq!(problems.len(), 1, "{problems:?}");
        assert!(problems[0].contains("porch_bulb"), "{problems:?}");
        assert_eq!(doc.to_string(), EVERY_TABLE);
    }
}
