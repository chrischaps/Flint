//! Composable texture pipeline — executes a sequence of [`TextureOp`]s.
//!
//! The pipeline pattern (`pattern = "pipeline"`) reads `[[params.ops]]` from the
//! spec TOML, instantiates each op, and runs them sequentially on a
//! [`TextureField`]. This replaces the monolithic pattern → derive_maps flow
//! with a composable stage-based architecture.
//!
//! Supports two execution modes:
//! - **Sequential** (legacy): ops run in TOML array order, sharing a single field
//! - **DAG** (explicit wiring): ops declare `id` and `inputs`, executed in
//!   topological order with scoped channels for isolation

use std::collections::HashMap;

use crate::types::ImageData;
use crate::{ProcGenError, Result, SeededRng};

use super::descriptor::OpDescriptor;
use super::field::TextureField;
use super::maps::TextureMapParams;
use super::node_graph;
use super::ops::{self, TextureOp};

// ─── Pipeline Execution ───────────────────────────────────────────────────

/// Run the composable texture pipeline defined by `[[params.ops]]`.
///
/// Returns one [`ImageData`] per requested output map (albedo, normal, roughness).
/// Automatically dispatches to DAG execution when any op has an `id` field.
pub fn run_pipeline(
    params: &toml::Value,
    map_params: &TextureMapParams,
    rng: &mut SeededRng,
) -> Result<Vec<ImageData>> {
    let (ops_array, ops) = parse_pipeline(params)?;

    if node_graph::has_explicit_connections(&ops_array) {
        return run_pipeline_dag(&ops_array, &ops, map_params, rng);
    }

    // Sequential execution (legacy)
    let mut field =
        TextureField::new(map_params.width, map_params.height).with_seamless(map_params.seamless);
    for (i, op) in ops.iter().enumerate() {
        let mut op_rng = rng.fork(&format!("op_{i}"));
        op.apply(&mut field, &mut op_rng);
    }

    Ok(extract_output_maps(&field, map_params))
}

/// Run the pipeline and return intermediate field snapshots after each op.
///
/// Returns `(output_images, snapshots)` where `snapshots[i]` is the field
/// state after op `i` has been applied.
/// Automatically dispatches to DAG execution when any op has an `id` field.
pub fn run_pipeline_with_snapshots(
    params: &toml::Value,
    map_params: &TextureMapParams,
    rng: &mut SeededRng,
) -> Result<(Vec<ImageData>, Vec<TextureField>)> {
    let (ops_array, ops) = parse_pipeline(params)?;

    if node_graph::has_explicit_connections(&ops_array) {
        return run_pipeline_dag_with_snapshots(&ops_array, &ops, map_params, rng);
    }

    // Sequential execution (legacy)
    let mut field =
        TextureField::new(map_params.width, map_params.height).with_seamless(map_params.seamless);
    let mut snapshots = Vec::with_capacity(ops.len());

    for (i, op) in ops.iter().enumerate() {
        let mut op_rng = rng.fork(&format!("op_{i}"));
        op.apply(&mut field, &mut op_rng);
        snapshots.push(field.clone());
    }

    let results = extract_output_maps(&field, map_params);
    Ok((results, snapshots))
}

// ─── Shared Helpers ──────────────────────────────────────────────────────

/// Parse and validate the ops array from params, returning the raw TOML and parsed ops.
fn parse_pipeline(params: &toml::Value) -> Result<(Vec<toml::Value>, Vec<Box<dyn TextureOp>>)> {
    let table = params
        .as_table()
        .ok_or_else(|| ProcGenError::InvalidParameter {
            name: "params".into(),
            reason: "expected a TOML table".into(),
        })?;

    let ops_array = table.get("ops").and_then(|v| v.as_array()).ok_or_else(|| {
        ProcGenError::InvalidParameter {
            name: "ops".into(),
            reason: "pipeline pattern requires a [[params.ops]] array".into(),
        }
    })?;

    if ops_array.is_empty() {
        return Err(ProcGenError::InvalidParameter {
            name: "ops".into(),
            reason: "pipeline requires at least one op".into(),
        });
    }

    let ops: Vec<Box<dyn TextureOp>> = ops_array
        .iter()
        .enumerate()
        .map(|(i, v)| parse_op(v, i))
        .collect::<Result<_>>()?;

    Ok((ops_array.clone(), ops))
}

/// Extract output images from a field based on requested map types.
fn extract_output_maps(field: &TextureField, map_params: &TextureMapParams) -> Vec<ImageData> {
    let mut results = Vec::with_capacity(map_params.output_maps.len());
    for map_name in &map_params.output_maps {
        match map_name.as_str() {
            "albedo" => results.push(field.to_albedo_image()),
            "normal" => results.push(field.to_normal_image()),
            "roughness" => results.push(field.to_roughness_image()),
            _ => {}
        }
    }
    results
}

// ─── DAG Execution ──────────────────────────────────────────────────────

/// Metadata for a node in the DAG pipeline.
struct DagNode {
    /// Node ID (from `id` field in TOML).
    id: Option<String>,
    /// Input mappings: `local_channel -> "source_node.channel"`.
    inputs: HashMap<String, String>,
    /// Original index in the TOML ops array (for snapshot ordering).
    original_index: usize,
}

/// Parse node metadata (id, inputs) from the ops array.
fn parse_dag_nodes(ops_array: &[toml::Value]) -> Vec<DagNode> {
    ops_array
        .iter()
        .enumerate()
        .map(|(i, v)| {
            let table = v.as_table();
            let id = table
                .and_then(|t| t.get("id"))
                .and_then(|v| v.as_str())
                .map(|s| s.to_string());
            let inputs = table
                .and_then(|t| t.get("inputs"))
                .and_then(|v| v.as_table())
                .map(|t| {
                    t.iter()
                        .filter_map(|(k, v)| v.as_str().map(|s| (k.clone(), s.to_string())))
                        .collect()
                })
                .unwrap_or_default();
            DagNode {
                id,
                inputs,
                original_index: i,
            }
        })
        .collect()
}

/// Resolve a dynamic placeholder like `<target>` or `<source>` from the op's
/// TOML params.  Returns `None` if the param is absent.
fn resolve_dynamic_channel(op_toml: &toml::Value, placeholder: &str) -> Option<String> {
    // placeholder is e.g. "<target>" — extract the param name between < and >
    let param_name = placeholder.strip_prefix('<')?.strip_suffix('>')?;
    op_toml
        .as_table()
        .and_then(|t| t.get(param_name))
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
}

/// Get the output channel names an op produces, resolving dynamic placeholders.
fn op_output_channels(op: &dyn TextureOp, op_toml: &toml::Value) -> Vec<String> {
    let info = op.port_info();
    let mut channels = Vec::new();
    for ch in info.writes.iter().chain(info.modifies.iter()) {
        if ch.starts_with('<') && ch.ends_with('>') {
            if let Some(resolved) = resolve_dynamic_channel(op_toml, ch) {
                channels.push(resolved);
            }
        } else {
            channels.push(ch.to_string());
        }
    }
    channels
}

/// Run the pipeline in DAG mode with topological sort and scoped channels.
fn run_pipeline_dag(
    ops_array: &[toml::Value],
    ops: &[Box<dyn TextureOp>],
    map_params: &TextureMapParams,
    rng: &mut SeededRng,
) -> Result<Vec<ImageData>> {
    let dag_nodes = parse_dag_nodes(ops_array);
    let execution_order = compute_dag_order(&dag_nodes)?;

    let mut field =
        TextureField::new(map_params.width, map_params.height).with_seamless(map_params.seamless);

    for &exec_idx in &execution_order {
        execute_dag_node(
            exec_idx,
            &dag_nodes,
            ops[exec_idx].as_ref(),
            &ops_array[exec_idx],
            ops_array,
            &mut field,
            rng,
        );
    }

    // Resolve output: copy the last writer's scoped channels to bare names
    resolve_output_channels(&dag_nodes, ops, ops_array, &mut field);

    Ok(extract_output_maps(&field, map_params))
}

/// Run the pipeline in DAG mode, capturing per-op snapshots.
///
/// Snapshots are indexed by original TOML position for the editor.
fn run_pipeline_dag_with_snapshots(
    ops_array: &[toml::Value],
    ops: &[Box<dyn TextureOp>],
    map_params: &TextureMapParams,
    rng: &mut SeededRng,
) -> Result<(Vec<ImageData>, Vec<TextureField>)> {
    let dag_nodes = parse_dag_nodes(ops_array);
    let execution_order = compute_dag_order(&dag_nodes)?;

    let mut field =
        TextureField::new(map_params.width, map_params.height).with_seamless(map_params.seamless);
    let mut snapshots_by_idx: Vec<Option<TextureField>> = vec![None; ops.len()];

    for &exec_idx in &execution_order {
        execute_dag_node(
            exec_idx,
            &dag_nodes,
            ops[exec_idx].as_ref(),
            &ops_array[exec_idx],
            ops_array,
            &mut field,
            rng,
        );
        snapshots_by_idx[dag_nodes[exec_idx].original_index] = Some(field.clone());
    }

    resolve_output_channels(&dag_nodes, ops, ops_array, &mut field);

    // Fill any missing snapshots with empty fields
    let snapshots: Vec<TextureField> = snapshots_by_idx
        .into_iter()
        .map(|opt| opt.unwrap_or_else(|| TextureField::new(map_params.width, map_params.height)))
        .collect();

    let results = extract_output_maps(&field, map_params);
    Ok((results, snapshots))
}

/// Build dependency edges from DAG node inputs and topologically sort.
fn compute_dag_order(dag_nodes: &[DagNode]) -> Result<Vec<usize>> {
    // Build id → index lookup
    let id_to_idx: HashMap<&str, usize> = dag_nodes
        .iter()
        .enumerate()
        .filter_map(|(i, n)| n.id.as_deref().map(|id| (id, i)))
        .collect();

    let mut edges = Vec::new();
    for (to_idx, node) in dag_nodes.iter().enumerate() {
        for ref_str in node.inputs.values() {
            if let Some((src_id, _)) = ref_str.split_once('.') {
                if let Some(&from_idx) = id_to_idx.get(src_id) {
                    edges.push((from_idx, to_idx));
                }
            }
        }
    }

    node_graph::topological_sort(dag_nodes.len(), &edges)
}

/// Execute a single DAG node: copy-in scoped inputs, run op, copy-out scoped outputs.
fn execute_dag_node(
    idx: usize,
    dag_nodes: &[DagNode],
    op: &dyn TextureOp,
    op_toml: &toml::Value,
    all_ops_toml: &[toml::Value],
    field: &mut TextureField,
    rng: &mut SeededRng,
) {
    let node = &dag_nodes[idx];

    // Build id → index lookup for resolving source placeholders
    let id_to_idx: HashMap<&str, usize> = dag_nodes
        .iter()
        .enumerate()
        .filter_map(|(i, n)| n.id.as_deref().map(|id| (id, i)))
        .collect();

    // Copy-in: for each declared input, copy source node's scoped channel to bare name.
    // Both the local channel key and the source channel reference may use
    // `<placeholder>` syntax (e.g. `"<target>" = "blend.<target>"`), which must
    // be resolved from the respective op's TOML params.
    for (local_channel, ref_str) in &node.inputs {
        if let Some((src_id, src_channel)) = ref_str.split_once('.') {
            // Resolve <placeholder> in the source channel from the source op's TOML
            let resolved_src = if src_channel.starts_with('<') && src_channel.ends_with('>') {
                id_to_idx
                    .get(src_id)
                    .and_then(|&si| resolve_dynamic_channel(&all_ops_toml[si], src_channel))
                    .unwrap_or_else(|| src_channel.to_string())
            } else {
                src_channel.to_string()
            };

            // Resolve <placeholder> in the local channel from the current op's TOML
            let resolved_local = if local_channel.starts_with('<') && local_channel.ends_with('>') {
                resolve_dynamic_channel(op_toml, local_channel)
                    .unwrap_or_else(|| local_channel.to_string())
            } else {
                local_channel.to_string()
            };

            let scoped_src = format!("{src_id}/{resolved_src}");
            field.copy_channel(&scoped_src, &resolved_local);
        }
    }

    // Run the op (reads/writes bare channel names)
    let mut op_rng = rng.fork(&format!("op_{}", node.original_index));
    op.apply(field, &mut op_rng);

    // Copy-out: copy bare output channels to scoped names
    if let Some(id) = &node.id {
        let out_channels = op_output_channels(op, op_toml);
        for ch in &out_channels {
            let scoped_dst = format!("{id}/{ch}");
            field.copy_channel(ch, &scoped_dst);
        }
    }
}

/// After DAG execution, resolve the final bare channels for output extraction.
///
/// For each standard output channel, find the last node (in topo order) that
/// produced it and copy from the scoped version to the bare name.
fn resolve_output_channels(
    dag_nodes: &[DagNode],
    ops: &[Box<dyn TextureOp>],
    ops_array: &[toml::Value],
    field: &mut TextureField,
) {
    // Track which node last wrote each channel
    let mut last_writer: HashMap<String, String> = HashMap::new(); // channel -> node_id

    for (i, node) in dag_nodes.iter().enumerate() {
        if let Some(id) = &node.id {
            let out_channels = op_output_channels(ops[i].as_ref(), &ops_array[i]);
            for ch in &out_channels {
                last_writer.insert(ch.clone(), id.clone());
            }
        }
    }

    // Copy scoped channels to bare names for output extraction
    for (channel, node_id) in &last_writer {
        let scoped = format!("{node_id}/{channel}");
        field.copy_channel(&scoped, channel);
    }
}

/// Every op, in the order the editor lists them: the registry (#720). An
/// op's struct, `impl TextureOp` and [`OpDescriptor`] live in its family's
/// module under `ops/`;
/// adding one is those and a line here.
pub const OPS: &[&OpDescriptor] = &[
    &ops::BRICK_GRID,
    &ops::VORONOI_GRID,
    &ops::DOMAIN_WARP,
    &ops::CELL_HEIGHT,
    &ops::CELL_BULGE,
    &ops::NOISE_LAYER,
    &ops::BLEND,
    &ops::MORTAR_GROOVE,
    &ops::CELL_COLOR,
    &ops::MORTAR_COLOR,
    &ops::DERIVE_NORMAL,
    &ops::CELL_ROUGHNESS,
    &ops::MATH,
    &ops::MAP_RANGE,
    &ops::COLOR_RAMP,
    &ops::CHECKER_TEXTURE,
    &ops::GRADIENT_TEXTURE,
    &ops::WAVE_TEXTURE,
    &ops::WHITE_NOISE,
    &ops::VORONOI_TEXTURE,
    &ops::MUSGRAVE_TEXTURE,
    &ops::INVERT,
    &ops::BRIGHTNESS_CONTRAST,
    &ops::HSV_ADJUST,
    &ops::GAMMA,
    &ops::CLAMP,
    &ops::BLUR,
    &ops::SHARPEN,
    &ops::EDGE_DETECT,
    &ops::EDGE_ERODE,
];

/// All known op type identifiers: the registry's keys, in its order.
pub const ALL_OP_TYPES: &[&str] = &{
    let mut names = [""; OPS.len()];
    let mut i = 0;
    while i < OPS.len() {
        names[i] = OPS[i].type_name;
        i += 1;
    }
    names
};

/// The descriptor of the op named `op_type`, if there is one.
pub fn op_descriptor(op_type: &str) -> Option<&'static OpDescriptor> {
    OPS.iter().copied().find(|d| d.type_name == op_type)
}

/// Return a JSON Schema describing the parameters for a given op type
/// (empty for an unknown one), from its [`OpDescriptor`].
pub fn op_param_schema(op_type: &str) -> serde_json::Value {
    match op_descriptor(op_type) {
        Some(d) => d.param_schema(),
        None => serde_json::json!({ "type": "object", "properties": {} }),
    }
}

/// The full, closed schema of one `[[params.ops]]` table of type `op_type`:
/// its [`op_param_schema`] knobs plus the keys every op may carry (`type`,
/// and `id` / `inputs` for graph wiring), with `"additionalProperties": false`
/// so [`crate::validate_params`] reports a typo inside an op.
pub fn op_schema(op_type: &str) -> serde_json::Value {
    let mut schema = op_param_schema(op_type);
    let props = schema["properties"]
        .as_object_mut()
        .expect("op schemas have properties");
    props.insert(
        "type".into(),
        serde_json::json!({ "const": op_type, "description": "Which op this table is" }),
    );
    props.insert(
        "id".into(),
        serde_json::json!({ "type": "string", "description": "Names the op so others can wire to its outputs (graph mode)" }),
    );
    props.insert(
        "inputs".into(),
        serde_json::json!({
            "type": "object",
            "description": "Graph wiring: local channel = \"source_id.channel\"",
            "additionalProperties": { "type": "string" }
        }),
    );
    schema["title"] = serde_json::json!(format!("{op_type} op"));
    schema["additionalProperties"] = serde_json::json!(false);
    schema
}

/// The schema of the `[[params.ops]]` array's items: one closed branch per
/// op type, chosen by the op's `type` (see [`crate::validate_params`]).
pub fn ops_items_schema() -> serde_json::Value {
    let branches: Vec<serde_json::Value> = ALL_OP_TYPES.iter().map(|t| op_schema(t)).collect();
    serde_json::json!({
        "type": "object",
        "required": ["type"],
        "discriminator": { "propertyName": "type" },
        "oneOf": branches
    })
}

/// The channels an op of `op_type` reads, writes and modifies, as the node
/// editor draws its ports; `None` for a type that isn't an op. Its
/// [`OpDescriptor`]'s (`flint gen --describe texture_v1` prints them).
pub fn op_ports(op_type: &str) -> Option<super::OpPortInfo> {
    op_descriptor(op_type).map(|d| d.port_info())
}

/// Return default parameter values for a given op type as a TOML map:
/// `type` and every key with a default (just `type` for an unknown op).
pub fn op_default_params(op_type: &str) -> toml::map::Map<String, toml::Value> {
    match op_descriptor(op_type) {
        Some(d) => d.default_params(),
        None => {
            let mut map = toml::map::Map::new();
            map.insert("type".to_string(), toml::Value::String(op_type.to_string()));
            map
        }
    }
}

// ─── Op Parsing ───────────────────────────────────────────────────────────

/// Parse a single op from a TOML table, through its [`OpDescriptor`]: an
/// unset key is its default, and a wrong type, an unknown choice or a number
/// out of its range is an error naming `ops[index].key`.
pub fn parse_op(value: &toml::Value, index: usize) -> Result<Box<dyn TextureOp>> {
    let table = value
        .as_table()
        .ok_or_else(|| ProcGenError::InvalidParameter {
            name: format!("ops[{index}]"),
            reason: "each op must be a TOML table".into(),
        })?;

    let op_type = table.get("type").and_then(|v| v.as_str()).ok_or_else(|| {
        ProcGenError::InvalidParameter {
            name: format!("ops[{index}].type"),
            reason: "each op must have a \"type\" string".into(),
        }
    })?;

    let descriptor = op_descriptor(op_type).ok_or_else(|| ProcGenError::InvalidParameter {
        name: format!("ops[{index}].type"),
        reason: format!(
            "unknown op type \"{op_type}\"; expected one of: {:?}",
            ALL_OP_TYPES
        ),
    })?;
    descriptor.parse(table, index)
}

// ─── Tests ────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::spec::ProcGenSpec;
    use crate::types::ChannelSemantics;

    fn pipeline_spec(ops_toml: &str) -> ProcGenSpec {
        ProcGenSpec::parse(&format!(
            r##"
generator = "texture_v1"
[meta]
name = "test_pipeline"
version = "1.0.0"
[seed]
mode = "fixed"
value = 42
[params]
width = 64
height = 64
pattern = "pipeline"
{ops_toml}
"##
        ))
        .unwrap()
    }

    #[test]
    fn pipeline_brick_grid_end_to_end() {
        let spec = pipeline_spec(
            r##"
[[params.ops]]
type = "brick_grid"
columns = 4
rows = 8
stagger = 0.5
gap_width = 0.04

[[params.ops]]
type = "cell_height"
variation = 0.3

[[params.ops]]
type = "mortar_groove"
depth = 0.15

[[params.ops]]
type = "cell_color"
base_color = "#8B5A3A"
variation = 0.12

[[params.ops]]
type = "mortar_color"
color = "#50483C"
threshold = 0.06

[[params.ops]]
type = "derive_normal"
strength = 1.5

[[params.ops]]
type = "cell_roughness"
base = 0.75
variation = 0.05
mortar = 0.55
"##,
        );

        let map_params = TextureMapParams::from_toml(&spec.params).unwrap();
        let mut rng = SeededRng::new(42);
        let maps = run_pipeline(&spec.params, &map_params, &mut rng).unwrap();

        assert_eq!(maps.len(), 3);
        assert_eq!(maps[0].channel_semantics, ChannelSemantics::Color);
        assert_eq!(maps[1].channel_semantics, ChannelSemantics::Normal);
        assert_eq!(maps[2].channel_semantics, ChannelSemantics::Roughness);
        for img in &maps {
            assert_eq!(img.width, 64);
            assert_eq!(img.height, 64);
            assert!(img.validate().is_ok());
        }
    }

    #[test]
    fn pipeline_determinism() {
        let spec = pipeline_spec(
            r##"
[[params.ops]]
type = "brick_grid"
columns = 4
rows = 8

[[params.ops]]
type = "cell_color"
base_color = "#8B5A3A"
variation = 0.1

[[params.ops]]
type = "derive_normal"
strength = 1.0
"##,
        );

        let map_params = TextureMapParams::from_toml(&spec.params).unwrap();

        let mut rng_a = SeededRng::new(42);
        let maps_a = run_pipeline(&spec.params, &map_params, &mut rng_a).unwrap();

        let mut rng_b = SeededRng::new(42);
        let maps_b = run_pipeline(&spec.params, &map_params, &mut rng_b).unwrap();

        for (a, b) in maps_a.iter().zip(maps_b.iter()) {
            assert_eq!(
                a.pixels, b.pixels,
                "same seed should produce identical pixels"
            );
        }
    }

    #[test]
    fn pipeline_empty_ops_error() {
        let spec_str = r##"
generator = "texture_v1"
[meta]
name = "test_empty"
version = "1.0.0"
[seed]
mode = "fixed"
value = 42
[params]
pattern = "pipeline"
ops = []
"##;
        let spec = ProcGenSpec::parse(spec_str).unwrap();
        let map_params = TextureMapParams::from_toml(&spec.params).unwrap();
        let mut rng = SeededRng::new(42);
        assert!(run_pipeline(&spec.params, &map_params, &mut rng).is_err());
    }

    #[test]
    fn pipeline_unknown_op_error() {
        let spec = pipeline_spec(
            r##"
[[params.ops]]
type = "magic_op"
"##,
        );
        let map_params = TextureMapParams::from_toml(&spec.params).unwrap();
        let mut rng = SeededRng::new(42);
        assert!(run_pipeline(&spec.params, &map_params, &mut rng).is_err());
    }

    #[test]
    fn pipeline_single_map_output() {
        let spec_str = r##"
generator = "texture_v1"
[meta]
name = "test_normal_only"
version = "1.0.0"
[seed]
mode = "fixed"
value = 42
[params]
width = 32
height = 32
pattern = "pipeline"
output_maps = ["normal"]

[[params.ops]]
type = "brick_grid"
columns = 4
rows = 4

[[params.ops]]
type = "cell_height"
variation = 0.3

[[params.ops]]
type = "derive_normal"
strength = 1.0
"##;
        let spec = ProcGenSpec::parse(spec_str).unwrap();
        let map_params = TextureMapParams::from_toml(&spec.params).unwrap();
        let mut rng = SeededRng::new(42);
        let maps = run_pipeline(&spec.params, &map_params, &mut rng).unwrap();

        assert_eq!(maps.len(), 1);
        assert_eq!(maps[0].channel_semantics, ChannelSemantics::Normal);
    }

    #[test]
    fn pipeline_noise_layer_on_height() {
        let spec = pipeline_spec(
            r##"
[[params.ops]]
type = "brick_grid"
columns = 4
rows = 8

[[params.ops]]
type = "cell_height"
variation = 0.3

[[params.ops]]
type = "noise_layer"
frequency = 40.0
octaves = 4

[[params.ops]]
type = "blend"
source = "noise"
target = "height"
mode = "add"
strength = 0.1

[[params.ops]]
type = "derive_normal"
strength = 1.5
"##,
        );

        let map_params = TextureMapParams::from_toml(&spec.params).unwrap();
        let mut rng = SeededRng::new(42);
        let maps = run_pipeline(&spec.params, &map_params, &mut rng).unwrap();

        assert_eq!(maps.len(), 3);
        for img in &maps {
            assert!(img.validate().is_ok());
        }
    }

    // ─── DAG pipeline tests ────────────────────────────────────────────

    #[test]
    fn dag_pipeline_linear_chain() {
        // Same as sequential but with explicit id/inputs
        let spec = pipeline_spec(
            r##"
[[params.ops]]
type = "brick_grid"
id = "grid"
columns = 4
rows = 8

[[params.ops]]
type = "cell_height"
id = "heights"
variation = 0.3
[params.ops.inputs]
cell_id = "grid.cell_id"

[[params.ops]]
type = "cell_color"
id = "color"
base_color = "#8B5A3A"
variation = 0.12
[params.ops.inputs]
cell_id = "grid.cell_id"

[[params.ops]]
type = "derive_normal"
id = "normals"
strength = 1.5
[params.ops.inputs]
height = "heights.height"
"##,
        );

        let map_params = TextureMapParams::from_toml(&spec.params).unwrap();
        let mut rng = SeededRng::new(42);
        let maps = run_pipeline(&spec.params, &map_params, &mut rng).unwrap();

        assert_eq!(maps.len(), 3);
        for img in &maps {
            assert!(img.validate().is_ok());
        }
    }

    #[test]
    fn dag_pipeline_diamond_topology() {
        // grid -> heights -> groove
        //      -> heights (used by groove via height input)
        // Tests that branching and merging works correctly
        let spec = pipeline_spec(
            r##"
[[params.ops]]
type = "brick_grid"
id = "grid"
columns = 4
rows = 8

[[params.ops]]
type = "cell_height"
id = "heights"
variation = 0.3
[params.ops.inputs]
cell_id = "grid.cell_id"

[[params.ops]]
type = "mortar_groove"
id = "groove"
depth = 0.15
[params.ops.inputs]
edge_dist = "grid.edge_dist"
height = "heights.height"

[[params.ops]]
type = "derive_normal"
id = "normals"
strength = 1.5
[params.ops.inputs]
height = "groove.height"
"##,
        );

        let map_params = TextureMapParams::from_toml(&spec.params).unwrap();
        let mut rng = SeededRng::new(42);
        let maps = run_pipeline(&spec.params, &map_params, &mut rng).unwrap();

        assert_eq!(maps.len(), 3);
        for img in &maps {
            assert!(img.validate().is_ok());
        }
    }

    #[test]
    fn dag_pipeline_with_snapshots() {
        let spec = pipeline_spec(
            r##"
[[params.ops]]
type = "brick_grid"
id = "grid"
columns = 4
rows = 8

[[params.ops]]
type = "cell_height"
id = "heights"
variation = 0.3
[params.ops.inputs]
cell_id = "grid.cell_id"
"##,
        );

        let map_params = TextureMapParams::from_toml(&spec.params).unwrap();
        let mut rng = SeededRng::new(42);
        let (maps, snapshots) =
            run_pipeline_with_snapshots(&spec.params, &map_params, &mut rng).unwrap();

        assert_eq!(snapshots.len(), 2);
        assert!(!maps.is_empty());
    }

    #[test]
    fn dag_pipeline_noise_blend_dynamic_target() {
        let spec = pipeline_spec(
            r##"
[[params.ops]]
type = "brick_grid"
id = "grid"
columns = 4
rows = 8

[[params.ops]]
type = "cell_height"
id = "heights"
variation = 0.3
[params.ops.inputs]
cell_id = "grid.cell_id"

[[params.ops]]
type = "noise_layer"
id = "noise"
frequency = 40.0
octaves = 4

[[params.ops]]
type = "blend"
id = "blend_noise"
source = "noise"
target = "height"
mode = "add"
strength = 0.1
[params.ops.inputs]
noise = "noise.noise"
height = "heights.height"

[[params.ops]]
type = "derive_normal"
id = "normals"
strength = 1.5
[params.ops.inputs]
height = "blend_noise.height"
"##,
        );

        let map_params = TextureMapParams::from_toml(&spec.params).unwrap();
        let mut rng = SeededRng::new(42);
        let maps = run_pipeline(&spec.params, &map_params, &mut rng).unwrap();

        assert_eq!(maps.len(), 3);
        for img in &maps {
            assert!(img.validate().is_ok());
        }
    }

    /// Every value of `field`'s channel, as bits, for exact comparison.
    fn channel_bits(field: &TextureField, name: &str) -> Vec<u32> {
        let (w, h) = (field.width, field.height);
        (0..h)
            .flat_map(|y| (0..w).map(move |x| (x, y)))
            .map(|(x, y)| field.get(name, x, y).to_bits())
            .collect()
    }

    /// A graph-mode op reads a `voronoi_texture`'s `cell_output` by node id
    /// (#1166): by its name and by its placeholder, each the cells the op
    /// writes, coloured as the sequential spec colours them.
    #[test]
    fn dag_pipeline_reads_a_voronoi_cell_output() {
        // The cell output is a 0-1 value a cell, so a colour ramp shows it.
        let ramp = r##"
[[params.ops.stops]]
position = 0.0
color = "#203060"
[[params.ops.stops]]
position = 1.0
color = "#F0C040"
"##;
        let sequential = pipeline_spec(&format!(
            r##"
output_maps = ["albedo"]

[[params.ops]]
type = "voronoi_texture"
output = "dist"
cell_output = "cells"
scale = 6.0

[[params.ops]]
type = "color_ramp"
input = "cells"
{ramp}"##
        ));
        let map_params = TextureMapParams::from_toml(&sequential.params).unwrap();
        let (want_maps, want_snaps) =
            run_pipeline_with_snapshots(&sequential.params, &map_params, &mut SeededRng::new(42))
                .unwrap();
        let cells = channel_bits(&want_snaps[0], "cells");
        let distinct: std::collections::HashSet<_> = cells.iter().collect();
        assert!(distinct.len() > 4, "the voronoi writes many cells");
        let colours: std::collections::HashSet<_> = want_maps[0].pixels.chunks(4).collect();
        assert!(colours.len() > 4, "the ramp colours each cell its own");

        for reference in ["cells.cells", "cells.<cell_output>"] {
            let graph = pipeline_spec(&format!(
                r##"
output_maps = ["albedo"]

[[params.ops]]
type = "voronoi_texture"
id = "cells"
output = "dist"
cell_output = "cells"
scale = 6.0

[[params.ops]]
type = "color_ramp"
id = "color"
input = "cell_value"
[params.ops.inputs]
cell_value = "{reference}"
{ramp}"##
            ));
            let (maps, snaps) =
                run_pipeline_with_snapshots(&graph.params, &map_params, &mut SeededRng::new(42))
                    .unwrap();
            assert_eq!(
                channel_bits(&snaps[0], "cells/cells"),
                cells,
                "{reference}: the node's scoped copy holds its cells"
            );
            assert_eq!(
                channel_bits(&snaps[1], "cell_value"),
                cells,
                "{reference}: color_ramp read the voronoi's cells"
            );
            assert_eq!(
                maps[0].pixels, want_maps[0].pixels,
                "{reference}: the graph colours each cell as the sequential spec does"
            );
        }
    }

    /// With `cell_output` unset, a graph-mode `voronoi_texture` copies out
    /// only its distance: the optional port is skipped (#1166).
    #[test]
    fn dag_pipeline_skips_an_unset_cell_output() {
        let spec = pipeline_spec(
            r##"
output_maps = ["roughness"]

[[params.ops]]
type = "voronoi_texture"
id = "cells"
output = "roughness"
"##,
        );
        let map_params = TextureMapParams::from_toml(&spec.params).unwrap();
        let (_, snaps) =
            run_pipeline_with_snapshots(&spec.params, &map_params, &mut SeededRng::new(42))
                .unwrap();
        let mut names: Vec<_> = snaps[0].channel_names().into_iter().collect();
        names.sort();
        assert_eq!(names, ["cells/roughness", "roughness"]);
    }

    #[test]
    fn dag_pipeline_backward_compat_no_ids() {
        // Spec without any id fields should use sequential mode
        let spec = pipeline_spec(
            r##"
[[params.ops]]
type = "brick_grid"
columns = 4
rows = 8

[[params.ops]]
type = "cell_height"
variation = 0.3

[[params.ops]]
type = "cell_color"
base_color = "#8B5A3A"
variation = 0.12
"##,
        );

        let map_params = TextureMapParams::from_toml(&spec.params).unwrap();

        let mut rng_a = SeededRng::new(42);
        let maps_a = run_pipeline(&spec.params, &map_params, &mut rng_a).unwrap();

        // Same result as before (regression test)
        assert_eq!(maps_a.len(), 3);
        for img in &maps_a {
            assert!(img.validate().is_ok());
        }
    }

    /// Per-op wall time for a spec file (#187). Run with
    /// `PROCGEN_PROFILE=specs/x.procgen.toml cargo test -p flint-procgen profile_spec_ops -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn profile_spec_ops() {
        let path = std::env::var("PROCGEN_PROFILE")
            .unwrap_or_else(|_| "../../specs/sci_fi_panel.procgen.toml".into());
        let spec = ProcGenSpec::parse(&std::fs::read_to_string(&path).unwrap()).unwrap();
        let map_params = TextureMapParams::from_toml(&spec.params).unwrap();
        let (_, ops) = parse_pipeline(&spec.params).unwrap();
        let mut rng = SeededRng::new(1);
        let mut field = TextureField::new(map_params.width, map_params.height);
        let total = std::time::Instant::now();
        for (i, op) in ops.iter().enumerate() {
            let mut op_rng = rng.fork(&format!("op_{i}"));
            let t = std::time::Instant::now();
            op.apply(&mut field, &mut op_rng);
            println!(
                "op {i:2} {:20} {:9.2} ms",
                spec.params["ops"][i]["type"].as_str().unwrap(),
                t.elapsed().as_secs_f64() * 1e3
            );
        }
        let t = std::time::Instant::now();
        let _ = extract_output_maps(&field, &map_params);
        println!(
            "extract maps             {:9.2} ms",
            t.elapsed().as_secs_f64() * 1e3
        );
        println!(
            "total                    {:9.2} ms",
            total.elapsed().as_secs_f64() * 1e3
        );
    }

    /// Run `[[ops]]` tables (TOML) on a fresh field, as the sequential
    /// pipeline does, and return the field.
    fn run_ops(ops_toml: &str, size: (u32, u32), seamless: bool, seed: u64) -> TextureField {
        let doc: toml::Value = toml::from_str(ops_toml).unwrap();
        let ops = doc["ops"].as_array().unwrap();
        let mut field = TextureField::new(size.0, size.1).with_seamless(seamless);
        let mut rng = SeededRng::new(seed);
        for (i, v) in ops.iter().enumerate() {
            let op = parse_op(v, i).unwrap();
            op.apply(&mut field, &mut rng.fork(&format!("op_{i}")));
        }
        field
    }

    /// Every op that samples noise or repeats a pattern, in each of its
    /// modes, with the channel it writes; `voronoi_grid` comes first where
    /// the op reads cells. Frequencies are fractional and the axes unequal on
    /// purpose: a tiling op must round them itself.
    fn tiling_cases() -> Vec<(String, String, &'static str)> {
        const CELLS: &str = "[[ops]]\ntype = \"voronoi_grid\"\ncell_count = 9\n";
        let mut cases = Vec::new();
        let mut case = |name: String, op: String, channel: &'static str| {
            cases.push((name, op, channel));
        };
        for noise in ["perlin", "simplex", "value", "voronoi"] {
            case(
                format!("noise_layer {noise}"),
                format!("[[ops]]\ntype = \"noise_layer\"\noutput = \"out\"\nnoise_type = \"{noise}\"\nfrequency = 3.3\noctaves = 4\nscale_x = 2.0\nscale_y = 0.7\n"),
                "out",
            );
            case(
                format!("noise_layer {noise} cell_offset"),
                format!("{CELLS}[[ops]]\ntype = \"noise_layer\"\noutput = \"out\"\nnoise_type = \"{noise}\"\nfrequency = 5.5\noctaves = 3\ncell_offset = true\n"),
                "out",
            );
            for kind in [
                "fbm",
                "multifractal",
                "ridged_multifractal",
                "hybrid_multifractal",
                "hetero_terrain",
            ] {
                case(
                    format!("musgrave {kind} {noise}"),
                    format!("[[ops]]\ntype = \"musgrave_texture\"\noutput = \"out\"\nnoise_type = \"{noise}\"\nmusgrave_type = \"{kind}\"\nfrequency = 2.6\noctaves = 4\nlacunarity = 2.1\nscale_x = 1.5\nscale_y = 1.0\n"),
                    "out",
                );
            }
            case(
                format!("edge_erode {noise}"),
                format!("{CELLS}[[ops]]\ntype = \"edge_erode\"\nnoise_type = \"{noise}\"\nnoise_frequency = 13.7\nstrength = 0.03\nedge_width = 0.6\nthreshold = 0.2\n"),
                "edge_dist",
            );
        }
        for feature in ["f1", "f2", "smooth_f1", "distance_to_edge"] {
            for metric in ["euclidean", "manhattan"] {
                case(
                    format!("voronoi_texture {feature} {metric}"),
                    format!("[[ops]]\ntype = \"voronoi_texture\"\noutput = \"out\"\nfeature = \"{feature}\"\nmetric = \"{metric}\"\nscale = 5.4\n"),
                    "out",
                );
            }
        }
        for wave in ["sine", "saw", "triangle"] {
            for direction in ["x", "y", "diagonal"] {
                case(
                    format!("wave_texture {wave} {direction}"),
                    format!("[[ops]]\ntype = \"wave_texture\"\noutput = \"out\"\nwave_type = \"{wave}\"\ndirection = \"{direction}\"\nscale = 3.4\ndistortion = 0.05\ndetail = 3\n"),
                    "out",
                );
            }
        }
        case(
            "checker_texture".into(),
            "[[ops]]\ntype = \"checker_texture\"\noutput = \"out\"\nscale_x = 5.0\nscale_y = 3.0\n"
                .into(),
            "out",
        );
        cases
    }

    /// The #25 seam check, per op: with `seamless` (the default) the step
    /// across each wrap is no larger than the roughest step inside the tile,
    /// at two sizes (one not square) and two seeds.
    #[test]
    fn every_tiling_op_tiles() {
        let mut failures = Vec::new();
        for (name, ops, channel) in tiling_cases() {
            // A saw wave starts a band at u = 0, so its cliff sits on the
            // seam (bent a little by the distortion) in nearly every row:
            // an edge on the wrap by design, as a grid's groove is (see
            // `grid_seamless_has_no_seam`).
            let limit = if name.contains("saw") { 1.6 } else { 1.25 };
            for size in [(128u32, 128u32), (160, 96)] {
                for seed in [3u64, 42] {
                    let s = run_ops(&ops, size, true, seed).seam_steps(channel);
                    if !(s.ratio() < limit) {
                        failures.push(format!("{name} {size:?} seed {seed}: {s:?}"));
                    }
                }
            }
        }
        assert!(failures.is_empty(), "seams:\n{}", failures.join("\n"));
    }

    /// The check sees the seams `seamless = false` leaves, so the test above
    /// is measuring something.
    #[test]
    fn unwrapped_noise_ops_show_a_seam() {
        for (name, ops, channel) in tiling_cases() {
            let unwrapped_seam = name.starts_with("noise_layer") && !name.contains("cell_offset")
                || name.starts_with("musgrave fbm")
                || name.starts_with("voronoi_texture f1");
            if unwrapped_seam {
                let s = run_ops(&ops, (128, 128), false, 42).seam_steps(channel);
                assert!(s.ratio() > 1.5, "{name}: expected a seam, got {s:?}");
            }
        }
    }

    /// `seamless` reaches the ops through the spec: a pipeline of noise
    /// tiles by default and keeps its old seam with `seamless = false`.
    #[test]
    fn pipeline_seamless_switch_reaches_the_ops() {
        let ops = r##"
[[params.ops]]
type = "noise_layer"
output = "height"
frequency = 3.3
octaves = 5
"##;
        let run = |seamless: Option<bool>| {
            let mut spec = pipeline_spec(ops);
            if let Some(b) = seamless {
                spec.params
                    .as_table_mut()
                    .unwrap()
                    .insert("seamless".into(), toml::Value::Boolean(b));
            }
            let map_params = TextureMapParams::from_toml(&spec.params).unwrap();
            run_pipeline_with_snapshots(&spec.params, &map_params, &mut SeededRng::new(42))
                .unwrap()
                .1
                .pop()
                .unwrap()
                .seam_steps("height")
                .ratio()
        };
        assert!(run(None) < 1.25, "default {}", run(None));
        assert!(run(Some(true)) < 1.25);
        assert!(run(Some(false)) > 1.5, "unwrapped {}", run(Some(false)));
    }

    /// Each op's table, every key set: a default, or for a key with none
    /// (an optional channel) its own name; a list gets one entry the same way.
    fn full_table(
        params: &[super::super::descriptor::ParamDesc],
    ) -> toml::map::Map<String, toml::Value> {
        use super::super::descriptor::ParamKind;
        let mut map = toml::map::Map::new();
        for p in params {
            let value = match p.kind {
                ParamKind::Text(None) => toml::Value::String(p.key.into()),
                ParamKind::Tables { items, .. } => {
                    toml::Value::Array(vec![toml::Value::Table(full_table(items))])
                }
                _ => continue,
            };
            map.insert(p.key.into(), value);
        }
        map
    }

    /// The schema and the parser can't drift (#720, replacing a test that
    /// read this file's source): every op builds from its descriptor's own
    /// defaults, and its build reads every key it declares (reading one it
    /// doesn't declare panics). The lint trusts the schema, so a key it
    /// lacked would be reported as a typo, and one it invented would hide one.
    #[test]
    fn every_op_reads_exactly_the_keys_it_declares() {
        use super::super::descriptor::{OpParams, ParamKind};
        let mut problems = Vec::new();
        for op in OPS {
            let mut table = op.default_params();
            table.extend(full_table(op.params));
            let params = OpParams::for_test(&table, op);
            if let Err(e) = (op.build)(&params) {
                problems.push(format!(
                    "{}: doesn't build from its defaults: {e}",
                    op.type_name
                ));
                continue;
            }
            let mut declared: Vec<&str> = Vec::new();
            for p in op.params {
                declared.push(p.key);
                if let ParamKind::Tables { items, .. } = p.kind {
                    declared.extend(items.iter().map(|i| i.key));
                }
            }
            declared.sort_unstable();
            declared.dedup();
            let read = params.keys_read();
            if read != declared {
                problems.push(format!(
                    "{}: build reads {read:?}, descriptor declares {declared:?}",
                    op.type_name
                ));
            }
        }
        assert!(problems.is_empty(), "\n{}", problems.join("\n"));
    }

    /// The registry's keys are unique, and each op answers with its own
    /// descriptor.
    #[test]
    fn the_registry_names_each_op_once() {
        let mut names: Vec<&str> = ALL_OP_TYPES.to_vec();
        assert_eq!(names.len(), OPS.len());
        names.sort_unstable();
        names.dedup();
        assert_eq!(names.len(), OPS.len(), "an op type is registered twice");
        for op in OPS {
            let mut table = op.default_params();
            table.extend(full_table(op.params));
            let built = parse_op(&toml::Value::Table(table), 0).unwrap();
            assert_eq!(built.descriptor().type_name, op.type_name);
            assert_eq!(built.port_info().label, op.label);
        }
    }

    /// Every op and every one of its keys says what it does (the schema's
    /// `description`, which the editor's tooltips, the README catalog and
    /// `texture_v1`'s schema carry).
    #[test]
    fn every_op_parameter_has_help() {
        use crate::generator::Generator;
        let schema = super::super::TextureGenerator.param_schema();
        let branches = schema["properties"]["ops"]["items"]["oneOf"]
            .as_array()
            .unwrap();
        assert_eq!(branches.len(), OPS.len());
        let mut missing = Vec::new();
        for branch in branches {
            let title = branch["title"].as_str().unwrap().to_string();
            if branch["description"].as_str().unwrap_or("").is_empty() {
                missing.push(title.clone());
            }
            for (key, prop) in branch["properties"].as_object().unwrap() {
                let mut props = vec![(key.clone(), prop)];
                if let Some(items) = prop["items"]["properties"].as_object() {
                    props.extend(items.iter().map(|(k, v)| (format!("{key}.{k}"), v)));
                }
                for (k, p) in props {
                    if p["description"].as_str().unwrap_or("").is_empty() {
                        missing.push(format!("{title}: {k}"));
                    }
                }
            }
        }
        assert!(missing.is_empty(), "no help: {missing:?}");
    }

    /// A default is the value parse_op applies, and the editor's new node
    /// gets that value: building from `op_default_params` and from a bare
    /// `type` makes the same op (#720: the schema used to say `#735A47` for
    /// cell_color while parse_op used another colour).
    #[test]
    fn a_default_table_builds_the_same_texture_as_a_bare_type() {
        let ops: Vec<String> = [
            "brick_grid",
            "cell_height",
            "noise_layer",
            "blend",
            "mortar_color",
            "cell_roughness",
            "derive_normal",
        ]
        .iter()
        .map(|t| {
            let table = toml::Value::Table(op_default_params(t));
            format!("[[ops]]\n{}", toml::to_string(&table).unwrap())
        })
        .collect();
        let bare: Vec<String> = [
            "brick_grid",
            "cell_height",
            "noise_layer",
            "blend",
            "mortar_color",
            "cell_roughness",
            "derive_normal",
        ]
        .iter()
        .map(|t| format!("[[ops]]\ntype = \"{t}\"\n"))
        .collect();
        let a = run_ops(&ops.concat(), (64, 64), true, 5);
        let b = run_ops(&bare.concat(), (64, 64), true, 5);
        for ch in ["height", "r", "g", "b", "roughness", "normal_x"] {
            assert_eq!(a.channel(ch), b.channel(ch), "{ch}");
        }
    }

    /// A wrong type, an unknown choice and a number out of range are errors
    /// naming the op's key, where they used to fall back to a default.
    #[test]
    fn parse_op_names_the_key_that_is_out_of_range_or_unknown() {
        let err = |op: &str| {
            let doc: toml::Value = toml::from_str(op).unwrap();
            match parse_op(&doc, 2) {
                Ok(_) => panic!("{op} parsed"),
                Err(e) => e.to_string(),
            }
        };
        assert_eq!(
            err("type = \"brick_grid\"\ncolumns = 0"),
            "invalid parameter `ops[2].columns`: 0 is below the minimum 1"
        );
        assert_eq!(
            err("type = \"brick_grid\"\ncolumns = \"x\""),
            "invalid parameter `ops[2].columns`: expected an integer"
        );
        assert_eq!(
            err("type = \"noise_layer\"\noctaves = 17"),
            "invalid parameter `ops[2].octaves`: 17 is above the maximum 16"
        );
        assert_eq!(
            err("type = \"blend\"\nmode = \"addd\""),
            "invalid parameter `ops[2].mode`: \"addd\" is not one of: add, add_raw, multiply, mix, screen, overlay, soft_light, linear_light, difference, darken, lighten, color_dodge, color_burn, subtract"
        );
        assert_eq!(
            err("type = \"blend\"\ntarget = 3"),
            "invalid parameter `ops[2].target`: expected a string (a channel name)"
        );
        assert_eq!(
            err("type = \"cell_color\"\nbase_color = \"#12345\""),
            "invalid parameter `ops[2].base_color`: expected 6 or 8 hex digits (got 5 chars: \"12345\")"
        );
        assert_eq!(
            err("type = \"color_ramp\""),
            "invalid parameter `ops[2].stops`: color_ramp requires a [[params.ops.stops]] array"
        );
        assert_eq!(
            err("type = \"color_ramp\"\nstops = []"),
            "invalid parameter `ops[2].stops`: color_ramp needs at least one entry"
        );
        assert_eq!(
            err("type = \"color_ramp\"\n[[stops]]\nposition = 1.5"),
            "invalid parameter `ops[2].stops[0].position`: 1.5 is above the maximum 1"
        );
        // A float bound holds at the value written, not its f32 rounding.
        let doc: toml::Value =
            toml::from_str("type = \"edge_erode\"\nstrength = 0.5\nnoise_frequency = 0.1").unwrap();
        assert!(parse_op(&doc, 0).is_ok());
    }

    /// The README's op catalog is [`op_reference_markdown`]: every op with
    /// its channels and a row per key.
    #[test]
    fn the_op_catalog_lists_every_op_and_key() {
        let md = super::super::descriptor::op_reference_markdown();
        for op in OPS {
            assert!(
                md.contains(&format!("### `{}` ({})", op.type_name, op.label)),
                "{}",
                op.type_name
            );
            for p in op.params {
                assert!(
                    md.contains(&format!("| `{}` |", p.key)),
                    "{}.{}",
                    op.type_name,
                    p.key
                );
            }
        }
        assert!(md.contains("Each `[[params.ops.stops]]` entry:"));
        assert!(md.contains("| `base_color` | hex colour | `\"#B3A090\"` |"));
    }

    #[test]
    fn lint_reports_a_typo_inside_an_op() {
        use crate::generator::Generator;
        let spec = pipeline_spec(
            r##"
[[params.ops]]
type = "brick_grid"
colums = 8
[[params.ops]]
type = "color_ramp"
id = "ramp"
[params.ops.inputs]
input = "grid.height"
[[params.ops.stops]]
position = 0.0
colour = "#000000"
"##,
        );
        let schema = super::super::TextureGenerator.param_schema();
        let shown: Vec<String> = crate::validate_params(&schema, &spec.params)
            .iter()
            .map(|i| i.to_string())
            .collect();
        assert_eq!(
            shown,
            [
                "`ops[0].colums` is not a parameter of the brick_grid op (did you mean `columns`?)",
                "`ops[1].stops[0].colour` is not a parameter of the color_ramp stop (did you mean `color`?)",
            ]
        );
    }
}
