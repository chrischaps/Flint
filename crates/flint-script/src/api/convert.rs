//! Script values and TOML values, both ways: what a component field,
//! a data file, a saved value or an event's data becomes in a script, and
//! back. Shared by the modules that read or write TOML (`entity`, `style`,
//! `persistence`, `data`, `event`, `living`, `wooze`).

use rhai::{Dynamic, Map};

/// A TOML value as a script value: integers stay ints, floats stay floats,
/// tables become maps and a datetime its text.
pub fn toml_to_dynamic(val: toml::Value) -> Dynamic {
    match val {
        toml::Value::Boolean(b) => Dynamic::from(b),
        toml::Value::Integer(i) => Dynamic::from(i),
        toml::Value::Float(f) => Dynamic::from(f),
        toml::Value::String(s) => Dynamic::from(s),
        toml::Value::Array(arr) => {
            let items: Vec<Dynamic> = arr.into_iter().map(toml_to_dynamic).collect();
            Dynamic::from(items)
        }
        toml::Value::Table(table) => {
            let mut map = Map::new();
            for (k, v) in table {
                map.insert(k.into(), toml_to_dynamic(v));
            }
            Dynamic::from(map)
        }
        toml::Value::Datetime(dt) => Dynamic::from(dt.to_string()),
    }
}

/// A script value as a TOML value; `None` for what TOML can't hold (`()`,
/// a function pointer). An array drops such items, a map such keys.
pub fn dynamic_to_toml(val: &Dynamic) -> Option<toml::Value> {
    if val.is_bool() {
        return val.as_bool().ok().map(toml::Value::Boolean);
    }
    if val.is_int() {
        return val.as_int().ok().map(toml::Value::Integer);
    }
    if val.is_float() {
        return val.as_float().ok().map(toml::Value::Float);
    }
    if val.is_string() {
        return val.clone().into_string().ok().map(toml::Value::String);
    }
    if val.is_array() {
        let arr = val.clone().into_array().ok()?;
        let items: Vec<toml::Value> = arr.iter().filter_map(dynamic_to_toml).collect();
        return Some(toml::Value::Array(items));
    }
    if val.is_map() {
        return Some(match val.read_lock::<Map>() {
            Some(map) => map_to_toml(&map),
            None => toml::Value::String(val.to_string()),
        });
    }
    None
}

/// A script map as a TOML table, dropping the keys TOML can't hold.
pub(crate) fn map_to_toml(map: &Map) -> toml::Value {
    let mut table = toml::map::Map::new();
    for (k, v) in map {
        if let Some(tv) = dynamic_to_toml(v) {
            table.insert(k.to_string(), tv);
        }
    }
    toml::Value::Table(table)
}

#[cfg(test)]
mod tests {
    use super::*;
    use rhai::Array;

    #[test]
    fn toml_scalars_become_their_script_types() {
        assert!(toml_to_dynamic(toml::Value::Boolean(true))
            .as_bool()
            .unwrap());
        assert_eq!(
            toml_to_dynamic(toml::Value::Integer(42)).as_int().unwrap(),
            42
        );
        let f = toml_to_dynamic(toml::Value::Float(3.25));
        assert!(f.is_float());
        assert!((f.as_float().unwrap() - 3.25).abs() < 1e-10);
        assert_eq!(
            toml_to_dynamic(toml::Value::String("hello".into()))
                .into_string()
                .unwrap(),
            "hello"
        );
    }

    #[test]
    fn toml_arrays_and_tables_become_arrays_and_maps() {
        let arr = toml::Value::Array(vec![
            toml::Value::Integer(1),
            toml::Value::Integer(2),
            toml::Value::Integer(3),
        ]);
        let items = toml_to_dynamic(arr).into_array().unwrap();
        assert_eq!(items.len(), 3);
        assert_eq!(items[0].as_int().unwrap(), 1);
        let table: toml::Value = "n = 1\n[inner]\nk = \"v\"\n".parse().unwrap();
        let map = toml_to_dynamic(table).cast::<Map>();
        assert_eq!(map["n"].as_int().unwrap(), 1);
        assert_eq!(
            map["inner"].clone().cast::<Map>()["k"]
                .clone()
                .into_string()
                .unwrap(),
            "v"
        );
    }

    #[test]
    fn script_values_round_trip_through_toml() {
        for v in [
            toml::Value::Float(2.75),
            toml::Value::Integer(-3),
            toml::Value::Boolean(false),
            toml::Value::String("test".into()),
            "a = [1, 2.5, \"x\"]\n[b]\nc = true\n"
                .parse::<toml::Value>()
                .unwrap(),
        ] {
            assert_eq!(dynamic_to_toml(&toml_to_dynamic(v.clone())), Some(v));
        }
    }

    #[test]
    fn what_toml_cannot_hold_is_dropped() {
        assert_eq!(dynamic_to_toml(&Dynamic::UNIT), None);
        let arr: Array = vec![Dynamic::from(1_i64), Dynamic::UNIT];
        assert_eq!(
            dynamic_to_toml(&Dynamic::from(arr)),
            Some(toml::Value::Array(vec![toml::Value::Integer(1)]))
        );
        let mut map = Map::new();
        map.insert("keep".into(), Dynamic::from(1.5_f64));
        map.insert("drop".into(), Dynamic::UNIT);
        let table = map_to_toml(&map);
        assert_eq!(table.as_table().unwrap().len(), 1);
        assert_eq!(table["keep"].as_float(), Some(1.5));
    }
}
