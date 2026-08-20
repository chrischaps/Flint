//! Graph data model for the texture pipeline node editor.
//!
//! Converts between the sequential `[[params.ops]]` TOML representation and
//! a node graph that can be displayed/edited by egui-snarl.

use std::collections::HashMap;

use super::ops::OpPortInfo;
use super::pipeline::{op_default_params, parse_op};
use crate::{ProcGenError, Result};

// ─── TextureNode ─────────────────────────────────────────────────────────

/// A node in the texture pipeline graph.
#[derive(Debug, Clone)]
pub enum TextureNode {
    /// A texture operation with its type, parameters, and port metadata.
    Op {
        op_type: String,
        params: toml::map::Map<String, toml::Value>,
        port_info: OpPortInfo,
        /// Optional unique node identifier for explicit wiring (e.g. `"grid"`, `"heights"`).
        id: Option<String>,
        /// Explicit input mappings: `local_channel -> "source_node.channel"`.
        inputs: HashMap<String, String>,
    },
    /// The final output node that gathers the pipeline results.
    Output { output_maps: Vec<String> },
}

impl TextureNode {
    /// Create a new Op node from an op type string, using default parameters.
    pub fn new_op(op_type: &str) -> Result<Self> {
        let params = op_default_params(op_type);
        let toml_val = toml::Value::Table(params.clone());
        let op = parse_op(&toml_val, 0)?;
        let port_info = op.port_info();
        Ok(TextureNode::Op {
            op_type: op_type.to_string(),
            params,
            port_info,
            id: None,
            inputs: HashMap::new(),
        })
    }

    /// Create a new Op node from existing TOML parameters.
    ///
    /// Missing params are filled from schema defaults so that dynamic channel
    /// placeholders (e.g. `<output>`) can always be resolved from the params map.
    pub fn from_toml(value: &toml::Value, index: usize) -> Result<Self> {
        let op = parse_op(value, index)?;
        let port_info = op.port_info();
        let mut params = value.as_table().cloned().unwrap_or_default();
        let op_type = params
            .get("type")
            .and_then(|v| v.as_str())
            .unwrap_or("unknown")
            .to_string();

        // Fill in schema defaults for any missing params so dynamic placeholders
        // like <output>, <source>, <target> can always be resolved.
        let defaults = super::pipeline::op_default_params(&op_type);
        for (key, val) in &defaults {
            if key != "type" && !params.contains_key(key) {
                params.insert(key.clone(), val.clone());
            }
        }

        // Extract `id` field (not a pipeline param)
        let id = params
            .remove("id")
            .and_then(|v| v.as_str().map(|s| s.to_string()));

        // Extract `inputs` table (not a pipeline param)
        let inputs = params
            .remove("inputs")
            .and_then(|v| {
                v.as_table().map(|t| {
                    t.iter()
                        .filter_map(|(k, v)| v.as_str().map(|s| (k.clone(), s.to_string())))
                        .collect()
                })
            })
            .unwrap_or_default();

        Ok(TextureNode::Op {
            op_type,
            params,
            port_info,
            id,
            inputs,
        })
    }

    /// Create the output node.
    pub fn new_output(output_maps: Vec<String>) -> Self {
        TextureNode::Output { output_maps }
    }

    /// Get a human-readable label for this node.
    pub fn label(&self) -> &str {
        match self {
            TextureNode::Op { port_info, .. } => port_info.label,
            TextureNode::Output { .. } => "Output",
        }
    }

    /// Get the op type, if this is an Op node.
    pub fn op_type(&self) -> Option<&str> {
        match self {
            TextureNode::Op { op_type, .. } => Some(op_type),
            TextureNode::Output { .. } => None,
        }
    }

    /// Get all output channel names (channels this node writes or modifies).
    pub fn output_channels(&self) -> Vec<&str> {
        match self {
            TextureNode::Op { port_info, .. } => {
                let mut channels: Vec<&str> = Vec::new();
                channels.extend_from_slice(port_info.writes);
                channels.extend_from_slice(port_info.modifies);
                channels
            }
            TextureNode::Output { .. } => vec![],
        }
    }

    /// Get all input channel names (channels this node reads or modifies).
    pub fn input_channels(&self) -> Vec<&str> {
        match self {
            TextureNode::Op { port_info, .. } => {
                let mut channels: Vec<&str> = Vec::new();
                channels.extend_from_slice(port_info.reads);
                channels.extend_from_slice(port_info.modifies);
                channels
            }
            TextureNode::Output { output_maps, .. } => {
                // The output node implicitly reads color/normal/roughness channels
                let mut channels = Vec::new();
                for map in output_maps {
                    match map.as_str() {
                        "albedo" => channels.extend_from_slice(&["r", "g", "b"]),
                        "normal" => {
                            channels.extend_from_slice(&["normal_x", "normal_y", "normal_z"])
                        }
                        "roughness" => channels.push("roughness"),
                        _ => {}
                    }
                }
                channels
            }
        }
    }
}

// ─── Graph Conversion ────────────────────────────────────────────────────

/// Connection between two nodes in the graph.
#[derive(Debug, Clone)]
pub struct Connection {
    /// Source node index.
    pub from_node: usize,
    /// Source output pin index.
    pub from_pin: usize,
    /// Destination node index.
    pub to_node: usize,
    /// Destination input pin index.
    pub to_pin: usize,
}

/// Check whether any op in the array has an `id` field (explicit wiring mode).
pub fn has_explicit_connections(ops_array: &[toml::Value]) -> bool {
    ops_array
        .iter()
        .any(|v| v.as_table().map(|t| t.contains_key("id")).unwrap_or(false))
}

/// Parse explicit `inputs` references from a set of nodes into [`Connection`]s.
///
/// Each input entry like `"cell_id" = "grid.cell_id"` becomes a connection
/// from the node named `"grid"` (output pin for `"cell_id"`) to the current
/// node's input pin for `"cell_id"`.
pub fn explicit_connections(nodes: &[TextureNode]) -> Vec<Connection> {
    // Build node-id → index lookup
    let id_to_idx: HashMap<&str, usize> = nodes
        .iter()
        .enumerate()
        .filter_map(|(i, n)| match n {
            TextureNode::Op { id: Some(id), .. } => Some((id.as_str(), i)),
            _ => None,
        })
        .collect();

    let mut connections = Vec::new();

    for (node_idx, node) in nodes.iter().enumerate() {
        let inputs_map = match node {
            TextureNode::Op { inputs, .. } if !inputs.is_empty() => inputs,
            _ => continue,
        };

        let in_channels = node.input_channels();

        for (local_channel, ref_str) in inputs_map {
            // Parse "source_node.channel"
            let (src_node_id, src_channel) = match ref_str.split_once('.') {
                Some(pair) => pair,
                None => continue,
            };

            let src_idx = match id_to_idx.get(src_node_id) {
                Some(&idx) => idx,
                None => continue,
            };

            let src_node = &nodes[src_idx];
            let out_channels = src_node.output_channels();

            let from_pin = out_channels
                .iter()
                .position(|ch| *ch == src_channel)
                .unwrap_or(0);

            let to_pin = in_channels
                .iter()
                .position(|ch| *ch == local_channel.as_str())
                .unwrap_or(0);

            connections.push(Connection {
                from_node: src_idx,
                from_pin,
                to_node: node_idx,
                to_pin,
            });
        }
    }

    connections
}

/// Topological sort using Kahn's algorithm.
///
/// `edges` are `(from, to)` pairs. Returns node indices in execution order.
/// Returns an error if the graph contains a cycle.
pub fn topological_sort(node_count: usize, edges: &[(usize, usize)]) -> Result<Vec<usize>> {
    let mut in_degree = vec![0usize; node_count];
    let mut adjacency: Vec<Vec<usize>> = vec![Vec::new(); node_count];

    for &(from, to) in edges {
        adjacency[from].push(to);
        in_degree[to] += 1;
    }

    let mut queue: std::collections::VecDeque<usize> = in_degree
        .iter()
        .enumerate()
        .filter(|(_, &d)| d == 0)
        .map(|(i, _)| i)
        .collect();

    let mut order = Vec::with_capacity(node_count);

    while let Some(node) = queue.pop_front() {
        order.push(node);
        for &neighbor in &adjacency[node] {
            in_degree[neighbor] -= 1;
            if in_degree[neighbor] == 0 {
                queue.push_back(neighbor);
            }
        }
    }

    if order.len() != node_count {
        return Err(ProcGenError::InvalidParameter {
            name: "ops".into(),
            reason: "cycle detected in node graph".into(),
        });
    }

    Ok(order)
}

/// Convert a `[[params.ops]]` TOML array into a graph of nodes + connections.
///
/// Nodes are created from ops, plus a final Output node. Connections are
/// inferred by matching channel names, or derived from explicit `id`/`inputs`
/// when any op has an `id` field.
pub fn ops_to_graph(
    ops_array: &[toml::Value],
    output_maps: &[String],
) -> Result<(Vec<TextureNode>, Vec<Connection>)> {
    let mut nodes = Vec::with_capacity(ops_array.len() + 1);

    for (i, op_val) in ops_array.iter().enumerate() {
        nodes.push(TextureNode::from_toml(op_val, i)?);
    }

    nodes.push(TextureNode::new_output(output_maps.to_vec()));

    let connections = if has_explicit_connections(ops_array) {
        // Explicit mode: derive connections from id/inputs, but still infer
        // output node connections since it doesn't have explicit inputs
        let mut conns = explicit_connections(&nodes);

        // Infer connections for the output node (last node) only
        let output_idx = nodes.len() - 1;
        let output_conns = infer_output_connections(&nodes, output_idx);
        conns.extend(output_conns);
        conns
    } else {
        infer_connections(&nodes)
    };

    Ok((nodes, connections))
}

/// Serialize graph nodes (excluding the Output node) back to `[[params.ops]]` TOML.
pub fn graph_to_ops(nodes: &[TextureNode]) -> Vec<toml::Value> {
    nodes
        .iter()
        .filter_map(|node| match node {
            TextureNode::Op {
                params, id, inputs, ..
            } => {
                let mut table = params.clone();
                if let Some(id) = id {
                    table.insert("id".to_string(), toml::Value::String(id.clone()));
                }
                if !inputs.is_empty() {
                    let mut inputs_table = toml::map::Map::new();
                    for (k, v) in inputs {
                        inputs_table.insert(k.clone(), toml::Value::String(v.clone()));
                    }
                    table.insert("inputs".to_string(), toml::Value::Table(inputs_table));
                }
                Some(toml::Value::Table(table))
            }
            TextureNode::Output { .. } => None,
        })
        .collect()
}

/// Infer connections for the output node only (used in explicit mode).
///
/// Scans all earlier nodes to find the last producer of each channel the
/// output node reads.
fn infer_output_connections(nodes: &[TextureNode], output_idx: usize) -> Vec<Connection> {
    let output_node = &nodes[output_idx];
    let output_inputs = output_node.input_channels();

    let mut channel_producers: Vec<(String, usize)> = Vec::new();
    for (i, node) in nodes.iter().enumerate() {
        if i == output_idx {
            break;
        }
        for ch in node.output_channels() {
            if let Some(entry) = channel_producers.iter_mut().find(|(c, _)| c == ch) {
                entry.1 = i;
            } else {
                channel_producers.push((ch.to_string(), i));
            }
        }
    }

    let mut connections = Vec::new();
    for input_ch in &output_inputs {
        if let Some((_, producer_idx)) = channel_producers
            .iter()
            .rev()
            .find(|(ch, _)| ch == input_ch)
        {
            let producer = &nodes[*producer_idx];
            let out_channels = producer.output_channels();
            let from_pin = out_channels
                .iter()
                .position(|ch| ch == input_ch)
                .unwrap_or(0);
            let to_pin = output_inputs
                .iter()
                .position(|ch| ch == input_ch)
                .unwrap_or(0);
            connections.push(Connection {
                from_node: *producer_idx,
                from_pin,
                to_node: output_idx,
                to_pin,
            });
        }
    }
    connections
}

/// Resolve dynamic placeholders like `<target>` and `<source>` in a channel
/// list to the actual channel names from the node's parameters.
fn resolve_channels(node: &TextureNode, channels: &[&str]) -> Vec<String> {
    channels
        .iter()
        .map(|ch| {
            if ch.starts_with('<') && ch.ends_with('>') {
                let param_name = &ch[1..ch.len() - 1]; // e.g. "target", "source"
                if let TextureNode::Op { params, .. } = node {
                    params
                        .get(param_name)
                        .and_then(|v| v.as_str())
                        .unwrap_or("height")
                        .to_string()
                } else {
                    ch.to_string()
                }
            } else {
                ch.to_string()
            }
        })
        .collect()
}

/// Infer connections between nodes based on channel name matching.
///
/// For each node, find which earlier node provides each of its input channels.
fn infer_connections(nodes: &[TextureNode]) -> Vec<Connection> {
    let mut connections = Vec::new();

    // Track which channels each node produces (cumulative from prior nodes)
    // For each channel, track which node last wrote/modified it
    let mut channel_producers: Vec<(String, usize)> = Vec::new();

    for (node_idx, node) in nodes.iter().enumerate() {
        let raw_inputs = node.input_channels();
        let inputs = resolve_channels(node, &raw_inputs);

        for (pin_idx, input_ch) in inputs.iter().enumerate() {
            // Find the most recent producer of this channel
            if let Some((_, producer_idx)) = channel_producers
                .iter()
                .rev()
                .find(|(ch, _)| ch == input_ch)
            {
                let producer = &nodes[*producer_idx];
                let raw_out = producer.output_channels();
                let out_channels = resolve_channels(producer, &raw_out);
                let from_pin = out_channels
                    .iter()
                    .position(|ch| ch == input_ch)
                    .unwrap_or(0);

                connections.push(Connection {
                    from_node: *producer_idx,
                    from_pin,
                    to_node: node_idx,
                    to_pin: pin_idx,
                });
            }
        }

        // Register this node's outputs
        let raw_outputs = node.output_channels();
        let outputs = resolve_channels(node, &raw_outputs);
        for ch in outputs {
            // Update or add the producer for this channel
            if let Some(entry) = channel_producers.iter_mut().find(|(c, _)| *c == ch) {
                entry.1 = node_idx;
            } else {
                channel_producers.push((ch, node_idx));
            }
        }
    }

    connections
}

// ─── Tests ────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_op_creates_valid_node() {
        let node = TextureNode::new_op("brick_grid").unwrap();
        assert_eq!(node.label(), "Brick Grid");
        assert_eq!(node.op_type(), Some("brick_grid"));
        assert!(node.output_channels().contains(&"cell_id"));
        assert!(node.output_channels().contains(&"edge_dist"));
        assert!(node.input_channels().is_empty());
    }

    #[test]
    fn new_op_unknown_type_errors() {
        assert!(TextureNode::new_op("magic_op").is_err());
    }

    #[test]
    fn output_node_input_channels() {
        let node =
            TextureNode::new_output(vec!["albedo".into(), "normal".into(), "roughness".into()]);
        let inputs = node.input_channels();
        assert!(inputs.contains(&"r"));
        assert!(inputs.contains(&"g"));
        assert!(inputs.contains(&"b"));
        assert!(inputs.contains(&"normal_x"));
        assert!(inputs.contains(&"roughness"));
    }

    #[test]
    fn ops_to_graph_roundtrip() {
        let ops_toml: Vec<toml::Value> = vec![
            toml::toml! { type = "brick_grid" columns = 4 rows = 8 }.into(),
            toml::toml! { type = "cell_height" variation = 0.3 }.into(),
            toml::toml! { type = "cell_color" base_color = "#8B5A3A" }.into(),
        ];
        let output_maps = vec!["albedo".into(), "normal".into(), "roughness".into()];

        let (nodes, connections) = ops_to_graph(&ops_toml, &output_maps).unwrap();
        assert_eq!(nodes.len(), 4); // 3 ops + 1 output
        assert!(!connections.is_empty());

        // Roundtrip
        let ops_back = graph_to_ops(&nodes);
        assert_eq!(ops_back.len(), 3);
        assert_eq!(
            ops_back[0]
                .as_table()
                .unwrap()
                .get("type")
                .unwrap()
                .as_str()
                .unwrap(),
            "brick_grid"
        );
    }

    #[test]
    fn infer_connections_basic() {
        let nodes = vec![
            TextureNode::new_op("brick_grid").unwrap(),
            TextureNode::new_op("cell_height").unwrap(),
            TextureNode::new_output(vec!["albedo".into()]),
        ];

        let conns = infer_connections(&nodes);
        // cell_height reads cell_id from brick_grid
        assert!(conns.iter().any(|c| c.from_node == 0 && c.to_node == 1));
    }

    #[test]
    fn has_explicit_connections_detection() {
        let without: Vec<toml::Value> = vec![
            toml::toml! { type = "brick_grid" columns = 4 rows = 8 }.into(),
            toml::toml! { type = "cell_height" variation = 0.3 }.into(),
        ];
        assert!(!has_explicit_connections(&without));

        let with: Vec<toml::Value> = vec![
            toml::toml! { type = "brick_grid" id = "grid" columns = 4 rows = 8 }.into(),
            toml::toml! { type = "cell_height" variation = 0.3 }.into(),
        ];
        assert!(has_explicit_connections(&with));
    }

    #[test]
    fn from_toml_parses_id_and_inputs() {
        let val: toml::Value = toml::toml! {
            type = "cell_height"
            id = "heights"
            variation = 0.3
            [inputs]
            cell_id = "grid.cell_id"
        }
        .into();

        let node = TextureNode::from_toml(&val, 0).unwrap();
        match &node {
            TextureNode::Op {
                id, inputs, params, ..
            } => {
                assert_eq!(id.as_deref(), Some("heights"));
                assert_eq!(
                    inputs.get("cell_id").map(|s| s.as_str()),
                    Some("grid.cell_id")
                );
                // id and inputs should be stripped from params
                assert!(!params.contains_key("id"));
                assert!(!params.contains_key("inputs"));
            }
            _ => panic!("expected Op"),
        }
    }

    #[test]
    fn graph_to_ops_preserves_id_and_inputs() {
        let val: toml::Value = toml::toml! {
            type = "cell_height"
            id = "heights"
            variation = 0.3
            [inputs]
            cell_id = "grid.cell_id"
        }
        .into();

        let node = TextureNode::from_toml(&val, 0).unwrap();
        let ops = graph_to_ops(&[node]);
        assert_eq!(ops.len(), 1);
        let table = ops[0].as_table().unwrap();
        assert_eq!(table.get("id").unwrap().as_str().unwrap(), "heights");
        let inputs = table.get("inputs").unwrap().as_table().unwrap();
        assert_eq!(
            inputs.get("cell_id").unwrap().as_str().unwrap(),
            "grid.cell_id"
        );
    }

    #[test]
    fn explicit_connections_basic() {
        let ops_toml: Vec<toml::Value> = vec![
            toml::toml! { type = "brick_grid" id = "grid" columns = 4 rows = 8 }.into(),
            {
                let v: toml::Value = toml::toml! {
                    type = "cell_height"
                    id = "heights"
                    variation = 0.3
                    [inputs]
                    cell_id = "grid.cell_id"
                }
                .into();
                v
            },
        ];

        let mut nodes = Vec::new();
        for (i, v) in ops_toml.iter().enumerate() {
            nodes.push(TextureNode::from_toml(v, i).unwrap());
        }
        nodes.push(TextureNode::new_output(vec!["albedo".into()]));

        let conns = explicit_connections(&nodes);
        assert_eq!(conns.len(), 1);
        assert_eq!(conns[0].from_node, 0);
        assert_eq!(conns[0].to_node, 1);
    }

    #[test]
    fn topological_sort_basic() {
        // 0 -> 1 -> 2
        let edges = vec![(0, 1), (1, 2)];
        let order = topological_sort(3, &edges).unwrap();
        assert_eq!(order, vec![0, 1, 2]);
    }

    #[test]
    fn topological_sort_diamond() {
        // 0 -> 1, 0 -> 2, 1 -> 3, 2 -> 3
        let edges = vec![(0, 1), (0, 2), (1, 3), (2, 3)];
        let order = topological_sort(4, &edges).unwrap();
        assert_eq!(order[0], 0); // source first
        assert_eq!(order[3], 3); // sink last
                                 // 1 and 2 can be in either order
        assert!(order.contains(&1));
        assert!(order.contains(&2));
    }

    #[test]
    fn topological_sort_cycle_error() {
        let edges = vec![(0, 1), (1, 0)];
        assert!(topological_sort(2, &edges).is_err());
    }

    #[test]
    fn ops_to_graph_explicit_mode() {
        let ops_toml: Vec<toml::Value> = vec![
            toml::toml! { type = "brick_grid" id = "grid" columns = 4 rows = 8 }.into(),
            {
                let v: toml::Value = toml::toml! {
                    type = "cell_height"
                    id = "heights"
                    variation = 0.3
                    [inputs]
                    cell_id = "grid.cell_id"
                }
                .into();
                v
            },
            {
                let v: toml::Value = toml::toml! {
                    type = "cell_color"
                    id = "color"
                    base_color = "#8B5A3A"
                    [inputs]
                    cell_id = "grid.cell_id"
                }
                .into();
                v
            },
        ];

        let output_maps = vec!["albedo".into()];
        let (nodes, connections) = ops_to_graph(&ops_toml, &output_maps).unwrap();

        // 3 ops + 1 output
        assert_eq!(nodes.len(), 4);

        // grid -> heights (cell_id), grid -> color (cell_id), and output connections
        assert!(connections
            .iter()
            .any(|c| c.from_node == 0 && c.to_node == 1));
        assert!(connections
            .iter()
            .any(|c| c.from_node == 0 && c.to_node == 2));
        // Output node should have connections inferred from last producers
        assert!(connections.iter().any(|c| c.to_node == 3));
    }
}
