//! The `persistence` module: the engine-managed save data, a key-value
//! store in slots, versioned for migration (#726), written atomically by
//! the player 1 s after the last change, on scene transitions and on exit.
//! ADR 0116.
//!
//! The store is the one the host lends for the call (`StateScope`).
//! Without one (a host with no saves) every read is empty and every write
//! does nothing.

use super::builder::kind::{Any, Array, Bool, Int, Map as MapKind, Str};
use super::builder::ScriptApi;
use super::convert::{dynamic_to_toml, toml_to_dynamic};
use rhai::{Dynamic, Map};

pub(crate) fn register(api: &mut ScriptApi) {
    let mut m = api.module("persistence");

    m.func(
        "persist_set",
        "Save a value under a key: a bool, a number, a string, or an array or map of them.",
    )
    .arg("key", Str)
    .arg("value", Any)
    .run(|call, (key, value)| {
        if let (Some(store), Some(v)) = (
            call.ctx().try_persistent_store_mut(),
            dynamic_to_toml(&value),
        ) {
            store.set(&key, v);
        }
    });

    m.func("persist_get", "The value saved under a key.")
        .arg("key", Str)
        .returns(Any, "the value as it was saved; () for no such key")
        .run(|call, (key,)| {
            call.ctx_ref()
                .try_persistent_store()
                .and_then(|s| s.get(&key).cloned())
                .map_or(Dynamic::UNIT, toml_to_dynamic)
        });

    m.func("persist_has", "Whether a value is saved under a key.")
        .arg("key", Str)
        .returns(Bool, "")
        .run(|call, (key,)| {
            call.ctx_ref()
                .try_persistent_store()
                .is_some_and(|s| s.has(&key))
        });

    m.func("persist_remove", "Forget the value saved under a key.")
        .arg("key", Str)
        .run(|call, (key,)| {
            if let Some(store) = call.ctx().try_persistent_store_mut() {
                store.remove(&key);
            }
        });

    m.func(
        "persist_clear",
        "Forget every saved value in the slot held.",
    )
    .run(|call, ()| {
        if let Some(store) = call.ctx().try_persistent_store_mut() {
            store.clear();
        }
    });

    m.func("persist_keys", "Every key with a saved value.")
        .returns(Array, "the keys, as strings")
        .run(|call, ()| -> rhai::Array {
            call.ctx_ref()
                .try_persistent_store()
                .map(|s| {
                    s.keys()
                        .into_iter()
                        .map(|k| Dynamic::from(k.to_string()))
                        .collect()
                })
                .unwrap_or_default()
        });

    m.func(
        "persist_save",
        "Write the save file now, not waiting for the player's own write.",
    )
    .run(|call, ()| {
        if let Some(store) = call.ctx().try_persistent_store_mut() {
            store.request_flush();
        }
    });

    m.func(
        "persist_save",
        "Write the saved values to a file at this path, at once.",
    )
    .arg("path", Str)
    .run(|call, (path,)| {
        if let Some(store) = call.ctx_ref().try_persistent_store() {
            if let Err(e) = store.save_to_file(std::path::Path::new(path.as_str())) {
                tracing::warn!("save error: {}", e);
            }
        }
    });

    m.func(
        "persist_load",
        "Read saved values from a file at this path into the slot held.",
    )
    .arg("path", Str)
    .run(|call, (path,)| {
        if let Some(store) = call.ctx().try_persistent_store_mut() {
            if let Err(e) = store.load_from_file(std::path::Path::new(path.as_str())) {
                tracing::warn!("load error: {}", e);
            }
        }
    });

    m.func("persist_slot", "The save slot held.")
        .returns(Str, "its name; \"default\" to begin with")
        .run(|call, ()| {
            call.ctx_ref()
                .try_persistent_store()
                .map_or(flint_runtime::DEFAULT_SLOT, |s| s.slot())
                .to_string()
        });

    m.func(
        "persist_slot",
        "Hold another save slot: the one held is saved first if it changed, then this one loaded or begun empty.",
    )
    .arg("name", Str)
    .returns(
        Bool,
        "false, with a warning, for a bad name, a failed save or no save location; the slot held is then unchanged",
    )
    .run(|call, (name,)| {
        let Some(store) = call.ctx().try_persistent_store_mut() else {
            return false;
        };
        match store.open_slot(&name) {
            Ok(loaded) if loaded.is_error() => {
                tracing::error!("persist: {loaded}");
                true
            }
            Ok(loaded) => {
                tracing::info!("persist: slot '{name}': {loaded}");
                true
            }
            Err(e) => {
                tracing::warn!("persist_slot: {e}");
                false
            }
        }
    });

    m.func("persist_slots", "The slots that have a save.")
        .returns(Array, "their names, in name order")
        .run(|call, ()| -> rhai::Array {
            call.ctx_ref()
                .try_persistent_store()
                .map(|s| s.slots().into_iter().map(Dynamic::from).collect())
                .unwrap_or_default()
        });

    m.func("persist_version", "The version of the save data held.")
        .returns(Int, "0 for a file from before versions")
        .run(|call, ()| {
            call.ctx_ref()
                .try_persistent_store()
                .map_or(0, |s| s.version())
        });

    m.func(
        "persist_set_version",
        "Stamp the save data held with a version, for a game that migrates by hand.",
    )
    .arg("version", Int)
    .run(|call, (version,)| {
        let Some(store) = call.ctx().try_persistent_store_mut() else {
            return;
        };
        if version < 0 {
            tracing::warn!("persist_set_version({version}): a version is 0 or more");
            return;
        }
        store.set_version(version);
    });

    m.func(
        "persist_corrupt",
        "What was wrong with the slot held's file, when it didn't load.",
    )
    .returns(
        MapKind,
        "#{slot, file, backup, error}; () when it loaded cleanly or had no file",
    )
    .run(|call, ()| {
        match call
            .ctx_ref()
            .try_persistent_store()
            .and_then(|s| s.corrupt())
        {
            None => Dynamic::UNIT,
            Some(corrupt) => {
                let mut map = Map::new();
                map.insert("slot".into(), corrupt.slot.clone().into());
                map.insert("file".into(), corrupt.file.clone().into());
                map.insert("backup".into(), corrupt.backup.clone().into());
                map.insert("error".into(), corrupt.error.clone().into());
                Dynamic::from_map(map)
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use crate::context::StateScope;
    use crate::engine::ScriptEngine;
    use flint_physics::PhysicsSystem;
    use flint_runtime::{GameStateMachine, PersistentStore};

    #[test]
    fn a_script_saves_reads_and_forgets_values() {
        let engine = ScriptEngine::new();
        let mut sm = GameStateMachine::new();
        let mut store = PersistentStore::new();
        let physics = PhysicsSystem::new();
        {
            let _scope = StateScope::new(&engine.ctx, &mut sm, &mut store, &physics);
            let rhai = engine.rhai_engine();
            let got: rhai::Map = rhai
                .eval(
                    "persist_set(\"best\", 12); persist_set(\"name\", \"ada\"); \
                     persist_set(\"gone\", 1.5); persist_remove(\"gone\"); \
                     persist_set_version(3); persist_set_version(-1); \
                     #{ best: persist_get(\"best\"), has: persist_has(\"name\"), \
                        gone: persist_get(\"gone\"), keys: persist_keys().len(), \
                        version: persist_version(), slot: persist_slot() }",
                )
                .unwrap();
            assert_eq!(got["best"].as_int().unwrap(), 12, "an int stays an int");
            assert!(got["has"].as_bool().unwrap());
            assert!(got["gone"].is_unit());
            assert_eq!(got["keys"].as_int().unwrap(), 2);
            assert_eq!(got["version"].as_int().unwrap(), 3, "-1 was refused");
            assert_eq!(got["slot"].clone().into_string().unwrap(), "default");
            // A whole float is a version
            let _: () = rhai.eval("persist_set_version(4.0)").unwrap();
        }
        assert_eq!(store.version(), 4);
        assert_eq!(store.get("name").and_then(|v| v.as_str()), Some("ada"));
    }

    #[test]
    fn without_a_store_reads_are_empty_and_writes_do_nothing() {
        let engine = ScriptEngine::new();
        let got: rhai::Map = engine
            .rhai_engine()
            .eval(
                "persist_set(\"k\", 1); persist_save(); \
                 #{ v: persist_get(\"k\"), has: persist_has(\"k\"), slot: persist_slot(), \
                    open: persist_slot(\"two\"), version: persist_version(), \
                    corrupt: persist_corrupt(), slots: persist_slots() }",
            )
            .unwrap();
        assert!(got["v"].is_unit());
        assert!(!got["has"].as_bool().unwrap());
        assert_eq!(got["slot"].clone().into_string().unwrap(), "default");
        assert!(!got["open"].as_bool().unwrap());
        assert_eq!(got["version"].as_int().unwrap(), 0);
        assert!(got["corrupt"].is_unit());
        assert!(got["slots"].clone().into_array().unwrap().is_empty());
    }
}
