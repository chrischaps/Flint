//! TOML loading for .ui.toml layout files and .style.toml style files.
//!
//! Both are strict (#731): a `type`, `anchor`, key, parent, class or style
//! property the loader doesn't know is an error naming the file and the
//! element (or class), never a quiet default. Every problem in a file is
//! reported at once. Elements keep the order the file writes them in, which
//! is the order siblings stack and are drawn in.

use super::element::{Anchor, Dir, ElementType, StyleValue, UiElement};
use super::style::{property_problem, StyleClass, UiState};
use super::tokens::{toml_to_style_value, TokenSet};
use flint_core::vfs;
use std::collections::HashMap;
use std::path::Path;

/// Every key an element's table may hold, and which types each is for
/// (`None`: every type).
const ELEMENT_KEYS: &[(&str, Option<&[ElementType]>)] = &[
    ("type", None),
    ("class", None),
    ("anchor", None),
    ("parent", None),
    ("text", None),
    ("visible", None),
    ("focusable", None),
    ("enabled", None),
    ("autofocus", None),
    ("nav_up", None),
    ("nav_down", None),
    ("nav_left", None),
    ("nav_right", None),
    ("src", Some(&[ElementType::Image])),
    ("value", Some(&[ElementType::Slider, ElementType::List])),
    ("min", Some(&[ElementType::Slider])),
    ("max", Some(&[ElementType::Slider])),
    ("step", Some(&[ElementType::Slider])),
    ("items", Some(&[ElementType::List])),
    ("item_class", Some(&[ElementType::List])),
    ("wrap", Some(&[ElementType::List])),
];

/// The id a list gives the button it makes for its `index`th item.
pub fn item_id(list: &str, index: usize) -> String {
    format!("{list}.{index}")
}

/// A layout's elements, in file order, and the style file it names.
pub struct Layout {
    pub elements: Vec<UiElement>,
    pub style: String,
}

/// Parse a .ui.toml layout file. On any problem, every problem, each a line
/// naming the file and the element.
pub fn load_layout(path: &Path) -> Result<Layout, Vec<String>> {
    let shown = path.display().to_string();
    let content = vfs::read_to_string(path).map_err(|e| vec![format!("{shown}: {e}")])?;
    parse_layout(&content, &shown)
}

/// [`load_layout`] on text already read; `shown` is the name problems give.
pub fn parse_layout(content: &str, shown: &str) -> Result<Layout, Vec<String>> {
    let doc: toml::Table = content
        .parse()
        .map_err(|e| vec![format!("{shown}: not TOML: {e}")])?;
    let order = element_order(content);
    let mut problems = Vec::new();
    let mut problem = |what: String| problems.push(format!("{shown}: {what}"));

    for key in doc.keys() {
        if key != "ui" && key != "elements" {
            problem(format!(
                "unknown table `{key}` (a layout has [ui] and [elements.<id>])"
            ));
        }
    }

    let mut style = String::new();
    if let Some(ui) = doc.get("ui") {
        match ui.as_table() {
            Some(ui) => {
                for (key, value) in ui {
                    match (key.as_str(), value.as_str()) {
                        ("style", Some(s)) => style = s.to_string(),
                        ("style", None) => problem("[ui] style is a path, a string".into()),
                        // A title for the reader; nothing draws it
                        ("name", Some(_)) => {}
                        ("name", None) => problem("[ui] name is a string".into()),
                        _ => problem(format!(
                            "[ui]: unknown key `{key}` (it has `style` and `name`)"
                        )),
                    }
                }
            }
            None => problem("`ui` is a table, [ui]".into()),
        }
    }

    let mut elements = Vec::new();
    if let Some(table) = doc.get("elements") {
        let Some(table) = table.as_table() else {
            problem("`elements` is a table of elements, [elements.<id>]".into());
            return Err(problems);
        };
        let mut ids: Vec<&String> = table.keys().collect();
        ids.sort_by_key(|id| order.iter().position(|o| o == *id).unwrap_or(usize::MAX));
        for id in ids {
            match table[id].as_table() {
                Some(t) => match parse_element(id, t) {
                    Ok(elem) => elements.push(elem),
                    Err(errs) => {
                        for e in errs {
                            problem(format!("element `{id}`: {e}"));
                        }
                    }
                },
                None => problem(format!("element `{id}` is not a table")),
            }
        }
    }

    // A list's items become its buttons, in the element order right after
    // the list, so they stack and draw ahead of children written later
    let elements_written = std::mem::take(&mut elements);
    for elem in elements_written {
        let items = (elem.element_type == ElementType::List).then(|| list_items(&elem));
        elements.push(elem);
        elements.extend(items.into_iter().flatten());
    }

    for e in link(&mut elements) {
        problem(e);
    }

    if problems.is_empty() {
        Ok(Layout { elements, style })
    } else {
        Err(problems)
    }
}

/// The buttons a list makes for its items.
pub fn list_items(list: &UiElement) -> Vec<UiElement> {
    list.items
        .iter()
        .enumerate()
        .map(|(i, text)| {
            let mut item = UiElement::new(item_id(&list.id, i), ElementType::Button);
            item.class = list.item_class.clone();
            item.text = text.clone();
            item.parent_id = Some(list.id.clone());
            item.made_by_list = true;
            item
        })
        .collect()
}

/// Rebuild every element's `children` from the parents, in element order,
/// and say what doesn't hold: a parent or `nav_*` no element has, or a
/// parent cycle.
pub fn link(elements: &mut [UiElement]) -> Vec<String> {
    let mut problems = Vec::new();
    let index: HashMap<String, usize> = elements
        .iter()
        .enumerate()
        .map(|(i, e)| (e.id.clone(), i))
        .collect();
    for e in elements.iter_mut() {
        e.children.clear();
    }
    for i in 0..elements.len() {
        if let Some(pid) = elements[i].parent_id.clone() {
            match index.get(&pid) {
                Some(&p) if p == i => {
                    problems.push(format!("element `{pid}` is its own parent"));
                }
                Some(&p) => {
                    let child = elements[i].id.clone();
                    elements[p].children.push(child);
                }
                None => problems.push(format!(
                    "element `{}`: parent `{pid}` is no element in this layout",
                    elements[i].id
                )),
            }
        }
        for (dir, target) in &elements[i].nav {
            if !index.contains_key(target) {
                problems.push(format!(
                    "element `{}`: {} `{target}` is no element in this layout",
                    elements[i].id,
                    dir.key()
                ));
            }
        }
    }
    // A cycle never reaches a root
    for start in 0..elements.len() {
        let mut at = start;
        for _ in 0..=elements.len() {
            match elements[at].parent_id.as_ref().and_then(|p| index.get(p)) {
                Some(&p) => at = p,
                None => break,
            }
            if at == start {
                problems.push(format!(
                    "element `{}`: its parents go round in a circle",
                    elements[start].id
                ));
                break;
            }
        }
    }
    problems
}

/// The ids under `[elements]` in the order the file writes them, header
/// tables (`[elements.title]`) and inline ones (`title = { .. }`) alike.
/// `toml::Table` sorts its keys, so the order comes from `toml_edit`.
fn element_order(content: &str) -> Vec<String> {
    let Ok(doc) = content.parse::<toml_edit::DocumentMut>() else {
        return Vec::new();
    };
    let Some(elements) = doc.get("elements").and_then(|e| e.as_table_like()) else {
        return Vec::new();
    };
    let mut ids: Vec<(usize, usize, String)> = elements
        .iter()
        .enumerate()
        .map(|(i, (id, item))| {
            let at = item
                .as_table()
                .and_then(|t| t.position())
                .unwrap_or(usize::MAX);
            (at, i, id.to_string())
        })
        .collect();
    // A header table's place in the document, else where its key sits
    ids.sort_by_key(|(at, i, _)| (*at, *i));
    ids.into_iter().map(|(_, _, id)| id).collect()
}

fn parse_element(id: &str, t: &toml::Table) -> Result<UiElement, Vec<String>> {
    let mut problems = Vec::new();
    let element_type = match t.get("type") {
        None => ElementType::Panel,
        Some(toml::Value::String(s)) => match ElementType::from_str(s) {
            Some(ty) => ty,
            None => {
                problems.push(format!(
                    "unknown type `{s}` (types: {})",
                    ElementType::NAMES.join(", ")
                ));
                ElementType::Panel
            }
        },
        Some(other) => {
            problems.push(format!("`type` is a string, not {other}"));
            ElementType::Panel
        }
    };
    let type_name = ElementType::NAMES
        .iter()
        .find(|n| ElementType::from_str(n) == Some(element_type))
        .copied()
        .unwrap_or("panel");
    let mut elem = UiElement::new(id.to_string(), element_type);

    for (key, value) in t {
        let Some((_, types)) = ELEMENT_KEYS.iter().find(|(k, _)| k == key) else {
            let keys: Vec<&str> = ELEMENT_KEYS.iter().map(|(k, _)| *k).collect();
            problems.push(format!("unknown key `{key}` (keys: {})", keys.join(", ")));
            continue;
        };
        if let Some(types) = types {
            if !types.contains(&element_type) {
                problems.push(format!("`{key}` is not a {type_name}'s key"));
                continue;
            }
        }
        let wrong = |want: &str| format!("`{key}` is {want}, not {value}");
        match key.as_str() {
            "type" => {}
            "anchor" => match value.as_str() {
                Some(s) => match Anchor::from_str(s) {
                    Some(a) => {
                        elem.anchor = a;
                        elem.anchored = true;
                    }
                    None => problems.push(format!(
                        "unknown anchor `{s}` (anchors: {})",
                        Anchor::NAMES.join(", ")
                    )),
                },
                None => problems.push(wrong("a string")),
            },
            "parent" | "class" | "text" | "src" | "item_class" | "nav_up" | "nav_down"
            | "nav_left" | "nav_right" => match value.as_str() {
                Some(s) => {
                    let s = s.to_string();
                    match key.as_str() {
                        "parent" => elem.parent_id = Some(s),
                        "class" => elem.class = s,
                        "text" => elem.text = s,
                        "src" => elem.src = s,
                        "item_class" => elem.item_class = s,
                        nav => {
                            let dir = Dir::ALL.into_iter().find(|d| d.key() == nav).unwrap();
                            elem.nav.insert(dir, s);
                        }
                    }
                }
                None => problems.push(wrong("a string")),
            },
            "visible" | "focusable" | "enabled" | "autofocus" | "wrap" => match value.as_bool() {
                Some(b) => match key.as_str() {
                    "visible" => elem.visible = b,
                    "focusable" => elem.focusable = b,
                    "enabled" => elem.enabled = b,
                    "autofocus" => elem.autofocus = b,
                    _ => elem.wrap = b,
                },
                None => problems.push(wrong("true or false")),
            },
            "value" | "min" | "max" | "step" => match flint_core::toml_util::toml_f32(value) {
                Some(v) => match key.as_str() {
                    "value" => elem.value = v,
                    "min" => elem.min = v,
                    "max" => elem.max = v,
                    _ => elem.step = v,
                },
                None => problems.push(wrong("a number")),
            },
            "items" => match value.as_array() {
                Some(items) => {
                    for item in items {
                        match item.as_str() {
                            Some(s) => elem.items.push(s.to_string()),
                            None => problems.push(format!("`items` are strings, not {item}")),
                        }
                    }
                }
                None => problems.push(wrong("an array of strings")),
            },
            _ => unreachable!("every ELEMENT_KEYS key has an arm"),
        }
    }
    if element_type == ElementType::Slider && elem.max <= elem.min {
        problems.push(format!(
            "`max` ({}) is not above `min` ({})",
            elem.max, elem.min
        ));
    }
    if problems.is_empty() {
        Ok(elem)
    } else {
        Err(problems)
    }
}

/// A parsed `.style.toml`: named classes plus the token tables that
/// were used to resolve `"$name"` references inside them.
pub struct StyleSheet {
    pub classes: HashMap<String, StyleClass>,
    pub tokens: TokenSet,
}

/// Parse a .style.toml file into named style classes.
///
/// `root_dir` is the project root, where a top-level `tokens = "ui/theme.toml"`
/// resolves. String property values starting with `$` are looked up in the
/// local `[tokens]` table and the shared file; unknown tokens warn once and
/// leave the property unset. A class's sub-table named for a state
/// (`[styles.button.focus]`, `pressed`, `disabled`) goes over it in that
/// state. An unknown property, or a value of the wrong shape for one, is a
/// problem naming the file and the class.
pub fn load_styles(path: &Path, root_dir: &Path) -> Result<StyleSheet, Vec<String>> {
    let shown = path.display().to_string();
    let content = vfs::read_to_string(path).map_err(|e| vec![format!("{shown}: {e}")])?;
    let table: toml::Table = content
        .parse()
        .map_err(|e| vec![format!("{shown}: not TOML: {e}")])?;

    let style_dir = path.parent().unwrap_or(root_dir);
    let mut tokens = TokenSet::from_style_table(&table, root_dir, style_dir);
    let mut problems = Vec::new();
    let mut classes = HashMap::new();

    let styles_table = match table.get("styles") {
        Some(toml::Value::Table(t)) => t.clone(),
        Some(_) => {
            return Err(vec![format!(
                "{shown}: `styles` is a table of classes, [styles.<name>]"
            )])
        }
        None => toml::Table::new(),
    };

    for (name, value) in &styles_table {
        let Some(props_table) = value.as_table() else {
            problems.push(format!("{shown}: class `{name}` is not a table"));
            continue;
        };
        let mut class = StyleClass {
            name: name.clone(),
            properties: HashMap::new(),
            states: HashMap::new(),
        };
        for (key, val) in props_table {
            if let (Some(state), Some(state_table)) = (UiState::from_name(key), val.as_table()) {
                let mut props = HashMap::new();
                for (k, v) in state_table {
                    let at = format!("class `{name}`, {key}");
                    if let Some(sv) = style_value(&mut tokens, k, v, &at, &shown, &mut problems) {
                        props.insert(k.clone(), sv);
                    }
                }
                class.states.insert(state, props);
                continue;
            }
            let at = format!("class `{name}`");
            if let Some(sv) = style_value(&mut tokens, key, val, &at, &shown, &mut problems) {
                class.properties.insert(key.clone(), sv);
            }
        }
        classes.insert(name.clone(), class);
    }

    if problems.is_empty() {
        Ok(StyleSheet { classes, tokens })
    } else {
        Err(problems)
    }
}

/// One property's value, or a problem: a token resolves (an unknown one
/// warns and leaves the property unset, as it always has), anything else is
/// held to the property's shape.
fn style_value(
    tokens: &mut TokenSet,
    key: &str,
    val: &toml::Value,
    at: &str,
    shown: &str,
    problems: &mut Vec<String>,
) -> Option<StyleValue> {
    let resolved = match val {
        toml::Value::String(s) if TokenSet::is_reference(s) => {
            if !super::style::is_known_property(key) {
                None
            } else {
                return tokens.resolve_style(key, s);
            }
        }
        other => toml_to_style_value(other),
    };
    let problem = match &resolved {
        Some(sv) => property_problem(key, sv),
        None if !super::style::is_known_property(key) => {
            property_problem(key, &StyleValue::Bool(false))
        }
        None => Some(format!("`{key}` can't be {val}")),
    };
    match problem {
        Some(p) => {
            problems.push(format!("{shown}: {at}: {p}"));
            None
        }
        None => resolved,
    }
}
