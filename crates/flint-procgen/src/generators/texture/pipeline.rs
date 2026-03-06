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

use crate::generators::util::{parse_hex_color, toml_f32, toml_u32};
use crate::types::ImageData;
use crate::{ProcGenError, Result, SeededRng};

use super::field::TextureField;
use super::maps::TextureMapParams;
use super::node_graph;
use super::ops::*;

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
    let mut field = TextureField::new(map_params.width, map_params.height);
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
    let mut field = TextureField::new(map_params.width, map_params.height);
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

    let ops_array = table
        .get("ops")
        .and_then(|v| v.as_array())
        .ok_or_else(|| ProcGenError::InvalidParameter {
            name: "ops".into(),
            reason: "pipeline pattern requires a [[params.ops]] array".into(),
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

    let mut field = TextureField::new(map_params.width, map_params.height);

    for &exec_idx in &execution_order {
        execute_dag_node(exec_idx, &dag_nodes, ops[exec_idx].as_ref(), &ops_array[exec_idx], ops_array, &mut field, rng);
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

    let mut field = TextureField::new(map_params.width, map_params.height);
    let mut snapshots_by_idx: Vec<Option<TextureField>> = vec![None; ops.len()];

    for &exec_idx in &execution_order {
        execute_dag_node(exec_idx, &dag_nodes, ops[exec_idx].as_ref(), &ops_array[exec_idx], ops_array, &mut field, rng);
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

/// All known op type identifiers.
pub const ALL_OP_TYPES: &[&str] = &[
    "brick_grid",
    "voronoi_grid",
    "domain_warp",
    "cell_height",
    "noise_layer",
    "blend",
    "mortar_groove",
    "cell_color",
    "mortar_color",
    "derive_normal",
    "cell_roughness",
    // Phase 1
    "math",
    "map_range",
    "color_ramp",
    "checker_texture",
    // Phase 2
    "gradient_texture",
    "wave_texture",
    "white_noise",
    "voronoi_texture",
    "musgrave_texture",
    // Phase 3
    "invert",
    "brightness_contrast",
    "hsv_adjust",
    "gamma",
    "clamp",
    // Phase 4
    "blur",
    "sharpen",
    "edge_detect",
];

/// Return a JSON Schema describing the parameters for a given op type.
pub fn op_param_schema(op_type: &str) -> serde_json::Value {
    match op_type {
        "brick_grid" => serde_json::json!({
            "type": "object",
            "properties": {
                "columns": { "type": "integer", "minimum": 1, "default": 8 },
                "rows": { "type": "integer", "minimum": 1, "default": 16 },
                "stagger": { "type": "number", "minimum": 0.0, "maximum": 1.0, "default": 0.5 },
                "gap_width": { "type": "number", "minimum": 0.0, "maximum": 1.0, "default": 0.04 },
                "width_variation": { "type": "number", "minimum": 0.0, "maximum": 1.0, "default": 0.0 },
                "warp_x": { "type": "string" },
                "warp_y": { "type": "string" },
                "warp_strength": { "type": "number", "minimum": 0.0, "default": 0.0 }
            }
        }),
        "voronoi_grid" => serde_json::json!({
            "type": "object",
            "properties": {
                "cell_count": { "type": "integer", "minimum": 1, "default": 20 },
                "regularity": { "type": "number", "minimum": 0.0, "maximum": 1.0, "default": 0.0 },
                "mortar_width": { "type": "number", "minimum": 0.0, "maximum": 1.0, "default": 0.03 },
                "warp_x": { "type": "string" },
                "warp_y": { "type": "string" },
                "warp_strength": { "type": "number", "minimum": 0.0, "default": 0.0 }
            }
        }),
        "domain_warp" => serde_json::json!({
            "type": "object",
            "properties": {
                "input": { "type": "string", "default": "input" },
                "output": { "type": "string", "default": "output" },
                "warp_x": { "type": "string", "default": "warp_x" },
                "warp_y": { "type": "string", "default": "warp_y" },
                "strength": { "type": "number", "minimum": 0.0, "default": 0.1 }
            }
        }),
        "cell_height" => serde_json::json!({
            "type": "object",
            "properties": {
                "variation": { "type": "number", "minimum": 0.0, "maximum": 1.0, "default": 0.3 }
            }
        }),
        "noise_layer" => serde_json::json!({
            "type": "object",
            "properties": {
                "output": { "type": "string", "default": "noise" },
                "noise_type": { "type": "string", "enum": ["perlin", "simplex", "value", "voronoi"], "default": "perlin" },
                "frequency": { "type": "number", "minimum": 0.1, "default": 10.0 },
                "octaves": { "type": "integer", "minimum": 1, "maximum": 16, "default": 4 },
                "scale_x": { "type": "number", "minimum": 0.01, "default": 1.0 },
                "scale_y": { "type": "number", "minimum": 0.01, "default": 1.0 }
            }
        }),
        "blend" => serde_json::json!({
            "type": "object",
            "properties": {
                "source": { "type": "string", "default": "noise" },
                "target": { "type": "string", "default": "height" },
                "mode": { "type": "string", "enum": [
                    "add", "multiply", "mix", "screen", "overlay", "soft_light",
                    "linear_light", "difference", "darken", "lighten",
                    "color_dodge", "color_burn", "subtract"
                ], "default": "add" },
                "strength": { "type": "number", "minimum": 0.0, "default": 0.1 }
            }
        }),
        "mortar_groove" => serde_json::json!({
            "type": "object",
            "properties": {
                "depth": { "type": "number", "minimum": 0.0, "maximum": 1.0, "default": 0.15 },
                "width": { "type": "number", "minimum": 0.0, "maximum": 1.0, "default": 0.04 }
            }
        }),
        "cell_color" => serde_json::json!({
            "type": "object",
            "properties": {
                "base_color": { "type": "string", "pattern": "^#?[0-9a-fA-F]{6,8}$", "default": "#735A47" },
                "variation": { "type": "number", "minimum": 0.0, "maximum": 1.0, "default": 0.12 }
            }
        }),
        "mortar_color" => serde_json::json!({
            "type": "object",
            "properties": {
                "color": { "type": "string", "pattern": "^#?[0-9a-fA-F]{6,8}$", "default": "#40382E" },
                "threshold": { "type": "number", "minimum": 0.0, "maximum": 1.0, "default": 0.06 }
            }
        }),
        "derive_normal" => serde_json::json!({
            "type": "object",
            "properties": {
                "strength": { "type": "number", "minimum": 0.0, "default": 1.0 }
            }
        }),
        "cell_roughness" => serde_json::json!({
            "type": "object",
            "properties": {
                "base": { "type": "number", "minimum": 0.0, "maximum": 1.0, "default": 0.75 },
                "variation": { "type": "number", "minimum": 0.0, "maximum": 1.0, "default": 0.05 },
                "mortar": { "type": "number", "minimum": 0.0, "maximum": 1.0, "default": 0.55 },
                "mortar_threshold": { "type": "number", "minimum": 0.0, "maximum": 1.0, "default": 0.06 }
            }
        }),
        // Phase 1
        "math" => serde_json::json!({
            "type": "object",
            "properties": {
                "operation": { "type": "string", "enum": [
                    "add", "subtract", "multiply", "divide", "power", "sqrt", "abs",
                    "min", "max", "fract", "modulo", "snap", "sin", "cos",
                    "less_than", "greater_than", "sign", "floor", "ceil", "round",
                    "pingpong", "wrap", "negate"
                ], "default": "add" },
                "input_a": { "type": "string", "default": "input" },
                "input_b": { "type": "string" },
                "value_b": { "type": "number", "default": 0.0 },
                "output": { "type": "string", "default": "output" },
                "clamp": { "type": "boolean", "default": false }
            }
        }),
        "map_range" => serde_json::json!({
            "type": "object",
            "properties": {
                "input": { "type": "string", "default": "input" },
                "output": { "type": "string", "default": "output" },
                "from_min": { "type": "number", "default": 0.0 },
                "from_max": { "type": "number", "default": 1.0 },
                "to_min": { "type": "number", "default": 0.0 },
                "to_max": { "type": "number", "default": 1.0 },
                "interpolation": { "type": "string", "enum": ["linear", "smoothstep", "smootherstep"], "default": "linear" },
                "clamp": { "type": "boolean", "default": true }
            }
        }),
        "color_ramp" => serde_json::json!({
            "type": "object",
            "properties": {
                "input": { "type": "string", "default": "input" },
                "interpolation": { "type": "string", "enum": ["linear", "constant", "ease"], "default": "linear" }
            }
        }),
        "checker_texture" => serde_json::json!({
            "type": "object",
            "properties": {
                "output": { "type": "string", "default": "output" },
                "scale_x": { "type": "number", "minimum": 0.1, "default": 8.0 },
                "scale_y": { "type": "number", "minimum": 0.1, "default": 8.0 }
            }
        }),
        // Phase 2
        "gradient_texture" => serde_json::json!({
            "type": "object",
            "properties": {
                "output": { "type": "string", "default": "output" },
                "gradient_type": { "type": "string", "enum": [
                    "linear", "quadratic", "easing", "diagonal",
                    "spherical", "quadratic_sphere", "radial"
                ], "default": "linear" }
            }
        }),
        "wave_texture" => serde_json::json!({
            "type": "object",
            "properties": {
                "output": { "type": "string", "default": "output" },
                "wave_type": { "type": "string", "enum": ["sine", "saw", "triangle"], "default": "sine" },
                "direction": { "type": "string", "enum": ["x", "y", "diagonal"], "default": "x" },
                "scale": { "type": "number", "minimum": 0.1, "default": 4.0 },
                "distortion": { "type": "number", "minimum": 0.0, "default": 0.0 },
                "detail": { "type": "integer", "minimum": 0, "maximum": 16, "default": 0 }
            }
        }),
        "white_noise" => serde_json::json!({
            "type": "object",
            "properties": {
                "output": { "type": "string", "default": "output" }
            }
        }),
        "voronoi_texture" => serde_json::json!({
            "type": "object",
            "properties": {
                "output": { "type": "string", "default": "output" },
                "cell_output": { "type": "string" },
                "scale": { "type": "number", "minimum": 0.1, "default": 4.0 },
                "randomness": { "type": "number", "minimum": 0.0, "maximum": 1.0, "default": 1.0 },
                "feature": { "type": "string", "enum": ["f1", "f2", "smooth_f1", "distance_to_edge"], "default": "f1" },
                "metric": { "type": "string", "enum": ["euclidean", "manhattan", "chebyshev", "minkowski"], "default": "euclidean" },
                "exponent": { "type": "number", "minimum": 0.1, "default": 2.0 }
            }
        }),
        "musgrave_texture" => serde_json::json!({
            "type": "object",
            "properties": {
                "output": { "type": "string", "default": "output" },
                "noise_type": { "type": "string", "enum": ["perlin", "simplex", "value", "voronoi"], "default": "perlin" },
                "musgrave_type": { "type": "string", "enum": [
                    "fbm", "multifractal", "ridged_multifractal",
                    "hybrid_multifractal", "hetero_terrain"
                ], "default": "fbm" },
                "frequency": { "type": "number", "minimum": 0.1, "default": 4.0 },
                "octaves": { "type": "integer", "minimum": 1, "maximum": 16, "default": 4 },
                "lacunarity": { "type": "number", "minimum": 0.1, "default": 2.0 },
                "dimension": { "type": "number", "default": 1.0 },
                "offset": { "type": "number", "default": 1.0 },
                "gain": { "type": "number", "default": 2.0 },
                "scale_x": { "type": "number", "minimum": 0.01, "default": 1.0 },
                "scale_y": { "type": "number", "minimum": 0.01, "default": 1.0 }
            }
        }),
        // Phase 3
        "invert" => serde_json::json!({
            "type": "object",
            "properties": {
                "input": { "type": "string", "default": "input" },
                "output": { "type": "string", "default": "output" },
                "factor": { "type": "number", "minimum": 0.0, "maximum": 1.0, "default": 1.0 }
            }
        }),
        "brightness_contrast" => serde_json::json!({
            "type": "object",
            "properties": {
                "input": { "type": "string", "default": "input" },
                "output": { "type": "string", "default": "output" },
                "brightness": { "type": "number", "minimum": -1.0, "maximum": 1.0, "default": 0.0 },
                "contrast": { "type": "number", "minimum": -1.0, "maximum": 10.0, "default": 0.0 }
            }
        }),
        "hsv_adjust" => serde_json::json!({
            "type": "object",
            "properties": {
                "hue_offset": { "type": "number", "minimum": -1.0, "maximum": 1.0, "default": 0.0 },
                "saturation_factor": { "type": "number", "minimum": 0.0, "maximum": 3.0, "default": 1.0 },
                "value_factor": { "type": "number", "minimum": 0.0, "maximum": 3.0, "default": 1.0 }
            }
        }),
        "gamma" => serde_json::json!({
            "type": "object",
            "properties": {
                "input": { "type": "string", "default": "input" },
                "output": { "type": "string", "default": "output" },
                "gamma": { "type": "number", "minimum": 0.01, "maximum": 10.0, "default": 1.0 }
            }
        }),
        "clamp" => serde_json::json!({
            "type": "object",
            "properties": {
                "input": { "type": "string", "default": "input" },
                "output": { "type": "string", "default": "output" },
                "min": { "type": "number", "default": 0.0 },
                "max": { "type": "number", "default": 1.0 }
            }
        }),
        // Phase 4
        "blur" => serde_json::json!({
            "type": "object",
            "properties": {
                "input": { "type": "string", "default": "input" },
                "output": { "type": "string", "default": "output" },
                "radius": { "type": "integer", "minimum": 0, "maximum": 64, "default": 2 },
                "sigma": { "type": "number", "minimum": 0.1, "maximum": 32.0, "default": 1.0 }
            }
        }),
        "sharpen" => serde_json::json!({
            "type": "object",
            "properties": {
                "input": { "type": "string", "default": "input" },
                "output": { "type": "string", "default": "output" },
                "strength": { "type": "number", "minimum": 0.0, "maximum": 10.0, "default": 1.0 },
                "radius": { "type": "integer", "minimum": 1, "maximum": 64, "default": 2 }
            }
        }),
        "edge_detect" => serde_json::json!({
            "type": "object",
            "properties": {
                "input": { "type": "string", "default": "input" },
                "output": { "type": "string", "default": "output" }
            }
        }),
        _ => serde_json::json!({ "type": "object", "properties": {} }),
    }
}

/// Return default parameter values for a given op type as a TOML map.
pub fn op_default_params(op_type: &str) -> toml::map::Map<String, toml::Value> {
    let schema = op_param_schema(op_type);
    let mut map = toml::map::Map::new();
    map.insert("type".to_string(), toml::Value::String(op_type.to_string()));

    if let Some(props) = schema.get("properties").and_then(|p| p.as_object()) {
        for (key, prop) in props {
            if let Some(default) = prop.get("default") {
                let toml_val = json_to_toml(default);
                map.insert(key.clone(), toml_val);
            }
        }
    }

    map
}

/// Convert a serde_json::Value to a toml::Value (for default param extraction).
fn json_to_toml(v: &serde_json::Value) -> toml::Value {
    match v {
        serde_json::Value::Bool(b) => toml::Value::Boolean(*b),
        serde_json::Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                toml::Value::Integer(i)
            } else {
                toml::Value::Float(n.as_f64().unwrap_or(0.0))
            }
        }
        serde_json::Value::String(s) => toml::Value::String(s.clone()),
        _ => toml::Value::String(String::new()),
    }
}

// ─── Op Parsing ───────────────────────────────────────────────────────────

/// Parse a single op from a TOML table.
pub fn parse_op(value: &toml::Value, index: usize) -> Result<Box<dyn TextureOp>> {
    let table = value.as_table().ok_or_else(|| ProcGenError::InvalidParameter {
        name: format!("ops[{index}]"),
        reason: "each op must be a TOML table".into(),
    })?;

    let op_type = table
        .get("type")
        .and_then(|v| v.as_str())
        .ok_or_else(|| ProcGenError::InvalidParameter {
            name: format!("ops[{index}].type"),
            reason: "each op must have a \"type\" string".into(),
        })?;

    match op_type {
        "brick_grid" => {
            let columns = opt_u32(table, "columns")?.unwrap_or(8);
            let rows = opt_u32(table, "rows")?.unwrap_or(16);
            let stagger = opt_f32(table, "stagger")?.unwrap_or(0.5);
            let gap_width = opt_f32(table, "gap_width")?.unwrap_or(0.04);
            let width_variation = opt_f32(table, "width_variation")?.unwrap_or(0.0);
            let warp_x = table.get("warp_x").and_then(|v| v.as_str()).map(|s| s.to_string());
            let warp_y = table.get("warp_y").and_then(|v| v.as_str()).map(|s| s.to_string());
            let warp_strength = opt_f32(table, "warp_strength")?.unwrap_or(0.0);
            Ok(Box::new(BrickGridOp {
                columns,
                rows,
                stagger,
                gap_width,
                width_variation,
                warp_x,
                warp_y,
                warp_strength,
            }))
        }
        "voronoi_grid" => {
            let cell_count = opt_u32(table, "cell_count")?.unwrap_or(20);
            let regularity = opt_f32(table, "regularity")?.unwrap_or(0.0);
            let mortar_width = opt_f32(table, "mortar_width")?.unwrap_or(0.03);
            let warp_x = table.get("warp_x").and_then(|v| v.as_str()).map(|s| s.to_string());
            let warp_y = table.get("warp_y").and_then(|v| v.as_str()).map(|s| s.to_string());
            let warp_strength = opt_f32(table, "warp_strength")?.unwrap_or(0.0);
            Ok(Box::new(VoronoiGridOp {
                cell_count,
                regularity,
                mortar_width,
                warp_x,
                warp_y,
                warp_strength,
            }))
        }
        "domain_warp" => {
            let input = table
                .get("input")
                .and_then(|v| v.as_str())
                .unwrap_or("input")
                .to_string();
            let output = table
                .get("output")
                .and_then(|v| v.as_str())
                .unwrap_or("output")
                .to_string();
            let warp_x = table
                .get("warp_x")
                .and_then(|v| v.as_str())
                .unwrap_or("warp_x")
                .to_string();
            let warp_y = table
                .get("warp_y")
                .and_then(|v| v.as_str())
                .unwrap_or("warp_y")
                .to_string();
            let strength = opt_f32(table, "strength")?.unwrap_or(0.1);
            Ok(Box::new(DomainWarpOp {
                input,
                output,
                warp_x,
                warp_y,
                strength,
            }))
        }
        "cell_height" => {
            let variation = opt_f32(table, "variation")?.unwrap_or(0.3);
            Ok(Box::new(CellHeightOp { variation }))
        }
        "noise_layer" => {
            let output = table
                .get("output")
                .and_then(|v| v.as_str())
                .unwrap_or("noise")
                .to_string();
            let noise_type = table
                .get("noise_type")
                .and_then(|v| v.as_str())
                .map(NoiseType::from_str)
                .unwrap_or(NoiseType::Perlin);
            let frequency = opt_f32(table, "frequency")?.unwrap_or(10.0);
            let octaves = opt_u32(table, "octaves")?.unwrap_or(4);
            let scale_x = opt_f32(table, "scale_x")?.unwrap_or(1.0);
            let scale_y = opt_f32(table, "scale_y")?.unwrap_or(1.0);
            Ok(Box::new(NoiseLayerOp {
                output,
                frequency,
                octaves,
                noise_type,
                scale_x,
                scale_y,
            }))
        }
        "blend" => {
            let source = table
                .get("source")
                .and_then(|v| v.as_str())
                .unwrap_or("noise")
                .to_string();
            let target = table
                .get("target")
                .and_then(|v| v.as_str())
                .unwrap_or("height")
                .to_string();
            let mode = table
                .get("mode")
                .and_then(|v| v.as_str())
                .map(BlendMode::from_str)
                .unwrap_or(BlendMode::Add);
            let strength = opt_f32(table, "strength")?.unwrap_or(0.1);
            Ok(Box::new(BlendOp {
                source,
                target,
                mode,
                strength,
            }))
        }
        "mortar_groove" => {
            let depth = opt_f32(table, "depth")?.unwrap_or(0.15);
            let width = opt_f32(table, "width")?.unwrap_or(0.04);
            Ok(Box::new(MortarGrooveOp { depth, width }))
        }
        "cell_color" => {
            let base_color = if let Some(v) = table.get("base_color") {
                let hex = v.as_str().ok_or_else(|| ProcGenError::InvalidParameter {
                    name: "base_color".into(),
                    reason: "expected a hex color string".into(),
                })?;
                parse_hex_color(hex)?
            } else {
                [0.45, 0.35, 0.28, 1.0]
            };
            let variation = opt_f32(table, "variation")?.unwrap_or(0.12);
            Ok(Box::new(CellColorOp {
                base_color,
                variation,
            }))
        }
        "mortar_color" => {
            let color = if let Some(v) = table.get("color") {
                let hex = v.as_str().ok_or_else(|| ProcGenError::InvalidParameter {
                    name: "color".into(),
                    reason: "expected a hex color string".into(),
                })?;
                parse_hex_color(hex)?
            } else {
                [0.25, 0.22, 0.18, 1.0]
            };
            let threshold = opt_f32(table, "threshold")?.unwrap_or(0.06);
            Ok(Box::new(MortarColorOp { color, threshold }))
        }
        "derive_normal" => {
            let strength = opt_f32(table, "strength")?.unwrap_or(1.0);
            Ok(Box::new(DeriveNormalOp { strength }))
        }
        "cell_roughness" => {
            let base = opt_f32(table, "base")?.unwrap_or(0.75);
            let variation = opt_f32(table, "variation")?.unwrap_or(0.05);
            let mortar = opt_f32(table, "mortar")?.unwrap_or(0.55);
            let mortar_threshold = opt_f32(table, "mortar_threshold")?.unwrap_or(0.06);
            Ok(Box::new(CellRoughnessOp {
                base,
                variation,
                mortar,
                mortar_threshold,
            }))
        }
        // ── Phase 1 ──────────────────────────────────────────────────
        "math" => {
            let operation = table
                .get("operation")
                .and_then(|v| v.as_str())
                .map(MathOperation::from_str)
                .unwrap_or(MathOperation::Add);
            let input_a = table
                .get("input_a")
                .and_then(|v| v.as_str())
                .unwrap_or("input")
                .to_string();
            let input_b = table
                .get("input_b")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string());
            let value_b = opt_f32(table, "value_b")?.unwrap_or(0.0);
            let output = table
                .get("output")
                .and_then(|v| v.as_str())
                .unwrap_or("output")
                .to_string();
            let clamp_output = table
                .get("clamp")
                .and_then(|v| v.as_bool())
                .unwrap_or(false);
            Ok(Box::new(MathOp {
                operation,
                input_a,
                input_b,
                value_b,
                output,
                clamp_output,
            }))
        }
        "map_range" => {
            let input = table
                .get("input")
                .and_then(|v| v.as_str())
                .unwrap_or("input")
                .to_string();
            let output = table
                .get("output")
                .and_then(|v| v.as_str())
                .unwrap_or("output")
                .to_string();
            let from_min = opt_f32(table, "from_min")?.unwrap_or(0.0);
            let from_max = opt_f32(table, "from_max")?.unwrap_or(1.0);
            let to_min = opt_f32(table, "to_min")?.unwrap_or(0.0);
            let to_max = opt_f32(table, "to_max")?.unwrap_or(1.0);
            let interpolation = table
                .get("interpolation")
                .and_then(|v| v.as_str())
                .map(MapRangeInterp::from_str)
                .unwrap_or(MapRangeInterp::Linear);
            let clamp_output = table
                .get("clamp")
                .and_then(|v| v.as_bool())
                .unwrap_or(true);
            Ok(Box::new(MapRangeOp {
                input,
                output,
                from_min,
                from_max,
                to_min,
                to_max,
                interpolation,
                clamp_output,
            }))
        }
        "color_ramp" => {
            let input = table
                .get("input")
                .and_then(|v| v.as_str())
                .unwrap_or("input")
                .to_string();
            let interpolation = table
                .get("interpolation")
                .and_then(|v| v.as_str())
                .map(ColorRampInterp::from_str)
                .unwrap_or(ColorRampInterp::Linear);
            let stops_arr = table
                .get("stops")
                .and_then(|v| v.as_array())
                .ok_or_else(|| ProcGenError::InvalidParameter {
                    name: format!("ops[{index}].stops"),
                    reason: "color_ramp requires a [[params.ops.stops]] array".into(),
                })?;
            let mut stops = Vec::with_capacity(stops_arr.len());
            for (si, stop_val) in stops_arr.iter().enumerate() {
                let stop_table = stop_val.as_table().ok_or_else(|| ProcGenError::InvalidParameter {
                    name: format!("ops[{index}].stops[{si}]"),
                    reason: "each stop must be a TOML table".into(),
                })?;
                let position = opt_f32(stop_table, "position")?.unwrap_or(0.0);
                let color = if let Some(v) = stop_table.get("color") {
                    let hex = v.as_str().ok_or_else(|| ProcGenError::InvalidParameter {
                        name: format!("ops[{index}].stops[{si}].color"),
                        reason: "expected a hex color string".into(),
                    })?;
                    let c = parse_hex_color(hex)?;
                    [c[0], c[1], c[2]]
                } else {
                    [0.0, 0.0, 0.0]
                };
                stops.push(ColorStop { position, color });
            }
            // Sort stops by position
            stops.sort_by(|a, b| a.position.partial_cmp(&b.position).unwrap_or(std::cmp::Ordering::Equal));
            Ok(Box::new(ColorRampOp {
                input,
                interpolation,
                stops,
            }))
        }
        "checker_texture" => {
            let output = table
                .get("output")
                .and_then(|v| v.as_str())
                .unwrap_or("output")
                .to_string();
            let scale_x = opt_f32(table, "scale_x")?.unwrap_or(8.0);
            let scale_y = opt_f32(table, "scale_y")?.unwrap_or(8.0);
            Ok(Box::new(CheckerTextureOp {
                output,
                scale_x,
                scale_y,
            }))
        }
        // ── Phase 2 ──────────────────────────────────────────────────
        "gradient_texture" => {
            let output = table
                .get("output")
                .and_then(|v| v.as_str())
                .unwrap_or("output")
                .to_string();
            let gradient_type = table
                .get("gradient_type")
                .and_then(|v| v.as_str())
                .map(GradientType::from_str)
                .unwrap_or(GradientType::Linear);
            Ok(Box::new(GradientTextureOp {
                output,
                gradient_type,
            }))
        }
        "wave_texture" => {
            let output = table
                .get("output")
                .and_then(|v| v.as_str())
                .unwrap_or("output")
                .to_string();
            let wave_type = table
                .get("wave_type")
                .and_then(|v| v.as_str())
                .map(WaveType::from_str)
                .unwrap_or(WaveType::Sine);
            let direction = table
                .get("direction")
                .and_then(|v| v.as_str())
                .map(WaveDirection::from_str)
                .unwrap_or(WaveDirection::X);
            let scale = opt_f32(table, "scale")?.unwrap_or(4.0);
            let distortion = opt_f32(table, "distortion")?.unwrap_or(0.0);
            let detail = opt_u32(table, "detail")?.unwrap_or(0);
            Ok(Box::new(WaveTextureOp {
                output,
                wave_type,
                direction,
                scale,
                distortion,
                detail,
            }))
        }
        "white_noise" => {
            let output = table
                .get("output")
                .and_then(|v| v.as_str())
                .unwrap_or("output")
                .to_string();
            Ok(Box::new(WhiteNoiseOp { output }))
        }
        "voronoi_texture" => {
            use crate::algorithms::noise::{VoronoiFeature, VoronoiMetric};
            let output = table
                .get("output")
                .and_then(|v| v.as_str())
                .unwrap_or("output")
                .to_string();
            let cell_output = table
                .get("cell_output")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string());
            let scale = opt_f32(table, "scale")?.unwrap_or(4.0);
            let randomness = opt_f32(table, "randomness")?.unwrap_or(1.0);
            let feature = match table.get("feature").and_then(|v| v.as_str()) {
                Some("f2") => VoronoiFeature::F2,
                Some("smooth_f1") => VoronoiFeature::SmoothF1,
                Some("distance_to_edge") => VoronoiFeature::DistanceToEdge,
                _ => VoronoiFeature::F1,
            };
            let exponent = opt_f32(table, "exponent")?.unwrap_or(2.0);
            let metric = match table.get("metric").and_then(|v| v.as_str()) {
                Some("manhattan") => VoronoiMetric::Manhattan,
                Some("chebyshev") => VoronoiMetric::Chebyshev,
                Some("minkowski") => VoronoiMetric::Minkowski(exponent as f64),
                _ => VoronoiMetric::Euclidean,
            };
            Ok(Box::new(VoronoiTextureOp {
                output,
                cell_output,
                scale,
                randomness,
                feature,
                metric,
            }))
        }
        "musgrave_texture" => {
            use crate::algorithms::noise::MusgraveType;
            let output = table
                .get("output")
                .and_then(|v| v.as_str())
                .unwrap_or("output")
                .to_string();
            let noise_type = table
                .get("noise_type")
                .and_then(|v| v.as_str())
                .map(NoiseType::from_str)
                .unwrap_or(NoiseType::Perlin);
            let musgrave_type = match table.get("musgrave_type").and_then(|v| v.as_str()) {
                Some("multifractal") => MusgraveType::Multifractal,
                Some("ridged_multifractal") => MusgraveType::RidgedMultifractal,
                Some("hybrid_multifractal") => MusgraveType::HybridMultifractal,
                Some("hetero_terrain") => MusgraveType::HeteroTerrain,
                _ => MusgraveType::Fbm,
            };
            let frequency = opt_f32(table, "frequency")?.unwrap_or(4.0);
            let octaves = opt_u32(table, "octaves")?.unwrap_or(4);
            let lacunarity = opt_f32(table, "lacunarity")?.unwrap_or(2.0);
            let dimension = opt_f32(table, "dimension")?.unwrap_or(1.0);
            let offset = opt_f32(table, "offset")?.unwrap_or(1.0);
            let gain = opt_f32(table, "gain")?.unwrap_or(2.0);
            let scale_x = opt_f32(table, "scale_x")?.unwrap_or(1.0);
            let scale_y = opt_f32(table, "scale_y")?.unwrap_or(1.0);
            Ok(Box::new(MusgraveTextureOp {
                output,
                noise_type,
                musgrave_type,
                frequency,
                octaves,
                lacunarity,
                dimension,
                offset,
                gain,
                scale_x,
                scale_y,
            }))
        }
        // ── Phase 3 ──────────────────────────────────────────────────
        "invert" => {
            let input = table
                .get("input")
                .and_then(|v| v.as_str())
                .unwrap_or("input")
                .to_string();
            let output = table
                .get("output")
                .and_then(|v| v.as_str())
                .unwrap_or("output")
                .to_string();
            let factor = opt_f32(table, "factor")?.unwrap_or(1.0);
            Ok(Box::new(InvertOp {
                input,
                output,
                factor,
            }))
        }
        "brightness_contrast" => {
            let input = table
                .get("input")
                .and_then(|v| v.as_str())
                .unwrap_or("input")
                .to_string();
            let output = table
                .get("output")
                .and_then(|v| v.as_str())
                .unwrap_or("output")
                .to_string();
            let brightness = opt_f32(table, "brightness")?.unwrap_or(0.0);
            let contrast = opt_f32(table, "contrast")?.unwrap_or(0.0);
            Ok(Box::new(BrightnessContrastOp {
                input,
                output,
                brightness,
                contrast,
            }))
        }
        "hsv_adjust" => {
            let hue_offset = opt_f32(table, "hue_offset")?.unwrap_or(0.0);
            let saturation_factor = opt_f32(table, "saturation_factor")?.unwrap_or(1.0);
            let value_factor = opt_f32(table, "value_factor")?.unwrap_or(1.0);
            Ok(Box::new(HsvAdjustOp {
                hue_offset,
                saturation_factor,
                value_factor,
            }))
        }
        "gamma" => {
            let input = table
                .get("input")
                .and_then(|v| v.as_str())
                .unwrap_or("input")
                .to_string();
            let output = table
                .get("output")
                .and_then(|v| v.as_str())
                .unwrap_or("output")
                .to_string();
            let gamma = opt_f32(table, "gamma")?.unwrap_or(1.0);
            Ok(Box::new(GammaOp {
                input,
                output,
                gamma,
            }))
        }
        "clamp" => {
            let input = table
                .get("input")
                .and_then(|v| v.as_str())
                .unwrap_or("input")
                .to_string();
            let output = table
                .get("output")
                .and_then(|v| v.as_str())
                .unwrap_or("output")
                .to_string();
            let min = opt_f32(table, "min")?.unwrap_or(0.0);
            let max = opt_f32(table, "max")?.unwrap_or(1.0);
            Ok(Box::new(ClampOp {
                input,
                output,
                min,
                max,
            }))
        }
        // ── Phase 4 ──────────────────────────────────────────────────
        "blur" => {
            let input = table
                .get("input")
                .and_then(|v| v.as_str())
                .unwrap_or("input")
                .to_string();
            let output = table
                .get("output")
                .and_then(|v| v.as_str())
                .unwrap_or("output")
                .to_string();
            let radius = opt_u32(table, "radius")?.unwrap_or(2);
            let sigma = opt_f32(table, "sigma")?.unwrap_or(1.0);
            Ok(Box::new(BlurOp {
                input,
                output,
                radius,
                sigma,
            }))
        }
        "sharpen" => {
            let input = table
                .get("input")
                .and_then(|v| v.as_str())
                .unwrap_or("input")
                .to_string();
            let output = table
                .get("output")
                .and_then(|v| v.as_str())
                .unwrap_or("output")
                .to_string();
            let strength = opt_f32(table, "strength")?.unwrap_or(1.0);
            let radius = opt_u32(table, "radius")?.unwrap_or(2);
            Ok(Box::new(SharpenOp {
                input,
                output,
                strength,
                radius,
            }))
        }
        "edge_detect" => {
            let input = table
                .get("input")
                .and_then(|v| v.as_str())
                .unwrap_or("input")
                .to_string();
            let output = table
                .get("output")
                .and_then(|v| v.as_str())
                .unwrap_or("output")
                .to_string();
            Ok(Box::new(EdgeDetectOp { input, output }))
        }
        other => Err(ProcGenError::InvalidParameter {
            name: format!("ops[{index}].type"),
            reason: format!(
                "unknown op type \"{other}\"; expected one of: {:?}",
                ALL_OP_TYPES
            ),
        }),
    }
}

// ─── Parsing Helpers ──────────────────────────────────────────────────────

fn opt_f32(table: &toml::map::Map<String, toml::Value>, key: &str) -> Result<Option<f32>> {
    match table.get(key) {
        Some(v) => Ok(Some(toml_f32(v, key)?)),
        None => Ok(None),
    }
}

fn opt_u32(table: &toml::map::Map<String, toml::Value>, key: &str) -> Result<Option<u32>> {
    match table.get(key) {
        Some(v) => Ok(Some(toml_u32(v, key)?)),
        None => Ok(None),
    }
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
            assert_eq!(a.pixels, b.pixels, "same seed should produce identical pixels");
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
}
