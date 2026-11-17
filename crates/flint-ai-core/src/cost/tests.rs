use std::sync::{Arc, Mutex};

use serde_json::Value as Json;

use super::*;

fn usage(i: u64, o: u64) -> Usage {
    Usage {
        input_tokens: i,
        output_tokens: o,
        ..Usage::default()
    }
}

fn entry(ts: &str, provider: &str, model: &str, spec: &str, usd: f64) -> CostEntry {
    CostEntry {
        timestamp: ts.into(),
        provider: provider.into(),
        model: model.into(),
        input_tokens: 1000,
        output_tokens: 500,
        cache_read_input_tokens: 0,
        cache_creation_input_tokens: 0,
        estimated_usd: usd,
        spec_name: spec.into(),
        operation: "create".into(),
        priced_as: None,
        units: None,
        unit: None,
    }
}

fn tmp(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "flint-ai-cost-{name}-{}-{:?}",
        std::process::id(),
        std::thread::current().id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    dir
}

fn quiet(meter: CostMeter) -> (CostMeter, Arc<Mutex<Vec<String>>>) {
    let heard = Arc::new(Mutex::new(Vec::new()));
    let h = heard.clone();
    (
        meter.with_notify(move |m| h.lock().unwrap().push(m.to_string())),
        heard,
    )
}

#[test]
fn the_builtin_table_has_every_current_model_at_its_published_rate() {
    let t = PriceTable::builtin();
    assert_eq!(t.checked, "2026-11-16");
    // The ids the issue found missing or wrong (#721).
    for (model, input, output, cache_read) in [
        ("claude-fable-5-2", 8.0, 40.0, 0.20),
        ("claude-mythos-5-2", 8.0, 40.0, 0.20),
        ("claude-opus-5-5", 4.0, 20.0, 0.20),
        ("claude-sonnet-5-5", 2.0, 10.0, 0.10),
        ("claude-haiku-5-5", 0.10, 0.50, 0.01),
    ] {
        let (p, as_) = t.token_price("anthropic", model);
        assert_eq!(as_, None, "{model}");
        assert_eq!(
            (p.input, p.output, p.cache_read),
            (input, output, cache_read)
        );
        assert!((p.cache_write - input * 1.25).abs() < 1e-12, "{model}");
    }
    // 1M in + 1M out on Opus 5.5 = $4 + $20.
    let (p, _) = t.token_price("anthropic", "claude-opus-5-5");
    assert!((p.usd(&usage(1_000_000, 1_000_000)) - 24.0).abs() < 1e-9);
    assert_eq!(t.token_price("mock", "mock").0, Price::FREE);
    for provider in ["flux", "meshy", "elevenlabs"] {
        let c = t.call_price(provider).unwrap();
        assert!(c.usd > 0.0 && c.source.is_some(), "{provider}");
    }
}

#[test]
fn an_unknown_model_is_priced_as_the_dearest() {
    let t = PriceTable::builtin();
    let dearest = t.models.values().map(|p| p.output).fold(0.0_f64, f64::max);
    let (p, as_) = t.token_price("anthropic", "claude-new-thing");
    assert_eq!(as_.as_deref(), Some("claude-fable-5-1"));
    assert_eq!(p.output, dearest);
    // A suffixed id finds its family, not a shorter neighbour.
    assert_eq!(
        t.token_price("anthropic", "claude-opus-5-5-preview")
            .1
            .as_deref(),
        Some("claude-opus-5-5")
    );
    assert_eq!(price_for("anthropic", "claude-opus-5-5").1, None);
}

#[test]
fn a_project_file_overrides_entries_and_keeps_the_rest() {
    let dir = tmp("prices");
    std::fs::create_dir_all(dir.join(".flint")).unwrap();
    assert_eq!(
        *PriceTable::for_project(&dir).unwrap(),
        *PriceTable::builtin()
    );
    std::fs::write(
        dir.join(PRICES_PATH),
        "checked = \"2026-12-01\"\n\
         [tokens.models]\n\"claude-opus-5-5\" = { input = 1.0, output = 2.0, cache_read = 0.1 }\n\
         [calls.meshy]\nunit = \"task\"\nusd = 0.45\n",
    )
    .unwrap();
    let t = PriceTable::for_project(&dir).unwrap();
    assert_eq!(t.checked, "2026-12-01");
    assert_eq!(t.token_price("anthropic", "claude-opus-5-5").0.output, 2.0);
    assert_eq!(
        t.token_price("anthropic", "claude-sonnet-5-5").0.output,
        10.0
    );
    assert_eq!(t.call_price("meshy").unwrap().usd, 0.45);
    assert_eq!(t.call_price("flux").unwrap().usd, 0.025);
    assert_eq!(t.overrides, vec![dir.join(PRICES_PATH)]);

    for bad in [
        "[calls.flux]\nunit = \"megapixel\"\nusd = -1.0\n",
        "[tokens]\nfallback = \"nope\"\n",
        "[calls.flux]\nunit = \"megapixel\"\nusd = 1.0\nextra = 2\n",
        "not toml",
    ] {
        std::fs::write(dir.join(PRICES_PATH), bad).unwrap();
        let e = PriceTable::for_project(&dir).unwrap_err();
        assert!(matches!(e, CostError::Prices(_)), "{bad}: {e}");
        assert!(e.to_string().contains("prices.toml"), "{e}");
    }
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn timestamps_are_rfc3339_utc() {
    assert_eq!(rfc3339(0), "1970-01-01T00:00:00Z");
    assert_eq!(rfc3339(951_782_400), "2000-02-29T00:00:00Z");
    assert_eq!(rfc3339(1_793_458_509), "2026-10-31T14:55:09Z");
}

#[test]
fn the_ledger_appends_lines_and_reads_them_back_with_the_legacy_file() {
    let dir = tmp("ledger");
    let ledger = CostLedger::in_project(&dir);
    assert!(ledger.read().unwrap().entries.is_empty());
    let old = entry(
        "2026-09-01T00:00:00Z",
        "anthropic",
        "claude-opus-5-5",
        "pine",
        0.03,
    );
    std::fs::create_dir_all(dir.join(".flint")).unwrap();
    std::fs::write(
        dir.join(LEGACY_LEDGER_PATH),
        format!("{}\n", serde_json::to_string(&old).unwrap()),
    )
    .unwrap();
    let a = entry(
        "2026-10-01T00:00:00Z",
        "anthropic",
        "claude-opus-5-5",
        "oak",
        0.01,
    );
    let mut b = entry(
        "2026-10-02T00:00:00Z",
        "flux",
        "fal-ai/flux/dev",
        "brick",
        0.05,
    );
    b.units = Some(2.0);
    b.unit = Some("megapixel".into());
    ledger.append(&a).unwrap();
    ledger.append(&b).unwrap();
    assert_eq!(ledger.path(), dir.join(LEDGER_PATH));
    let text = std::fs::read_to_string(ledger.path()).unwrap();
    assert_eq!(text.lines().count(), 2);
    for line in text.lines() {
        let v: Json = serde_json::from_str(line).unwrap();
        for k in [
            "timestamp",
            "provider",
            "model",
            "estimated_usd",
            "spec_name",
        ] {
            assert!(v.get(k).is_some(), "{k} missing from {line}");
        }
    }
    assert!(text.contains("\"unit\":\"megapixel\""), "{text}");
    // A damaged line is skipped, not fatal.
    std::fs::write(ledger.path(), format!("{text}not json\n")).unwrap();
    let back = ledger.read().unwrap();
    assert_eq!(back.entries, vec![old, a, b]);
    assert_eq!(back.skipped.len(), 1);
    assert!(back.skipped[0].ends_with(":3"), "{:?}", back.skipped);
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn the_summary_adds_up() {
    let entries = vec![
        entry(
            "2026-09-30T23:59:59Z",
            "anthropic",
            "claude-opus-5-5",
            "oak",
            0.10,
        ),
        entry(
            "2026-10-01T00:00:00Z",
            "anthropic",
            "claude-opus-5-5",
            "oak",
            0.20,
        ),
        entry(
            "2026-10-02T00:00:00Z",
            "anthropic",
            "claude-sonnet-5-5",
            "elm",
            0.05,
        ),
        entry("2026-10-03T00:00:00Z", "mock", "mock", "elm", 0.0),
    ];
    let all = CostSummary::from_entries(&entries, None);
    assert_eq!(all.total.calls, 4);
    assert!((all.total.usd - 0.35).abs() < 1e-9);
    assert!((all.by_spec["oak"].usd - 0.30).abs() < 1e-9);
    assert_eq!(all.by_provider["anthropic"].calls, 3);
    assert_eq!(all.total.input_tokens, 4000);

    let since = parse_since("2026-10-01").unwrap();
    let recent = CostSummary::from_entries(&entries, Some(&since));
    assert_eq!(recent.total.calls, 3);
    assert!((recent.total.usd - 0.25).abs() < 1e-9);
    let text = recent.render();
    assert!(text.contains("since 2026-10-01"), "{text}");
    assert!(text.contains("$0.2500"), "{text}");
    assert!(text.contains("By spec or asset"), "{text}");

    assert!(CostSummary::from_entries(&[], None)
        .render()
        .contains("no AI calls"));
}

#[test]
fn since_must_be_a_date() {
    assert!(parse_since("2024-01-01").is_ok());
    assert!(parse_since("2024-01-01T12:00:00Z").is_ok());
    for bad in ["yesterday", "2024-13-01", "2024-1-1", "2024-01-01 12:00"] {
        assert!(parse_since(bad).is_err(), "{bad}");
    }
}

#[test]
fn the_ceiling_admits_caps_and_refuses() {
    let (meter, heard) = quiet(CostMeter::new("anthropic").with_ceiling(0.50));
    // Opus 5.5: 10K in ($0.04) + 16K out ($0.32) fits $0.50.
    assert_eq!(
        meter.admit("claude-opus-5-5", 10_000, 16_000).unwrap(),
        16_000
    );
    meter.record("claude-opus-5-5", &usage(10_000, 10_000)); // $0.24
                                                             // $0.26 left: 10K in ($0.04) leaves $0.22 = 11,000 output tokens.
    let capped = meter.admit("claude-opus-5-5", 10_000, 16_000).unwrap();
    assert!((10_990..=11_000).contains(&capped), "{capped}");
    assert!(heard.lock().unwrap().iter().any(|m| m.contains("capping")));
    meter.record("claude-opus-5-5", &usage(10_000, 9_000)); // $0.22 -> $0.46
    assert!(
        heard.lock().unwrap().iter().any(|m| m.contains("92%")),
        "{:?}",
        heard.lock().unwrap()
    );
    // $0.04 left can't pay for 10K in + 2K out.
    let err = meter.admit("claude-opus-5-5", 10_000, 16_000).unwrap_err();
    assert!(matches!(err, CostError::Ceiling { .. }));
    let msg = err.to_string();
    assert!(msg.contains("$0.50") && msg.contains("$0.46"), "{msg}");
    assert_eq!(meter.calls(), 2);
    assert_eq!(meter.refused(), 1);
}

#[test]
fn no_ceiling_admits_everything_and_mock_is_free() {
    let meter = CostMeter::new("anthropic");
    assert_eq!(
        meter.admit("claude-fable-5-1", 1 << 30, 128_000).unwrap(),
        128_000
    );
    assert_eq!(meter.admit_call("unpriced", 9.0).unwrap(), 0.0);
    let mock = CostMeter::new("mock").with_ceiling(0.0);
    assert_eq!(mock.admit("mock", 1 << 30, 16_000).unwrap(), 16_000);
    assert_eq!(mock.record("mock", &usage(100, 100)), 0.0);
    assert_eq!(mock.admit_call("mock", 3.0).unwrap(), 0.0);
}

#[test]
fn calls_priced_per_unit_are_admitted_then_recorded() {
    // Flux at $0.025 a megapixel: two 2 MP images are $0.10; a third
    // passes a $0.12 ceiling and is refused before it's sent.
    let dir = tmp("calls");
    let ledger = CostLedger::in_project(&dir);
    let (meter, _) = quiet(
        CostMeter::new("anthropic")
            .with_ceiling(0.12)
            .with_ledger(ledger.clone()),
    );
    meter.set_context("brick_wall", "resolve");
    for _ in 0..2 {
        let est = meter.admit_call("flux", 2.0).unwrap();
        assert!((est - 0.05).abs() < 1e-12);
        assert!((meter.record_call("flux", "fal-ai/flux/dev", 2.0) - 0.05).abs() < 1e-12);
    }
    let e = meter.admit_call("flux", 2.0).unwrap_err();
    assert_eq!(
        e.to_string(),
        "cost ceiling reached: $0.1000 spent, and the next call could cost up to $0.0500, \
         past --max-cost $0.1200; nothing more was sent"
    );
    // A cheaper call still fits what's left.
    assert!(meter.admit_call("elevenlabs", 3.0).is_ok());
    // No price at all can't be kept under a ceiling.
    assert_eq!(
        meter.admit_call("someone", 1.0).unwrap_err(),
        CostError::Unpriced("someone".into())
    );
    let back = ledger.read().unwrap().entries;
    assert_eq!(back.len(), 2);
    assert_eq!(back[0].spec_name, "brick_wall");
    assert_eq!(back[0].operation, "resolve");
    assert_eq!(back[0].unit.as_deref(), Some("megapixel"));
    assert_eq!(back[0].units, Some(2.0));
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn record_writes_the_ledger_with_context() {
    let dir = tmp("meter");
    let ledger = CostLedger::new(dir.join("costs.jsonl"));
    let (meter, _) = quiet(CostMeter::new("anthropic").with_ledger(ledger.clone()));
    meter.set_context("twisted_dead_tree", "create");
    let cost = meter.record("claude-opus-5-5", &usage(1_000, 2_000));
    assert!((cost - (0.004 + 0.04)).abs() < 1e-9);
    let back = ledger.read().unwrap().entries;
    assert_eq!(back.len(), 1);
    assert_eq!(back[0].spec_name, "twisted_dead_tree");
    assert_eq!(back[0].operation, "create");
    assert_eq!(back[0].output_tokens, 2_000);
    assert!((back[0].estimated_usd - cost).abs() < 1e-12);
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn an_old_ledger_line_still_parses() {
    // What #36's ledger wrote, before units.
    let line = r#"{"timestamp":"2026-10-31T14:05:09Z","provider":"anthropic","model":"claude-opus-5-5","input_tokens":120,"output_tokens":400,"cache_read_input_tokens":3000,"estimated_usd":0.0366,"spec_name":"oak","operation":"create"}"#;
    let p = parse_ledger(line);
    assert_eq!(p.entries.len(), 1);
    assert_eq!(p.entries[0].units, None);
}
