//! Committing a tuned Living Props look to its scene file (#249).
//!
//! A look is a [`WoozePreset`]: the field's global knobs, the scene's `[living]`
//! with its `[living.species.<spec>]` tables, and the boil. What a commit writes
//! is the difference between two looks, the one the file gives and the one
//! running now, as one [`SceneEdit`] a value. Each lands through
//! [`SceneDocument::apply_edit`], which changes only its own line: comments,
//! order, inline tables and everything else in the file stay as they were.

use super::{LivingDef, LivingEchoesDef, LivingSpeciesDef, LivingVarianceDef, WoozePreset};
use flint_scene::{SceneDocument, SceneEdit, SceneScalar};
use std::path::Path;

/// The species an edit is about, when it is about one species' table
/// (`[living.species.<name>]`).
pub fn edit_species(edit: &SceneEdit) -> Option<&str> {
    match edit.table.as_slice() {
        [living, species, name] if living == "living" && species == "species" => Some(name),
        _ => None,
    }
}

/// A value's place: its table's header and its key inside.
type Place = (Vec<String>, Vec<String>);

/// Values by place, in the order the format lists them, so new lines are
/// written as `{ count = 3, alpha = 0.35 }` and `stagger` before `echoes`.
#[derive(Default)]
struct Entries(Vec<(Place, SceneScalar)>);

impl Entries {
    fn get(&self, place: &Place) -> Option<&SceneScalar> {
        self.0.iter().find(|(p, _)| p == place).map(|(_, v)| v)
    }

    fn places(&self) -> impl Iterator<Item = &Place> {
        self.0.iter().map(|(p, _)| p)
    }
}

fn put(out: &mut Entries, table: &[&str], key: &str, value: Option<SceneScalar>) {
    if let Some(value) = value {
        let table = table.iter().map(|s| s.to_string()).collect();
        let key = key.split('.').map(str::to_string).collect();
        out.0.push(((table, key), value));
    }
}

fn float(value: Option<f32>) -> Option<SceneScalar> {
    value.map(SceneScalar::Float)
}

fn text(value: &Option<String>) -> Option<SceneScalar> {
    value.clone().map(SceneScalar::Text)
}

/// Stagger, echoes and variance, wherever a table holds them.
fn props_entries(
    out: &mut Entries,
    table: &[&str],
    stagger: &Option<String>,
    echoes: Option<LivingEchoesDef>,
    variance: Option<LivingVarianceDef>,
) {
    put(out, table, "stagger", text(stagger));
    if let Some(echoes) = echoes {
        let count = echoes.count.map(|count| SceneScalar::Int(i64::from(count)));
        put(out, table, "echoes.count", count);
        put(out, table, "echoes.spread", float(echoes.spread));
        put(out, table, "echoes.alpha", float(echoes.alpha));
    }
    if let Some(variance) = variance {
        put(out, table, "variance.amount", float(variance.amount));
        put(out, table, "variance.steady", float(variance.steady));
        let keep = variance.keep_size.map(SceneScalar::Bool);
        put(out, table, "variance.keep_size", keep);
    }
}

/// Every value one species' table holds, by where it goes in a scene.
fn species_entries(out: &mut Entries, name: &str, def: &LivingSpeciesDef) {
    let table = ["living", "species", name];
    put(out, &table, "amplitude", float(def.amplitude));
    put(out, &table, "step", float(def.step));
    put(out, &table, "glide", float(def.glide));
    put(out, &table, "jitter", float(def.jitter));
    props_entries(out, &table, &def.stagger, def.echoes, def.variance);
    put(out, &table, "softness", float(def.softness));
}

/// Every value a look holds, by where it goes in a scene: `[wooze]`, `[living]`,
/// each `[living.species.<spec>]`, and the boil's three in `[post_process]`. A
/// boil that's off is amounts of 0 there, which is all a scene can say.
fn entries(look: &WoozePreset) -> Entries {
    let mut out = Entries::default();
    let wooze = ["wooze"];
    put(&mut out, &wooze, "amplitude", float(look.wooze.amplitude));
    put(&mut out, &wooze, "step", float(look.wooze.step));
    put(&mut out, &wooze, "glide", float(look.wooze.glide));
    put(&mut out, &wooze, "jitter", float(look.wooze.jitter));

    let living: &LivingDef = &look.living;
    put(&mut out, &["living"], "shadows", text(&living.shadows));
    props_entries(
        &mut out,
        &["living"],
        &living.stagger,
        living.echoes,
        living.variance,
    );
    put(&mut out, &["living"], "softness", float(living.softness));
    for (name, def) in &living.species {
        species_entries(&mut out, name, def);
    }

    if let Some(boil) = look.boil {
        let on = boil.enabled.unwrap_or(true);
        let amount =
            |value: Option<f32>| value.map(|v| SceneScalar::Float(if on { v } else { 0.0 }));
        let post = ["post_process"];
        put(&mut out, &post, "kuwahara_boil", amount(boil.kuwahara));
        put(&mut out, &post, "contour_boil", amount(boil.contour));
        let cadence = boil.on.map(|on| SceneScalar::Int(i64::from(on)));
        put(&mut out, &post, "boil_on", cadence);
    }
    out
}

/// What a scene file needs changing to go from giving `base` to giving `now`: a
/// value for each that differs, set to what `now` says, or taken out where `now`
/// says nothing. Values that are the same aren't touched, wherever they came
/// from, so a value a preset gives stays the preset's.
pub fn look_edits(base: &WoozePreset, now: &WoozePreset) -> Vec<SceneEdit> {
    let was = entries(base);
    let is = entries(now);
    let mut places: Vec<&Place> = is.places().collect();
    for place in was.places() {
        if !places.contains(&place) {
            places.push(place);
        }
    }
    places
        .into_iter()
        .filter(|key| was.get(key) != is.get(key))
        .map(|key| SceneEdit {
            table: key.0.clone(),
            key: key.1.clone(),
            value: is.get(key).cloned(),
        })
        .collect()
}

/// Fit `edits` to the file `doc` holds, returning whether boil values were
/// left out: with no `[post_process]` they go unwritten, since making one
/// would turn passes on. A boil goes where the scene keeps it (#599):
/// `boil` in the style's table when the file has one, else the old flat
/// key, which loads the same and makes no table (a table would switch the
/// style on).
pub(crate) fn fit_edits(doc: &SceneDocument, edits: &mut Vec<SceneEdit>) -> bool {
    let mut skipped = false;
    if !doc.has_table(&["post_process"]) {
        let before = edits.len();
        edits.retain(|edit| edit.table != ["post_process"]);
        skipped = edits.len() != before;
    }
    for edit in edits.iter_mut() {
        let style = match (edit.table.as_slice(), edit.key.as_slice()) {
            ([post], [key]) if post == "post_process" && key == "kuwahara_boil" => "kuwahara",
            ([post], [key]) if post == "post_process" && key == "contour_boil" => "contour",
            _ => continue,
        };
        if doc.has_table(&["post_process", "styles", style]) {
            edit.table = vec!["post_process".into(), "styles".into(), style.into()];
            edit.key = vec!["boil".into()];
        }
    }
    skipped
}

/// What came of a commit.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct CommitReport {
    /// Each value written or taken out, in the file's terms.
    pub edits: Vec<SceneEdit>,
    /// Values taken out of the scene that the preset it names still gives, so a
    /// fresh run won't look as this one does until they're taken out there too.
    pub preset_still_sets: Vec<String>,
    /// Boil values left unwritten, since the scene has no `[post_process]` to put
    /// them in and making one would turn passes on.
    pub boil_skipped: bool,
}

/// Write what's changed between `base`, what the scene file at `path` gives, and
/// `now`, keeping everything else in the file as it is. Not for the web, which
/// has no scene file to write.
pub fn commit_look(
    path: &Path,
    base: &WoozePreset,
    now: &WoozePreset,
) -> Result<CommitReport, String> {
    if cfg!(target_arch = "wasm32") {
        return Err("the web player has no scene file to write".to_string());
    }
    let mut report = CommitReport::default();
    let mut edits = look_edits(base, now);
    if edits.is_empty() {
        return Ok(report);
    }
    let mut doc = SceneDocument::from_file(path)?;
    report.boil_skipped = fit_edits(&doc, &mut edits);
    for edit in &edits {
        doc.apply_edit(edit)?;
    }
    doc.save(path)?;

    // Whatever the preset the scene names says, under the scene's own values.
    let text = flint_core::vfs::read_to_string(path).unwrap_or_default();
    let named = toml::from_str::<flint_scene::SceneFile>(&text)
        .ok()
        .and_then(|scene| super::LivingTables::of(&scene).ok())
        .and_then(|tables| tables.wooze.and_then(|w| w.preset));
    if let Some(Ok(preset)) = named.and_then(|name| super::read_preset(path, &name)) {
        let gives = entries(&preset);
        for edit in edits.iter().filter(|edit| edit.value.is_none()) {
            if gives.get(&(edit.table.clone(), edit.key.clone())).is_some() {
                report.preset_still_sets.push(edit.label());
            }
        }
    }
    report.edits = edits;
    Ok(report)
}

#[cfg(test)]
#[path = "commit_tests.rs"]
mod tests;
