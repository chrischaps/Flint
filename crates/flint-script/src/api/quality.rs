//! The `quality` module: the renderer's load levers (FlintKart#55) and
//! what the GPU is (ADR 0116). `set_quality(#{ msaa: 4, shadow_cascades:
//! 2 })` changes them from the next frame; `get_quality()` reads them back.
//! Keys and ranges are `flint_core::quality::QualitySettings`'.

use super::builder::kind::{Any, Bool, Map as MapKind, Str};
use super::builder::ScriptApi;
use crate::context::ScriptCallContext;
use flint_core::quality::{QualityError, QualityValue};
use rhai::{Dynamic, Map};

pub(crate) fn register(api: &mut ScriptApi) {
    let mut m = api.module("quality");

    m.func(
        "set_quality",
        "Change any of the quality settings from the next frame; a bad key is warned about and skipped.",
    )
    .arg("settings", MapKind)
    .returns(Bool, "true when every key was taken")
    .run(|call, (settings,)| request_quality(call.ctx(), &settings));

    m.func(
        "set_quality",
        "Change one quality setting (a bool, an int or a number) from the next frame.",
    )
    .arg("key", Str)
    .arg("value", Any)
    .returns(Bool, "true when the key and value were taken")
    .run(|call, (key, value)| {
        let mut settings = Map::new();
        settings.insert(key.as_str().into(), value);
        request_quality(call.ctx(), &settings)
    });

    m.func("get_quality", "Every quality setting as it stands.")
        .returns(MapKind, "#{msaa, shadow_cascades, ssao, render_scale, ...}")
        .run(|call, ()| -> Map {
            call.ctx_ref()
                .quality
                .entries()
                .into_iter()
                .map(|(k, v)| (k.into(), quality_dynamic(v)))
                .collect()
        });

    m.func("get_quality", "One quality setting as it stands.")
        .arg("key", Str)
        .returns(Any, "a bool, an int or a float; () for an unknown key")
        .run(|call, (key,)| {
            call.ctx_ref()
                .quality
                .get(&key)
                .map_or(Dynamic::UNIT, quality_dynamic)
        });

    m.func("gpu_info", "What the GPU is, as the host found it.")
        .returns(
            MapKind,
            "#{name, vendor, device_type, backend, driver, class, platform}, strings",
        )
        .run(|call, ()| {
            let g = &call.ctx_ref().gpu_info;
            let mut map = Map::new();
            for (k, v) in [
                ("name", &g.name),
                ("vendor", &g.vendor),
                ("device_type", &g.device_type),
                ("backend", &g.backend),
                ("driver", &g.driver),
                ("class", &g.class),
                ("platform", &g.platform),
            ] {
                map.insert(k.into(), v.clone().into());
            }
            map
        });
}

/// A script value as a quality value: bools, integers and floats.
fn quality_value(v: &Dynamic) -> Option<QualityValue> {
    if let Ok(b) = v.as_bool() {
        Some(QualityValue::Bool(b))
    } else if let Ok(i) = v.as_int() {
        Some(QualityValue::Int(i))
    } else if let Ok(f) = v.as_float() {
        Some(QualityValue::Float(f))
    } else {
        None
    }
}

/// A quality value as a script value.
fn quality_dynamic(v: QualityValue) -> Dynamic {
    match v {
        QualityValue::Bool(b) => b.into(),
        QualityValue::Int(i) => i.into(),
        QualityValue::Float(f) => f.into(),
    }
}

/// Merge `settings`' keys into `c.quality` and ask for it to be applied.
/// True when every key was taken; a bad key is warned about and skipped.
pub(crate) fn request_quality(c: &mut ScriptCallContext, settings: &Map) -> bool {
    let mut all_taken = true;
    let mut quality = c.quality;
    for (key, value) in settings {
        let result = match quality_value(value) {
            Some(v) => quality.set(key.as_str(), v),
            None => Err(QualityError::Malformed(format!("{key}={value}"))),
        };
        if let Err(e) = result {
            tracing::warn!("set_quality: {e}");
            all_taken = false;
        }
    }
    if quality != c.quality {
        c.quality = quality;
        c.quality_requested = true;
    }
    all_taken
}

#[cfg(test)]
mod tests {
    use crate::engine::ScriptEngine;

    #[test]
    fn a_script_sets_and_reads_quality() {
        let engine = ScriptEngine::new();
        let rhai = engine.rhai_engine();
        let ok: bool = rhai
            .eval(
                "set_quality(#{ msaa: 4, shadow_cascades: 2, ssao: false, render_scale: 0.75, \
                 shadow_distance: 90 })",
            )
            .unwrap();
        assert!(ok);
        {
            let c = engine.ctx.lock().unwrap();
            assert!(c.quality_requested);
            assert_eq!(c.quality.msaa, 4);
            assert_eq!(c.quality.shadow_cascades, 2);
            assert!(!c.quality.ssao);
            assert_eq!(c.quality.render_scale, 0.75);
            // An integer is fine where a float is wanted.
            assert_eq!(c.quality.shadow_distance, 90.0);
        }
        // Read back at once, clamped.
        let msaa: i64 = rhai.eval("get_quality().msaa").unwrap();
        assert_eq!(msaa, 4);
        let scale: f64 = rhai.eval("get_quality(\"render_scale\")").unwrap();
        assert_eq!(scale, 0.75);
        let keys: i64 = rhai.eval("get_quality().len()").unwrap();
        assert_eq!(
            keys as usize,
            flint_core::quality::QualitySettings::KEYS.len()
        );
        let unknown: bool = rhai.eval("get_quality(\"sharpness\") == ()").unwrap();
        assert!(unknown);
    }

    #[test]
    fn a_bad_key_is_skipped_and_the_rest_applied() {
        let engine = ScriptEngine::new();
        let ok: bool = engine
            .rhai_engine()
            .eval("set_quality(#{ bloom: false, sharpness: 3, msaa: \"lots\" })")
            .unwrap();
        assert!(!ok);
        let c = engine.ctx.lock().unwrap();
        assert!(!c.quality.bloom);
        assert_eq!(c.quality.msaa, 1);
    }

    #[test]
    fn setting_what_is_already_set_asks_for_nothing() {
        let engine = ScriptEngine::new();
        let rhai = engine.rhai_engine();
        let ok: bool = rhai.eval("set_quality(\"msaa\", 1)").unwrap();
        assert!(ok);
        assert!(!engine.ctx.lock().unwrap().quality_requested);
        let _: bool = rhai.eval("set_quality(\"shadows\", false)").unwrap();
        assert!(engine.ctx.lock().unwrap().quality_requested);
    }

    #[test]
    fn a_script_reads_the_gpu() {
        let engine = ScriptEngine::new();
        engine.ctx.lock().unwrap().gpu_info = flint_core::quality::GpuInfo {
            name: "Mali-G715".into(),
            class: "mobile".into(),
            platform: "android".into(),
            ..Default::default()
        };
        let class: String = engine.rhai_engine().eval("gpu_info().class").unwrap();
        assert_eq!(class, "mobile");
        let name: String = engine.rhai_engine().eval("gpu_info().name").unwrap();
        assert_eq!(name, "Mali-G715");
    }
}
