//! Read-modify-write helper for `animator.layers` entries, shared by the
//! Rhai animation API and the sequence runner so the two never disagree on
//! slot padding or legacy-field migration.

use flint_core::components as comp;
use flint_core::toml_util::toml_f64;
use flint_ecs::DynamicComponents;

/// Edit one entry of `animator.layers` in place.
///
/// Migrates the legacy `layer_clip`/`layer_weight` pair into slot 0 first,
/// pads slots below `index` with inactive (empty-clip) entries so indices
/// stay stable, applies `edit`, then clears the legacy field.
pub fn edit_layer_table(
    comps: &mut DynamicComponents,
    index: usize,
    edit: impl FnOnce(&mut toml::map::Map<String, toml::Value>),
) {
    // Layer IDs are serialized in u8 metadata elsewhere, so anything at or
    // above the u8 limit cannot be represented without overflowing the runtime
    // bookkeeping and growing the slots unboundedly.
    if index >= u8::MAX as usize {
        return;
    }

    let animator = comps
        .get(comp::ANIMATOR)
        .cloned()
        .unwrap_or(toml::Value::Table(Default::default()));

    let mut layers: Vec<toml::Value> = animator
        .get("layers")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();

    if layers.is_empty() {
        let legacy = animator
            .get("layer_clip")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        if !legacy.is_empty() {
            let weight = animator
                .get("layer_weight")
                .and_then(toml_f64)
                .unwrap_or(1.0);
            let mut t = toml::map::Map::new();
            t.insert("clip".into(), toml::Value::String(legacy.to_string()));
            t.insert("weight".into(), toml::Value::Float(weight));
            layers.push(toml::Value::Table(t));
        }
    }

    while layers.len() <= index {
        let mut t = toml::map::Map::new();
        t.insert("clip".into(), toml::Value::String(String::new()));
        layers.push(toml::Value::Table(t));
    }
    if !layers[index].is_table() {
        layers[index] = toml::Value::Table(Default::default());
    }
    if let Some(t) = layers[index].as_table_mut() {
        edit(t);
    }

    comps.set_field(comp::ANIMATOR, "layers", toml::Value::Array(layers));
    comps.set_field(
        comp::ANIMATOR,
        "layer_clip",
        toml::Value::String(String::new()),
    );
}

/// Set a layer's weight instantly, cancelling any fade in flight.
pub fn set_weight(t: &mut toml::map::Map<String, toml::Value>, weight: f64) {
    t.insert("weight".into(), toml::Value::Float(weight));
    t.insert("fade_duration".into(), toml::Value::Float(0.0));
}

/// Ramp a layer's weight toward `weight` over `duration` seconds
/// (instant when `duration <= 0`).
pub fn fade_weight(t: &mut toml::map::Map<String, toml::Value>, weight: f64, duration: f64) {
    if duration <= 0.0 {
        set_weight(t, weight);
        return;
    }
    t.insert("fade_target".into(), toml::Value::Float(weight));
    t.insert("fade_duration".into(), toml::Value::Float(duration));
}
