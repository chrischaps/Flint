//! Structure-preserving save for `*.procgen.toml` specs (#194).
//!
//! The previewers used to write a spec with `toml::to_string_pretty`, which
//! drops every comment and sorts the keys, so one save turned a commented
//! example spec into a bare key dump. Here the saved text is parsed into a
//! `toml_edit` document and only what differs between the file's values and
//! the editor's is patched into it: a changed value is replaced in place
//! (its trailing comment and spacing kept), a key the editor dropped is
//! removed (the comment block above it moves to the next key), and a new key
//! goes at the end of its table. Everything else is left byte for byte.

use toml_edit::{DocumentMut, Item, TableLike};

/// Rewrite `text` so it holds `new`, changing as little of it as possible.
///
/// `new` is the whole document as the editor wants it on disk. When `text`
/// is empty or doesn't parse, the result is `new` pretty-printed.
pub fn patch_document(text: &str, new: &toml::Value) -> Result<String, String> {
    let fresh = || toml::to_string_pretty(new).map_err(|e| e.to_string());
    if text.trim().is_empty() {
        return fresh();
    }
    let (Ok(old), Ok(mut doc)) = (text.parse::<toml::Table>(), text.parse::<DocumentMut>()) else {
        return fresh();
    };
    let toml::Value::Table(new) = new else {
        return Err("a spec must be a table".into());
    };
    patch_table(doc.as_table_mut(), &old, new);
    let out = doc.to_string();
    // toml_edit writes `\n`; a Windows checkout's `\r\n` file stays `\r\n`,
    // or every line would change.
    if text.contains("\r\n") {
        Ok(out.replace("\r\n", "\n").replace('\n', "\r\n"))
    } else {
        Ok(out)
    }
}

/// Values equal for saving: an integer and a float of the same number are
/// the same (a `8` in the file read back as `8.0` is not a change).
fn same(a: &toml::Value, b: &toml::Value) -> bool {
    use toml::Value as V;
    match (a, b) {
        (V::Integer(i), V::Float(f)) | (V::Float(f), V::Integer(i)) => *i as f64 == *f,
        (V::Array(x), V::Array(y)) => {
            x.len() == y.len() && x.iter().zip(y).all(|(p, q)| same(p, q))
        }
        (V::Table(x), V::Table(y)) => {
            x.len() == y.len() && x.iter().all(|(k, v)| y.get(k).is_some_and(|w| same(v, w)))
        }
        _ => a == b,
    }
}

fn patch_table(table: &mut dyn TableLike, old: &toml::Table, new: &toml::Table) {
    // Removed keys first, so a carried-over comment lands on a key that stays.
    let removed: Vec<String> = old
        .keys()
        .filter(|k| !new.contains_key(*k))
        .cloned()
        .collect();
    for key in removed {
        remove_keeping_comment(table, &key);
    }
    for (key, nv) in new {
        match old.get(key) {
            Some(ov) if same(ov, nv) => {}
            Some(ov) => match table.get_mut(key) {
                Some(item) => patch_item(item, ov, nv),
                None => {
                    table.insert(key, to_item(nv, false));
                }
            },
            None => {
                // A new array of tables follows the file's style for arrays
                // of tables when it has one, else goes inline.
                table.insert(key, to_item(nv, false));
            }
        }
    }
}

/// Remove `key`, handing the comment lines above it to the key after it so a
/// section header (`# ── Bark ──`) survives the key that happened to start it.
fn remove_keeping_comment(table: &mut dyn TableLike, key: &str) {
    let prefix = table
        .key(key)
        .and_then(|k| k.leaf_decor().prefix())
        .and_then(|p| p.as_str())
        .map(str::to_string);
    let order: Vec<String> = table.iter().map(|(k, _)| k.to_string()).collect();
    table.remove(key);
    let Some(prefix) = prefix.filter(|p| p.contains('#')) else {
        return;
    };
    let pos = order.iter().position(|k| k == key).unwrap_or(0);
    let next = order[pos + 1..]
        .iter()
        .find(|k| table.get(k).is_some_and(|i| i.is_value()));
    if let Some(next) = next {
        if let Some(mut k) = table.key_mut(next) {
            let own = k
                .leaf_decor()
                .prefix()
                .and_then(|p| p.as_str())
                .unwrap_or("")
                .to_string();
            // The removed key's prefix ends at its own line; the next key's
            // own prefix (often empty) follows.
            k.leaf_decor_mut().set_prefix(format!("{prefix}{own}"));
        }
    }
}

fn patch_item(item: &mut Item, old: &toml::Value, new: &toml::Value) {
    use toml::Value as V;
    match (old, new) {
        (V::Table(ot), V::Table(nt)) => {
            if let Some(t) = item.as_table_like_mut() {
                patch_table(t, ot, nt);
                return;
            }
        }
        (V::Array(oa), V::Array(na)) => {
            if let Some(aot) = item.as_array_of_tables_mut() {
                patch_array_of_tables(aot, oa, na);
                return;
            }
            if let Some(arr) = item.as_array_mut() {
                if oa.len() == na.len() {
                    for (i, (o, n)) in oa.iter().zip(na).enumerate() {
                        if !same(o, n) {
                            if let Some(v) = arr.get_mut(i) {
                                replace_value(v, n);
                            }
                        }
                    }
                    return;
                }
            }
        }
        _ => {}
    }
    if let Some(v) = item.as_value_mut() {
        if !matches!(new, V::Table(_)) {
            replace_value(v, new);
            return;
        }
    }
    let prefer_table = item.is_table() || item.is_array_of_tables();
    *item = to_item(new, prefer_table);
}

/// Line up the file's `[[...]]` tables with the new list: equal-length lists
/// patch table by table; otherwise the common head and tail are kept as they
/// are (comments and all) and only the middle is rewritten, so inserting or
/// deleting one op leaves the others alone.
fn patch_array_of_tables(
    aot: &mut toml_edit::ArrayOfTables,
    old: &[toml::Value],
    new: &[toml::Value],
) {
    let all_tables = old.iter().chain(new).all(|v| v.is_table());
    if !all_tables || aot.len() != old.len() {
        *aot = new_array_of_tables(new);
        return;
    }
    if old.len() == new.len() {
        for (i, (o, n)) in old.iter().zip(new).enumerate() {
            if let (Some(t), toml::Value::Table(ot), toml::Value::Table(nt)) =
                (aot.get_mut(i), o, n)
            {
                patch_table(t, ot, nt);
            }
        }
        return;
    }
    let head = old.iter().zip(new).take_while(|(o, n)| same(o, n)).count();
    let max_tail = old.len().min(new.len()) - head;
    let tail = old
        .iter()
        .rev()
        .zip(new.iter().rev())
        .take(max_tail)
        .take_while(|(o, n)| same(o, n))
        .count();
    let mut out = toml_edit::ArrayOfTables::new();
    for i in 0..head {
        out.push(aot.get(i).cloned().unwrap_or_default());
    }
    for v in &new[head..new.len() - tail] {
        if let toml::Value::Table(t) = v {
            out.push(new_table(t));
        }
    }
    for i in old.len() - tail..old.len() {
        out.push(aot.get(i).cloned().unwrap_or_default());
    }
    *aot = out;
}

/// The texture node editor's ops as the file should hold them.
///
/// Loading an op fills in every parameter's default, and the editor gives
/// every node an `id` and writes its wires as `inputs`. Written back as is,
/// one changed value would add a dozen lines. So: a key the file's op at the
/// same place didn't have, holding its default, is dropped; and when the file
/// named no ids at all and the ops without `id`/`inputs` wire up the same
/// graph by inference, those go too. Neither changes what the spec makes.
pub fn pipeline_ops_for_save(
    ops: &[toml::Value],
    source: &[toml::Value],
    output_maps: &[String],
) -> Vec<toml::Value> {
    use flint_procgen::generators::texture::{node_graph, pipeline};
    let type_of = |v: &toml::Value| v.get("type").and_then(|t| t.as_str()).map(str::to_string);
    let mut out: Vec<toml::Value> = ops
        .iter()
        .enumerate()
        .map(|(i, op)| {
            let Some(table) = op.as_table() else {
                return op.clone();
            };
            let ty = type_of(op).unwrap_or_default();
            let src = source
                .get(i)
                .filter(|s| type_of(s).as_deref() == Some(ty.as_str()))
                .and_then(|s| s.as_table());
            let defaults = pipeline::op_default_params(&ty);
            let kept = table
                .iter()
                .filter(|(k, v)| {
                    k.as_str() == "type"
                        || src.is_some_and(|s| s.contains_key(*k))
                        || !defaults.get(*k).is_some_and(|d| same(d, v))
                })
                .map(|(k, v)| (k.clone(), v.clone()))
                .collect();
            toml::Value::Table(kept)
        })
        .collect();

    if !node_graph::has_explicit_connections(source) {
        let stripped: Vec<toml::Value> = out
            .iter()
            .map(|op| match op {
                toml::Value::Table(t) => {
                    let mut t = t.clone();
                    t.remove("id");
                    t.remove("inputs");
                    toml::Value::Table(t)
                }
                other => other.clone(),
            })
            .collect();
        let wires = |ops: &[toml::Value]| {
            node_graph::ops_to_graph(ops, output_maps)
                .ok()
                .map(|(_, c)| {
                    let mut c: Vec<_> = c
                        .iter()
                        .map(|c| (c.from_node, c.from_pin, c.to_node, c.to_pin))
                        .collect();
                    c.sort_unstable();
                    c
                })
        };
        if wires(&out).is_some() && wires(&out) == wires(&stripped) {
            out = stripped;
        }
    }
    out
}

fn replace_value(v: &mut toml_edit::Value, new: &toml::Value) {
    let decor = v.decor().clone();
    *v = to_value(new);
    *v.decor_mut() = decor;
}

fn new_table(t: &toml::Table) -> toml_edit::Table {
    let mut tbl = toml_edit::Table::new();
    for (k, x) in t {
        tbl.insert(k, to_item(x, false));
    }
    tbl
}

fn new_array_of_tables(items: &[toml::Value]) -> toml_edit::ArrayOfTables {
    let mut aot = toml_edit::ArrayOfTables::new();
    for x in items {
        if let toml::Value::Table(t) = x {
            aot.push(new_table(t));
        }
    }
    aot
}

/// A `toml::Value` as a `toml_edit::Item`: inline unless `prefer_table` asks
/// for a standard table / array of tables, to match what the file used.
fn to_item(v: &toml::Value, prefer_table: bool) -> Item {
    match v {
        toml::Value::Table(t) if prefer_table => Item::Table(new_table(t)),
        toml::Value::Array(items)
            if prefer_table && !items.is_empty() && items.iter().all(|x| x.is_table()) =>
        {
            Item::ArrayOfTables(new_array_of_tables(items))
        }
        _ => Item::Value(to_value(v)),
    }
}

fn to_value(v: &toml::Value) -> toml_edit::Value {
    match v {
        toml::Value::String(s) => toml_edit::Value::from(s.as_str()),
        toml::Value::Integer(i) => toml_edit::Value::from(*i),
        toml::Value::Float(f) => toml_edit::Value::from(*f),
        toml::Value::Boolean(b) => toml_edit::Value::from(*b),
        toml::Value::Datetime(d) => toml_edit::Value::from(d.to_string()),
        toml::Value::Array(items) => {
            let mut arr = toml_edit::Array::new();
            for x in items {
                arr.push(to_value(x));
            }
            toml_edit::Value::Array(arr)
        }
        toml::Value::Table(t) => {
            let mut it = toml_edit::InlineTable::new();
            for (k, x) in t {
                it.insert(k, to_value(x));
            }
            toml_edit::Value::InlineTable(it)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn changed_lines(a: &str, b: &str) -> Vec<(String, String)> {
        assert_eq!(
            a.lines().count(),
            b.lines().count(),
            "line count moved:\n{b}"
        );
        a.lines()
            .zip(b.lines())
            .filter(|(x, y)| x != y)
            .map(|(x, y)| (x.to_string(), y.to_string()))
            .collect()
    }

    fn with(text: &str, edit: impl FnOnce(&mut toml::Table)) -> toml::Value {
        let mut t: toml::Table = text.parse().unwrap();
        edit(&mut t);
        toml::Value::Table(t)
    }

    fn params(t: &mut toml::Table) -> &mut toml::Table {
        t.get_mut("params").unwrap().as_table_mut().unwrap()
    }

    const OAK: &str = include_str!("../../../../specs/examples/oak_tree.procgen.toml");
    const PLANK: &str = include_str!("../../../../specs/examples/wood_plank.procgen.toml");

    #[test]
    fn unchanged_document_is_byte_identical() {
        let lf = |t: &str| t.replace("\r\n", "\n");
        let crlf = |t: &str| lf(t).replace('\n', "\r\n");
        for text in [lf(OAK), crlf(OAK), lf(PLANK), crlf(PLANK)] {
            let v = with(&text, |_| {});
            assert_eq!(patch_document(&text, &v).unwrap(), text);
        }
    }

    #[test]
    fn one_slider_changes_one_line() {
        let v = with(OAK, |t| {
            params(t).insert("trunk_height".into(), toml::Value::Float(5.25));
        });
        let out = patch_document(OAK, &v).unwrap();
        assert_eq!(
            changed_lines(OAK, &out),
            [(
                "trunk_height = 4.5".to_string(),
                "trunk_height = 5.25".to_string()
            )]
        );
    }

    #[test]
    fn trailing_comment_survives_a_change() {
        let v = with(OAK, |t| {
            params(t).insert("trunk_radius_top".into(), toml::Value::Float(0.2));
        });
        let out = patch_document(OAK, &v).unwrap();
        let lines = changed_lines(OAK, &out);
        assert_eq!(lines.len(), 1);
        assert!(
            lines[0].1.starts_with("trunk_radius_top = 0.2 "),
            "{lines:?}"
        );
        assert!(lines[0]
            .1
            .ends_with("# the limbs off the tip start this thick"));
    }

    #[test]
    fn integer_read_back_as_float_is_not_a_change() {
        let v = with(OAK, |t| {
            let p = params(t);
            p.insert("trunk_segments".into(), toml::Value::Float(12.0));
        });
        assert_eq!(patch_document(OAK, &v).unwrap(), OAK);
    }

    #[test]
    fn new_key_goes_at_the_end_of_its_table() {
        let src = "[params]\n# a\na = 1\nb = 2\n\n[lod]\nx = 1\n";
        let v = with(src, |t| {
            params(t).insert("c".into(), toml::Value::Integer(3));
        });
        let out = patch_document(src, &v).unwrap();
        assert_eq!(out, "[params]\n# a\na = 1\nb = 2\nc = 3\n\n[lod]\nx = 1\n");
    }

    #[test]
    fn removed_key_hands_its_header_to_the_next() {
        let src = "[params]\na = 1\n\n# ── Bark ──\nb = 2\nc = 3\n";
        let v = with(src, |t| {
            params(t).remove("b");
        });
        let out = patch_document(src, &v).unwrap();
        assert_eq!(out, "[params]\na = 1\n\n# ── Bark ──\nc = 3\n");
    }

    #[test]
    fn pipeline_op_value_changes_one_line() {
        let v = with(PLANK, |t| {
            let ops = params(t).get_mut("ops").unwrap().as_array_mut().unwrap();
            ops[0]
                .as_table_mut()
                .unwrap()
                .insert("rows".into(), toml::Value::Integer(9));
        });
        let out = patch_document(PLANK, &v).unwrap();
        let lines = changed_lines(PLANK, &out);
        assert_eq!(lines.len(), 1, "{lines:?}");
        assert!(lines[0].1.starts_with("rows = 9 "), "{lines:?}");
        assert!(lines[0].1.ends_with("# rows of planks down it"));
    }

    #[test]
    fn inserting_an_op_keeps_the_others_and_their_comments() {
        let v = with(PLANK, |t| {
            let ops = params(t).get_mut("ops").unwrap().as_array_mut().unwrap();
            let mut op = toml::Table::new();
            op.insert("type".into(), toml::Value::String("invert".into()));
            ops.insert(1, toml::Value::Table(op));
        });
        let out = patch_document(PLANK, &v).unwrap();
        assert!(out.contains("# 1. The boards."), "{out}");
        assert!(out.contains("# 2. Each plank sits"), "{out}");
        assert!(out.contains("# rows of planks down it"), "{out}");
        assert_eq!(
            out.matches("[[params.ops]]").count(),
            PLANK.matches("[[params.ops]]").count() + 1
        );
        let back: toml::Value = toml::from_str(&out).unwrap();
        assert!(same(&back, &v));
    }

    /// The ops as the node editor holds them after loading `text`: every
    /// default filled in, an id on each node and each wire as `inputs`
    /// (what `tex_edit::collect_ops_toml` writes).
    fn editor_ops(text: &str) -> (Vec<toml::Value>, Vec<toml::Value>, Vec<String>) {
        use flint_procgen::generators::texture::node_graph::{ops_to_graph, TextureNode};
        let doc: toml::Table = text.parse().unwrap();
        let params = doc["params"].as_table().unwrap();
        let source = params["ops"].as_array().unwrap().clone();
        let maps: Vec<String> = params["output_maps"]
            .as_array()
            .unwrap()
            .iter()
            .map(|m| m.as_str().unwrap().to_string())
            .collect();
        let (nodes, conns) = ops_to_graph(&source, &maps).unwrap();
        let ids: Vec<String> = nodes
            .iter()
            .enumerate()
            .map(|(i, n)| format!("{}_{i}", n.op_type().unwrap_or("out")))
            .collect();
        let mut ops = Vec::new();
        for (i, node) in nodes.iter().enumerate() {
            let TextureNode::Op { params, .. } = node else {
                continue;
            };
            let mut t = params.clone();
            t.insert("id".into(), toml::Value::String(ids[i].clone()));
            let mut inputs = toml::Table::new();
            for c in conns.iter().filter(|c| c.to_node == i) {
                let ch = node.input_channels()[c.to_pin].to_string();
                let src = nodes[c.from_node].output_channels()[c.from_pin].to_string();
                inputs.insert(
                    ch,
                    toml::Value::String(format!("{}.{src}", ids[c.from_node])),
                );
            }
            if !inputs.is_empty() {
                t.insert("inputs".into(), toml::Value::Table(inputs));
            }
            ops.push(toml::Value::Table(t));
        }
        (ops, source, maps)
    }

    fn save_pipeline(text: &str, edit: impl FnOnce(&mut Vec<toml::Value>)) -> String {
        let (mut ops, source, maps) = editor_ops(text);
        edit(&mut ops);
        let ops = pipeline_ops_for_save(&ops, &source, &maps);
        let v = with(text, |t| {
            params(t).insert("ops".into(), toml::Value::Array(ops));
        });
        patch_document(text, &v).unwrap()
    }

    #[test]
    fn node_editor_save_with_no_change_is_byte_identical() {
        assert_eq!(save_pipeline(PLANK, |_| {}), PLANK);
    }

    #[test]
    fn node_editor_one_value_changes_one_line() {
        let out = save_pipeline(PLANK, |ops| {
            ops[0]
                .as_table_mut()
                .unwrap()
                .insert("stagger".into(), toml::Value::Float(0.5));
        });
        let lines = changed_lines(PLANK, &out);
        assert_eq!(lines.len(), 1, "{lines:?}");
        assert!(lines[0].1.starts_with("stagger = 0.5 "), "{lines:?}");
    }

    #[test]
    fn node_editor_keeps_wires_inference_cannot_rebuild() {
        // Unhook the second op's input: inference would wire it again, so
        // the ids and inputs have to stay.
        let (mut ops, source, maps) = editor_ops(PLANK);
        ops[1].as_table_mut().unwrap().remove("inputs");
        let saved = pipeline_ops_for_save(&ops, &source, &maps);
        assert!(saved[0].get("id").is_some());
    }

    #[test]
    fn unparseable_text_falls_back_to_a_fresh_write() {
        let v = with("[params]\na = 1\n", |_| {});
        let out = patch_document("not = [toml", &v).unwrap();
        assert_eq!(out, toml::to_string_pretty(&v).unwrap());
    }
}
