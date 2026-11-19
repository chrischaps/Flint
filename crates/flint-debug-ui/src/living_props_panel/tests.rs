//! The panel driven through headless egui frames, and its parts on their own.

use super::knobs::*;
use super::overlay::*;
use super::ui::*;
use super::*;
use crate::run_headless_frame;
use egui::{Pos2, Rect, Shape, Vec2};
use flint_procgen::WoozePreset;

fn row(name: &str, position: [f32; 3]) -> LivingPropRow {
    LivingPropRow {
        name: name.to_string(),
        species: "oak".to_string(),
        lineage_seed: 7,
        position,
        height: 10.0,
        reach: 3.0,
        beats: 3.5,
        step: 0.5,
        glide: 0.3,
        pose: 3,
        weight: 0.0,
        shape_seed: 99,
        ring_first: 2,
        ring_len: 8,
        ring_ready: 8,
        echoes: 6,
        waiting: false,
    }
}

fn state() -> LivingPanelState {
    LivingPanelState {
        rows: vec![row("oak_1", [0.0, 0.0, 0.0])],
        preset_names: vec!["linklater".to_string(), "misty".to_string()],
        ..Default::default()
    }
}

fn frame(ctx: &egui::Context, panel: &mut LivingPropsDebugPanel, events: Vec<egui::Event>) {
    let input = egui::RawInput {
        screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(420.0, 1400.0))),
        events,
        ..Default::default()
    };
    let _ = run_headless_frame(ctx, input, |ui| {
        egui::CentralPanel::default().show(ui, |ui| panel.ui(ui));
    });
}

#[test]
fn knobs_read_back_through_a_preset() {
    let knobs = LivingKnobs {
        step: 1.5,
        glide: 0.75,
        amplitude: 0.25,
        jitter: 2.0,
        jitter_rate: 0.5,
        row_rate: 2.0,
        stagger: "per_branch".to_string(),
        boil_on: false,
        boil_paint: 0.5,
        boil_ink: 1.5,
        boil_cadence: 3,
        echo_count: 3,
        echo_spread: 1.25,
        echo_alpha: 0.5,
        variance_amount: 1.0,
        variance_steady: 0.0,
        keep_size: false,
        softness: 0.4,
    };
    assert_eq!(LivingKnobs::from_preset(&knobs.to_preset()), knobs);
}

#[test]
fn looking_at_it_asks_for_nothing() {
    let ctx = egui::Context::default();
    let mut panel = LivingPropsDebugPanel::new();
    panel.refresh(state());
    for _ in 0..3 {
        frame(&ctx, &mut panel, Vec::new());
    }
    assert!(!panel.is_dirty());
    assert!(panel.take_actions().is_empty());
}

/// Press the pointer at each point down a column of the panel, as a hand
/// reaching for whatever's there, and return what that asked for.
fn press_down(panel: &mut LivingPropsDebugPanel, x: f32) -> Vec<LivingPanelAction> {
    let ctx = egui::Context::default();
    frame(&ctx, panel, Vec::new());
    frame(&ctx, panel, Vec::new());
    // A hand that keeps to one scope: a press on the selector is put back.
    let scope = panel.scope().clone();
    let mut asked = Vec::new();
    for y in (0..1400).step_by(9) {
        let at = Pos2::new(x, y as f32);
        let press = |pressed| egui::Event::PointerButton {
            pos: at,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: Default::default(),
        };
        frame(&ctx, panel, vec![egui::Event::PointerMoved(at)]);
        frame(&ctx, panel, vec![press(true)]);
        frame(&ctx, panel, vec![press(false)]);
        frame(&ctx, panel, vec![egui::Event::PointerGone]);
        asked.extend(panel.take_actions());
        panel.clear_dirty();
        panel.set_scope(scope.clone());
    }
    asked
}

/// How many knobs a preset sets.
fn count_set(preset: &WoozePreset) -> usize {
    let f = KnobFlags::set_by(preset);
    let boil = preset.boil.unwrap_or_default();
    [
        f.step,
        f.glide,
        f.amplitude,
        f.jitter,
        f.jitter_rate,
        f.row_rate,
        f.stagger,
        f.echo_count,
        f.echo_spread,
        f.echo_alpha,
        f.variance_amount,
        f.variance_steady,
        f.keep_size,
        f.softness,
        boil.enabled.is_some(),
        boil.kuwahara.is_some(),
        boil.contour.is_some(),
        boil.on.is_some(),
    ]
    .into_iter()
    .filter(|set| *set)
    .count()
}

/// Every slider, checkbox and shortcut in the column sends the knob it moved
/// and nothing else (#244): a whole set would flatten a grove's species.
#[test]
fn pressing_down_the_sliders_moves_every_knob_one_at_a_time() {
    let mut panel = LivingPropsDebugPanel::new();
    panel.refresh(state());
    // On the sliders' rails, right of every section header's text: on
    // egui 0.36 a press at x = 60 folds the Clock, Shading boil and
    // Echoes sections away before their sliders are reached (#945)
    let asked = press_down(&mut panel, 110.0);
    let sets: Vec<WoozePreset> = asked
        .iter()
        .filter_map(|action| match action {
            LivingPanelAction::Set(_, moved) => Some(moved.clone()),
            _ => None,
        })
        .collect();
    assert!(!sets.is_empty(), "pressing a slider turned it");
    let mut all = KnobFlags::default();
    for moved in &sets {
        // One knob, or Unheld's three.
        let n = count_set(moved);
        assert!(n == 1 || n == 3, "{n} knobs in {moved:?}");
        let f = KnobFlags::set_by(moved);
        all = KnobFlags {
            step: all.step || f.step,
            jitter_rate: all.jitter_rate || f.jitter_rate,
            row_rate: all.row_rate || f.row_rate,
            echo_count: all.echo_count || f.echo_count,
            echo_spread: all.echo_spread || f.echo_spread,
            echo_alpha: all.echo_alpha || f.echo_alpha,
            variance_amount: all.variance_amount || f.variance_amount,
            variance_steady: all.variance_steady || f.variance_steady,
            softness: all.softness || f.softness,
            ..all
        };
    }
    assert!(all.step && all.echo_count && all.echo_spread && all.echo_alpha);
    assert!(all.variance_amount && all.variance_steady, "{sets:?}");
    assert!(all.softness, "the Silhouette slider: {sets:?}");
    assert!(all.jitter_rate, "the jitter rate slider: {sets:?}");
    assert!(all.row_rate, "the row rate slider: {sets:?}");
}

#[test]
fn a_knob_moved_from_before_is_the_only_one_set() {
    let before = LivingKnobs::default();
    let mut after = before.clone();
    after.glide = 0.9;
    let moved = after.moved(&before, &KnobFlags::default());
    assert_eq!(moved.wooze.glide, Some(0.9));
    assert_eq!(count_set(&moved), 1, "{moved:?}");
    assert_eq!(
        after.moved(&after, &KnobFlags::default()),
        WoozePreset::default()
    );
    // Touched, it's sent at the value it already had.
    let touched = KnobFlags {
        keep_size: true,
        ..Default::default()
    };
    let moved = after.moved(&after, &touched);
    assert_eq!(moved.living.variance.unwrap().keep_size, Some(true));
    assert_eq!(count_set(&moved), 1);
}

/// Every piece of text a frame drew.
fn texts(output: &egui::FullOutput) -> Vec<String> {
    output
        .shapes
        .iter()
        .filter_map(|clipped| match &clipped.shape {
            Shape::Text(text) => Some(text.galley.text().to_string()),
            _ => None,
        })
        .collect()
}

#[test]
fn a_knob_the_species_disagree_on_reads_mixed() {
    let ctx = egui::Context::default();
    let draw = |panel: &mut LivingPropsDebugPanel| {
        let input = egui::RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(420.0, 1400.0))),
            ..Default::default()
        };
        // Collapsing headers open on the second frame.
        let _ = run_headless_frame(&ctx, input.clone(), |ui| {
            egui::CentralPanel::default().show(ui, |ui| panel.ui(ui));
        });
        let output = run_headless_frame(&ctx, input, |ui| {
            egui::CentralPanel::default().show(ui, |ui| panel.ui(ui));
        });
        texts(&output)
    };
    let mut panel = LivingPropsDebugPanel::new();
    panel.refresh(state());
    assert!(!draw(&mut panel).iter().any(|t| t == "mixed"));

    let mut mixed = state();
    mixed.mixed = KnobFlags {
        stagger: true,
        echo_count: true,
        echo_spread: true,
        variance_amount: true,
        ..Default::default()
    };
    panel.refresh(mixed);
    let said = draw(&mut panel);
    assert_eq!(said.iter().filter(|t| *t == "mixed").count(), 4, "{said:?}");
    assert!(!panel.is_dirty(), "showing mixed asks for nothing");
}

#[test]
fn freeze_is_a_toggle_and_step_needs_the_clock_stopped() {
    // A step of a running clock couldn't be seen, so its button stays off.
    let mut running = LivingPropsDebugPanel::new();
    running.refresh(state());
    let steps = |asked: &[LivingPanelAction]| {
        asked
            .iter()
            .filter(|a| **a == LivingPanelAction::Step)
            .count()
    };
    let mut asked = Vec::new();
    for x in [20.0, 72.0] {
        // Whatever it presses, it can't be Step until Freeze has been.
        let mut panel = LivingPropsDebugPanel::new();
        panel.refresh(state());
        let ctx = egui::Context::default();
        frame(&ctx, &mut panel, Vec::new());
        asked.extend(press_down(&mut running, x));
    }
    assert!(
        asked.contains(&LivingPanelAction::Freeze(true)),
        "the button is in that column: {asked:?}"
    );

    let mut frozen = state();
    frozen.frozen = true;
    let mut panel = LivingPropsDebugPanel::new();
    panel.refresh(frozen);
    let mut pressed = Vec::new();
    for x in (30..110).step_by(10) {
        pressed.extend(press_down(&mut panel, x as f32));
        panel.state.frozen = true;
    }
    assert!(steps(&pressed) >= 1, "{pressed:?}");
}

#[test]
fn a_label_lands_where_the_prop_is_and_never_behind_the_camera() {
    // Looking down -z from the origin, with a plain perspective.
    let view_proj = [
        [1.0, 0.0, 0.0, 0.0],
        [0.0, 1.0, 0.0, 0.0],
        [0.0, 0.0, -1.0, -1.0],
        [0.0, 0.0, -0.2, 0.0],
    ];
    let screen = Rect::from_min_size(Pos2::ZERO, Vec2::new(800.0, 600.0));
    let mut ahead = row("ahead", [0.0, 0.0, -20.0]);
    ahead.height = 0.0;
    let (at, depth) = project(&ahead, &view_proj, screen).unwrap();
    assert!(
        (at.x - 400.0).abs() < 0.01 && (at.y - 300.0).abs() < 0.01,
        "{at:?}"
    );
    assert_eq!(depth, 20.0);
    assert!(project(&row("behind", [0.0, 0.0, 20.0]), &view_proj, screen).is_none());
    assert!(project(&row("aside", [100.0, 0.0, -2.0]), &view_proj, screen).is_none());
}

#[test]
fn a_label_that_would_land_on_a_nearer_one_stays_quiet() {
    let ctx = egui::Context::default();
    let mut panel = LivingPropsDebugPanel::new();
    let view_proj = [
        [1.0, 0.0, 0.0, 0.0],
        [0.0, 1.0, 0.0, 0.0],
        [0.0, 0.0, -1.0, -1.0],
        [0.0, 0.0, -0.2, 0.0],
    ];
    let at = |z: f32| {
        let mut row = row("oak", [0.0, 0.0, z]);
        row.height = 0.0;
        row
    };
    let painted = |rows: Vec<LivingPropRow>, panel: &mut LivingPropsDebugPanel| {
        panel.refresh(LivingPanelState {
            rows,
            overlay: true,
            ..Default::default()
        });
        let output = run_headless_frame(&ctx, egui::RawInput::default(), |ui| {
            let ctx = &ui.ctx().clone();
            panel.paint_overlay(ctx, &view_proj);
        });
        output.shapes.len()
    };
    let one = painted(vec![at(-20.0)], &mut panel);
    // Right behind it, so it projects to the same place.
    let behind = painted(vec![at(-40.0), at(-20.0)], &mut panel);
    assert_eq!(one, behind);
    // Off to the side there's room for both.
    let mut aside = at(-20.0);
    aside.position[0] = 10.0;
    assert_eq!(painted(vec![at(-20.0), aside], &mut panel), one * 2);
}

#[test]
fn an_overlay_paints_only_when_on() {
    let ctx = egui::Context::default();
    let mut panel = LivingPropsDebugPanel::new();
    let view_proj = [
        [1.0, 0.0, 0.0, 0.0],
        [0.0, 1.0, 0.0, 0.0],
        [0.0, 0.0, -1.0, -1.0],
        [0.0, 0.0, -0.2, 0.0],
    ];
    let mut with_prop = state();
    with_prop.rows = vec![row("oak_1", [0.0, 0.0, -20.0])];
    panel.refresh(with_prop.clone());
    let shapes = |ctx: &egui::Context, panel: &mut LivingPropsDebugPanel| {
        let output = run_headless_frame(ctx, egui::RawInput::default(), |ui| {
            let ctx = &ui.ctx().clone();
            panel.paint_overlay(ctx, &view_proj);
        });
        output.shapes.len()
    };
    assert_eq!(shapes(&ctx, &mut panel), 0);
    with_prop.overlay = true;
    panel.refresh(with_prop);
    assert!(shapes(&ctx, &mut panel) >= 4);
}

// ---- Scopes (#247) ----

/// Every knob from `layer`.
fn all_from(layer: &str) -> KnobSources {
    let mut sources = KnobSources::default();
    for knob in Knob::ALL {
        sources.set(knob, vec![layer.to_string()]);
    }
    sources
}

/// One oak and two thorns. The thorns set their own echo count, and one
/// stands in a zone with its own step.
fn grove() -> LivingPanelState {
    let mut thorn_1 = row("thorn_1", [10.0, 0.0, -20.0]);
    thorn_1.species = "thorn".to_string();
    let mut thorn_2 = row("thorn_2", [-10.0, 0.0, -20.0]);
    thorn_2.species = "thorn".to_string();
    let mut thorn = LivingSpeciesState {
        name: "thorn".to_string(),
        props: 2,
        knobs: LivingKnobs {
            echo_count: 2,
            ..Default::default()
        },
        mixed: KnobFlags {
            step: true,
            ..Default::default()
        },
        sources: all_from("global"),
        unsaved: false,
    };
    thorn
        .sources
        .set(Knob::EchoCount, vec!["species".to_string()]);
    thorn.sources.set(
        Knob::Step,
        vec!["zone:hush".to_string(), "global".to_string()],
    );
    LivingPanelState {
        rows: vec![row("oak_1", [0.0, 0.0, -20.0]), thorn_1, thorn_2],
        shadows: "blended".to_string(),
        species: vec![
            LivingSpeciesState {
                name: "oak".to_string(),
                props: 1,
                knobs: LivingKnobs::default(),
                mixed: KnobFlags::default(),
                sources: all_from("spec"),
                unsaved: false,
            },
            thorn,
        ],
        ..state()
    }
}

/// Every piece of text the panel draws, and where, once its headers are open.
/// What the panel draws once it has settled. egui 0.36 leaves out of
/// a frame's shapes what its clip hides, so a header still animating open
/// draws only the rows uncovered so far (0.30 culled them later, when
/// painting): frames until egui stops asking for the next at once (#945).
fn shown(ctx: &egui::Context, panel: &mut LivingPropsDebugPanel) -> Vec<(String, Rect)> {
    frame(ctx, panel, Vec::new());
    let input = egui::RawInput {
        screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(420.0, 1400.0))),
        ..Default::default()
    };
    let mut last = Vec::new();
    for _ in 0..64 {
        let output = run_headless_frame(ctx, input.clone(), |ui| {
            egui::CentralPanel::default().show(ui, |ui| panel.ui(ui));
        });
        let animating = output
            .viewport_output
            .get(&egui::ViewportId::ROOT)
            .is_some_and(|v| v.repaint_delay.is_zero());
        last = output
            .shapes
            .iter()
            .filter_map(|clipped| match &clipped.shape {
                Shape::Text(text) => Some((
                    text.galley.text().to_string(),
                    Rect::from_min_size(text.pos, text.galley.size()),
                )),
                _ => None,
            })
            .collect();
        if !animating {
            break;
        }
    }
    last
}

/// Click the first text that reads `text`, and return what that asked for.
fn click_text(
    ctx: &egui::Context,
    panel: &mut LivingPropsDebugPanel,
    text: &str,
) -> Vec<LivingPanelAction> {
    let said = shown(ctx, panel);
    let (_, rect) = said
        .iter()
        .find(|(t, _)| t == text)
        .unwrap_or_else(|| panic!("no {text:?} in {said:?}"));
    let at = rect.center();
    let press = |pressed| egui::Event::PointerButton {
        pos: at,
        button: egui::PointerButton::Primary,
        pressed,
        modifiers: Default::default(),
    };
    frame(ctx, panel, vec![egui::Event::PointerMoved(at)]);
    frame(ctx, panel, vec![press(true)]);
    frame(ctx, panel, vec![press(false)]);
    frame(ctx, panel, vec![egui::Event::PointerGone]);
    panel.take_actions()
}

fn says(said: &[(String, Rect)], text: &str) -> usize {
    said.iter().filter(|(t, _)| t.contains(text)).count()
}

#[test]
fn global_looks_as_it_did_with_a_mark_only_where_a_species_sets_its_own() {
    let ctx = egui::Context::default();
    let mut panel = LivingPropsDebugPanel::new();
    let mut plain = grove();
    for species in &mut plain.species {
        species.sources = all_from("global");
    }
    panel.refresh(plain);
    let said = shown(&ctx, &mut panel);
    for mark in ["own", "species", "zone:", "Whole scene only", "shadows"] {
        assert_eq!(says(&said, mark), 0, "{mark:?} in {said:?}");
    }
    assert_eq!(says(&said, "Shading boil"), 1);

    // The thorns' own count is the one thing Global has to say about them.
    panel.refresh(grove());
    let said = shown(&ctx, &mut panel);
    assert_eq!(says(&said, "1 own"), 1, "{said:?}");
    assert_eq!(says(&said, " own"), 1);
    assert!(!panel.is_dirty(), "showing marks asks for nothing");
}

#[test]
fn a_species_knob_is_sent_for_that_species_and_its_clear_for_that_knob() {
    let ctx = egui::Context::default();
    let mut panel = LivingPropsDebugPanel::new();
    panel.refresh(grove());
    assert_eq!(
        click_text(&ctx, &mut panel, "thorn 2 \u{2022}"),
        Vec::new(),
        "picking a species asks the host for nothing"
    );
    assert_eq!(panel.scope(), &PanelScope::Species("thorn".to_string()));
    let said = shown(&ctx, &mut panel);
    assert_eq!(says(&said, "global \u{b7} zone:hush"), 1, "{said:?}");
    assert_eq!(said.iter().filter(|(t, _)| t == "species").count(), 1);

    let asked = press_down(&mut panel, 60.0);
    let sets: Vec<&LivingPanelAction> = asked
        .iter()
        .filter(|a| matches!(a, LivingPanelAction::Set(..)))
        .collect();
    assert!(!sets.is_empty());
    for set in sets {
        let LivingPanelAction::Set(scope, moved) = set else {
            unreachable!()
        };
        assert_eq!(scope, &PanelScope::Species("thorn".to_string()));
        assert_eq!(moved.boil, None, "a species gets no boil: {moved:?}");
    }

    let ctx = egui::Context::default();
    let mut panel = LivingPropsDebugPanel::new();
    panel.refresh(grove());
    panel.set_scope(PanelScope::Species("thorn".to_string()));
    assert_eq!(
        click_text(&ctx, &mut panel, "\u{d7}"),
        vec![LivingPanelAction::Clear(
            "thorn".to_string(),
            Some(Knob::EchoCount)
        )]
    );
    assert_eq!(
        click_text(&ctx, &mut panel, "Clear all"),
        vec![LivingPanelAction::Clear("thorn".to_string(), None)]
    );
}

#[test]
fn at_a_species_the_boil_and_shadows_are_shown_off_with_the_reason() {
    let ctx = egui::Context::default();
    let mut panel = LivingPropsDebugPanel::new();
    panel.refresh(grove());
    panel.set_scope(PanelScope::Species("oak".to_string()));
    let said = shown(&ctx, &mut panel);
    assert_eq!(says(&said, "Whole scene only"), 1);
    assert_eq!(says(&said, "shadows: blended"), 1);
    assert_eq!(says(&said, "every species boils together"), 1, "{said:?}");
    assert_eq!(says(&said, "a species can't set them"), 1);
    assert_eq!(says(&said, "Clear all"), 1, "shown, if off: oak has none");

    // However hard it's pressed, the boil doesn't move.
    let mut asked = Vec::new();
    for x in [24.0, 60.0, 110.0, 160.0] {
        asked.extend(press_down(&mut panel, x));
    }
    for action in &asked {
        match action {
            LivingPanelAction::Set(scope, moved) => {
                assert_eq!(scope, &PanelScope::Species("oak".to_string()));
                assert_eq!(moved.boil, None);
            }
            // Once a knob is the species' own, its clear is there to press.
            LivingPanelAction::Clear(name, _) => assert_eq!(name, "oak"),
            _ => {}
        }
    }
}

#[test]
fn a_picked_species_is_the_only_one_the_overlay_labels() {
    let ctx = egui::Context::default();
    let view_proj = [
        [1.0, 0.0, 0.0, 0.0],
        [0.0, 1.0, 0.0, 0.0],
        [0.0, 0.0, -1.0, -1.0],
        [0.0, 0.0, -0.2, 0.0],
    ];
    let mut panel = LivingPropsDebugPanel::new();
    let mut on = grove();
    on.overlay = true;
    panel.refresh(on);
    let labels = |panel: &mut LivingPropsDebugPanel| {
        let output = run_headless_frame(&ctx, egui::RawInput::default(), |ui| {
            let ctx = &ui.ctx().clone();
            panel.paint_overlay(ctx, &view_proj);
        });
        texts(&output)
    };
    let all = labels(&mut panel);
    assert_eq!(all.len(), 3, "{all:?}");
    panel.set_scope(PanelScope::Species("thorn".to_string()));
    assert_eq!(labels(&mut panel).len(), 2);
    panel.set_scope(PanelScope::Species("oak".to_string()));
    assert_eq!(labels(&mut panel).len(), 1);
}

#[test]
fn the_props_are_grouped_by_species_and_a_filter_narrows_them() {
    let ctx = egui::Context::default();
    let mut panel = LivingPropsDebugPanel::new();
    panel.refresh(grove());
    click_text(&ctx, &mut panel, "Props");
    let said = shown(&ctx, &mut panel);
    assert_eq!(says(&said, "oak  1"), 1, "{said:?}");
    assert_eq!(says(&said, "thorn  2"), 1);
    assert_eq!(says(&said, "thorn_1"), 0, "folded until opened");

    panel.filter = "thorn".to_string();
    let said = shown(&ctx, &mut panel);
    assert_eq!(says(&said, "oak  1"), 0);
    assert_eq!(says(&said, "thorn_1"), 1, "a filter opens what it finds");
    assert_eq!(says(&said, "thorn_2"), 1);

    // A group's tune picks it.
    panel.filter.clear();
    click_text(&ctx, &mut panel, "tune");
    assert_eq!(panel.scope(), &PanelScope::Species("oak".to_string()));
}

#[test]
fn a_species_that_goes_away_gives_the_grove_back() {
    let mut panel = LivingPropsDebugPanel::new();
    panel.refresh(grove());
    panel.set_scope(PanelScope::Species("thorn".to_string()));
    panel.refresh(state());
    assert_eq!(panel.scope(), &PanelScope::Global);
}

#[test]
fn a_mark_counts_zones_once_there_are_two() {
    let labels = |l: &[&str]| -> Vec<String> { l.iter().map(|s| s.to_string()).collect() };
    assert_eq!(short_mark(&labels(&["species"])), "species");
    assert_eq!(
        short_mark(&labels(&["zone:hush", "global"])),
        "global \u{b7} zone:hush"
    );
    assert_eq!(
        short_mark(&labels(&[
            "zone:shiver",
            "global",
            "zone:drowse",
            "zone:hush"
        ])),
        "global \u{b7} 3 zones"
    );
}

#[test]
fn every_knob_has_the_key_its_layer_report_gives_it() {
    let keys: Vec<&str> = Knob::ALL.iter().map(|k| k.key()).collect();
    assert_eq!(
        keys,
        [
            "step",
            "glide",
            "amplitude",
            "jitter",
            "jitter_rate",
            "row_rate",
            "stagger",
            "echoes.count",
            "echoes.spread",
            "echoes.alpha",
            "variance.amount",
            "variance.steady",
            "variance.keep_size",
            "softness",
        ]
    );
    for knob in Knob::ALL {
        assert_eq!(Knob::ALL[knob.index()], knob);
        assert_eq!(knob.desc().knob, knob);
    }
}

/// The table is the one place a knob is described (#753): every row's
/// control can turn its value, says what it does, and round-trips through
/// a preset by itself.
#[test]
fn every_knob_has_a_control_that_fits() {
    for knob in Knob::ALL {
        let desc = knob.desc();
        assert!(fits(desc.control, knob), "{knob:?}");
        assert!(desc.help.ends_with('.'), "{knob:?} needs a help line");
        let mut touched = KnobFlags::default();
        touched.set_flag(knob, true);
        let base = LivingKnobs::default();
        let moved = base.moved(&base, &touched);
        assert_eq!(
            KnobFlags::set_by(&moved),
            touched,
            "{knob:?} sets only itself"
        );
        assert_eq!(moved.boil, None);
    }
    let clock: Vec<Knob> = Knob::ALL.into_iter().filter(|k| k.is_clock()).collect();
    assert_eq!(
        clock,
        [Knob::Step, Knob::Glide, Knob::Amplitude, Knob::Jitter]
    );
    let sections: Vec<usize> = [
        KnobSection::Clock,
        KnobSection::Echoes,
        KnobSection::Variance,
        KnobSection::Silhouette,
    ]
    .into_iter()
    .map(|section| Knob::in_section(section).count())
    .collect();
    assert_eq!(sections, [7, 3, 3, 1]);
}

// ---- Picking a tree in the world (#248) ----

/// Looking down -z from `eye`, with a plain perspective: the one the label
/// tests use, after a move by -eye.
fn looking_from(eye: [f32; 3]) -> [[f32; 4]; 4] {
    let [x, y, z] = eye;
    [
        [1.0, 0.0, 0.0, 0.0],
        [0.0, 1.0, 0.0, 0.0],
        [0.0, 0.0, -1.0, -1.0],
        [-x, -y, z - 0.2, z],
    ]
}

const SCREEN: Rect = Rect {
    min: Pos2::ZERO,
    max: Pos2::new(800.0, 600.0),
};

/// A tree 8 m tall reaching 3 m sideways, at `position`.
fn tree(name: &str, species: &str, position: [f32; 3]) -> LivingPropRow {
    let mut tree = row(name, position);
    tree.species = species.to_string();
    tree.height = 8.0;
    tree.reach = 3.0;
    tree
}

#[test]
fn a_pick_takes_the_nearest_tree_under_it_and_misses_the_sky() {
    let view = looking_from([0.0, 2.0, 0.0]);
    let rows = vec![
        tree("far", "oak", [0.0, 0.0, -40.0]),
        tree("near", "oak", [0.0, 0.0, -15.0]),
        tree("aside", "thorn", [12.0, 0.0, -30.0]),
    ];
    // Through both crowns, the near one's.
    let (crown, _) = project_point([0.0, 6.0, -15.0], &view, SCREEN).unwrap();
    assert_eq!(pick_prop(&rows, &view, SCREEN, crown), Some(1));
    // The thorn's crown, out toward its reach: picked. Twice that: not.
    let (aside, _) = project_point([12.0, 7.0, -30.0], &view, SCREEN).unwrap();
    assert_eq!(pick_prop(&rows, &view, SCREEN, aside), Some(2));
    let (wide, _) = project_point([12.0 + 2.6, 7.0, -30.0], &view, SCREEN).unwrap();
    assert_eq!(
        pick_prop(&rows, &view, SCREEN, wide),
        Some(2),
        "in its crown"
    );
    let (out, _) = project_point([12.0 + 6.0, 7.0, -30.0], &view, SCREEN).unwrap();
    assert_eq!(pick_prop(&rows, &view, SCREEN, out), None, "past its crown");
    // The sky, and a tree behind the camera, never.
    assert_eq!(
        pick_prop(&rows, &view, SCREEN, Pos2::new(400.0, 10.0)),
        None
    );
    let behind = vec![tree("behind", "oak", [0.0, 0.0, 10.0])];
    assert_eq!(
        pick_prop(&behind, &view, SCREEN, Pos2::new(400.0, 300.0)),
        None
    );
}

/// The grove, with thorn_1 standing in the hush and its species owning its
/// echo count.
fn picked_grove() -> LivingPanelState {
    let thorn = LivingKnobs {
        echo_count: 2,
        ..Default::default()
    };
    let mut sources = all_from("global");
    sources.set(Knob::EchoCount, vec!["species".to_string()]);
    sources.set(Knob::Step, vec!["zone:hush".to_string()]);
    LivingPanelState {
        rows: vec![
            tree("oak_1", "oak", [0.0, 0.0, -20.0]),
            tree("thorn_1", "thorn", [6.0, 0.0, -20.0]),
            tree("thorn_2", "thorn", [-6.0, 0.0, -25.0]),
        ],
        picked: Some(LivingPickedState {
            name: "thorn_1".to_string(),
            zones: vec![("hush".to_string(), 1.0)],
            knobs: thorn,
            sources,
        }),
        ..grove()
    }
}

/// Paint the overlay for one frame, with `events`, and return what it drew.
fn overlay(
    ctx: &egui::Context,
    panel: &mut LivingPropsDebugPanel,
    view: &[[f32; 4]; 4],
    events: Vec<egui::Event>,
) -> egui::FullOutput {
    let input = egui::RawInput {
        screen_rect: Some(SCREEN),
        events,
        ..Default::default()
    };
    run_headless_frame(ctx, input, |ui| panel.paint_overlay(ui.ctx(), view))
}

/// Two frames of it, since a new window is laid out unseen on its first.
fn overlay_settled(
    ctx: &egui::Context,
    panel: &mut LivingPropsDebugPanel,
    view: &[[f32; 4]; 4],
) -> egui::FullOutput {
    overlay(ctx, panel, view, Vec::new());
    overlay(ctx, panel, view, Vec::new())
}

fn rings(output: &egui::FullOutput) -> usize {
    (output.shapes.iter())
            .filter(|clipped| matches!(&clipped.shape, Shape::Path(path) if path.closed && path.points.len() == 40))
            .count()
}

fn window(ctx: &egui::Context) -> Rect {
    ctx.memory(|m| m.area_rect(egui::Id::new("living_picked_window")))
        .expect("the window was drawn")
}

#[test]
fn picking_a_tree_turns_to_its_species_and_marks_every_one_of_it() {
    let ctx = egui::Context::default();
    let mut panel = LivingPropsDebugPanel::new();
    panel.refresh(picked_grove());
    let view = looking_from([0.0, 2.0, 0.0]);
    assert_eq!(
        rings(&overlay_settled(&ctx, &mut panel, &view)),
        0,
        "none picked"
    );
    assert!(panel.take_actions().is_empty());

    assert!(panel.select(Some("thorn_1")));
    assert_eq!(panel.scope(), &PanelScope::Species("thorn".to_string()));
    assert_eq!(panel.selected(), Some("thorn_1"));
    let output = overlay_settled(&ctx, &mut panel, &view);
    // Two thorns, and the picked one's ring twice over; no oak.
    assert_eq!(rings(&output), 3);
    assert_eq!(panel.picked_on_screen(), Some(true));
    let said = texts(&output);
    for text in [
        "thorn_1",
        "in zone hush",
        "species",
        "zone:hush",
        "Esc lets go",
    ] {
        assert!(
            said.iter().any(|t| t.contains(text)),
            "{text:?} in {said:?}"
        );
    }
    assert!(panel.take_actions().is_empty(), "looking asks for nothing");

    // A name no tree has lets go; so does a tree that's gone.
    assert!(!panel.select(Some("birch_9")));
    assert_eq!(panel.selected(), None);
    panel.select(Some("thorn_2"));
    let mut gone = picked_grove();
    gone.rows.retain(|r| r.name != "thorn_2");
    panel.refresh(gone);
    assert_eq!(panel.selected(), None);
}

#[test]
fn the_window_follows_its_tree_and_never_leaves_the_screen() {
    let ctx = egui::Context::default();
    let mut panel = LivingPropsDebugPanel::new();
    panel.refresh(picked_grove());
    panel.select(Some("thorn_1"));
    let mut seen = 0;
    // The camera walks left past the tree, which slides right across the
    // screen and off it; the window goes with it and stays on the screen.
    for x in [10.0, 5.0, 0.0, -5.0, -12.0, -20.0] {
        let view = looking_from([x, 2.0, 0.0]);
        overlay_settled(&ctx, &mut panel, &view);
        let (crown, _) = project_point([6.0, 8.0, -20.0], &view, SCREEN).unwrap();
        if !SCREEN.contains(crown) {
            assert_eq!(panel.picked_on_screen(), Some(false), "at {x}");
            continue;
        }
        assert_eq!(panel.picked_on_screen(), Some(true), "at {x}");
        let rect = window(&ctx);
        assert!(SCREEN.contains_rect(rect), "{rect:?} at {x}");
        // Beside the crown, on whichever side has room, unless the screen's
        // edge holds it in.
        let gap = (rect.left() - crown.x)
            .abs()
            .min((rect.right() - crown.x).abs());
        let held = rect.left() <= 5.0 || rect.right() >= SCREEN.right() - 5.0;
        assert!(gap <= 40.0 || held, "{gap} from the crown at {x}: {rect:?}");
        seen += 1;
    }
    assert!(seen >= 4, "{seen}");

    // Close under it, its crown in the screen's top corner: held in.
    let view = looking_from([0.0, 0.5, -12.0]);
    overlay_settled(&ctx, &mut panel, &view);
    let (crown, _) = project_point([6.0, 8.0, -20.0], &view, SCREEN).unwrap();
    assert!(crown.y < 40.0 && crown.x > 600.0, "{crown:?}");
    assert_eq!(panel.picked_on_screen(), Some(true));
    assert!(SCREEN.contains_rect(window(&ctx)), "{:?}", window(&ctx));

    // With the panel open down the right, the window keeps out from under
    // it, even for a tree standing behind the panel's column.
    panel.toggle();
    panel.panel_left = Some(600.0);
    let view = looking_from([-9.0, 2.0, 0.0]);
    overlay_settled(&ctx, &mut panel, &view);
    let (crown, _) = project_point([6.0, 8.0, -20.0], &view, SCREEN).unwrap();
    assert!(crown.x > 650.0, "{crown:?}");
    let rect = window(&ctx);
    assert!(rect.right() <= 600.0, "{rect:?} under the panel");
}

#[test]
fn a_tree_off_screen_draws_no_window_and_the_panel_says_so() {
    let ctx = egui::Context::default();
    let mut panel = LivingPropsDebugPanel::new();
    panel.refresh(picked_grove());
    panel.select(Some("thorn_1"));
    // Past the thorns, looking on: they're behind.
    let away = looking_from([0.0, 2.0, -60.0]);
    let output = overlay_settled(&ctx, &mut panel, &away);
    assert_eq!(panel.picked_on_screen(), Some(false));
    assert!(!texts(&output).iter().any(|t| t.contains("Esc lets go")));
    assert_eq!(rings(&output), 0);
    let said = shown(&egui::Context::default(), &mut panel);
    assert_eq!(says(&said, "off screen"), 1, "{said:?}");
    assert_eq!(says(&said, "Picked"), 1);
}

/// A hand on a slider in the window: the picked tree's species, that knob
/// alone, and no boil. The window's × clears that species' own value.
#[test]
fn the_windows_sliders_turn_the_trees_species() {
    let view = looking_from([0.0, 2.0, 0.0]);
    let fresh = || {
        let ctx = egui::Context::default();
        let mut panel = LivingPropsDebugPanel::new();
        panel.refresh(picked_grove());
        panel.select(Some("thorn_1"));
        let output = overlay_settled(&ctx, &mut panel, &view);
        (ctx, panel, output)
    };
    let at = |output: &egui::FullOutput, text: &str| {
        (output.shapes.iter())
            .find_map(|clipped| match &clipped.shape {
                Shape::Text(t) if t.galley.text() == text => {
                    Some(Rect::from_min_size(t.pos, t.galley.size()))
                }
                _ => None,
            })
            .unwrap_or_else(|| panic!("no {text:?} in {:?}", texts(output)))
    };
    let press = |ctx: &egui::Context, panel: &mut LivingPropsDebugPanel, at: Pos2| {
        let button = |pressed| egui::Event::PointerButton {
            pos: at,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: Default::default(),
        };
        overlay(ctx, panel, &view, vec![egui::Event::PointerMoved(at)]);
        overlay(ctx, panel, &view, vec![button(true)]);
        overlay(ctx, panel, &view, vec![button(false)]);
        overlay(ctx, panel, &view, vec![egui::Event::PointerGone]);
        panel.take_actions()
    };

    let (ctx, mut panel, output) = fresh();
    let clear = at(&output, "\u{d7}");
    assert_eq!(
        press(&ctx, &mut panel, clear.center()),
        vec![LivingPanelAction::Clear(
            "thorn".to_string(),
            Some(Knob::EchoCount)
        )]
    );

    // The glide slider's rail, left of its number and its label.
    let (ctx, mut panel, output) = fresh();
    let glide = at(&output, "glide");
    let asked = press(
        &ctx,
        &mut panel,
        Pos2::new(glide.left() - 100.0, glide.center().y),
    );
    let [LivingPanelAction::Set(scope, moved)] = asked.as_slice() else {
        panic!("{asked:?}");
    };
    assert_eq!(scope, &PanelScope::Species("thorn".to_string()));
    assert_eq!(moved.boil, None);
    assert!(moved.wooze.glide.is_some(), "{moved:?}");
    assert_eq!(count_set(moved), 1, "{moved:?}");
}

#[test]
fn with_nothing_unsaved_there_are_no_marks_and_commit_asks_for_nothing() {
    let ctx = egui::Context::default();
    let mut panel = LivingPropsDebugPanel::new();
    panel.refresh(grove());
    let said = shown(&ctx, &mut panel);
    assert_eq!(says(&said, " *"), 0, "{said:?}");
    assert_eq!(says(&said, "the scene has it all"), 1, "{said:?}");
    assert!(click_text(&ctx, &mut panel, "Commit to File").is_empty());
}

#[test]
fn an_unsaved_level_wears_a_star_and_commit_sends_once() {
    let ctx = egui::Context::default();
    let mut panel = LivingPropsDebugPanel::new();
    let mut state = grove();
    state.species[1].unsaved = true;
    state.unsaved = vec!["[living.species.thorn] echoes.count = 4".to_string()];
    panel.refresh(state.clone());
    let said = shown(&ctx, &mut panel);
    assert_eq!(says(&said, "thorn 2 \u{2022} *"), 1, "{said:?}");
    assert_eq!(says(&said, "Global *"), 0, "the grove's own are saved");
    assert_eq!(says(&said, "* 1 value unsaved"), 1, "{said:?}");

    state.unsaved_global = true;
    state.unsaved.push("[wooze] glide = 0.8".to_string());
    panel.refresh(state);
    let said = shown(&ctx, &mut panel);
    assert_eq!(says(&said, "Global *"), 1, "{said:?}");
    assert_eq!(says(&said, "* 2 values unsaved"), 1, "{said:?}");
    let asked = click_text(&ctx, &mut panel, "Commit to File");
    assert_eq!(asked, [LivingPanelAction::Commit]);
}
