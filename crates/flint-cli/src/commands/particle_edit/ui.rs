//! egui layout for the particle editor: left effect panel, bottom timeline,
//! viewport overlays. Every widget mutates the authored `EmitterDef`
//! directly and reports `changed`; `mod.rs` turns that into undo + re-sim.

use egui::{Color32, Pos2, Rect, Sense, Stroke, Vec2};
use flint_debug_ui::widgets::{
    check, combo_str, drag_f32, drag_range_f32, drag_u32, drag_xyz, CurveEditor, GradientEditor,
};
use flint_particles::effect::{CountDef, ShapeField};
use flint_particles::{
    BurstDef, Curve, CurveDef, EmitterDef, ForceDef, Interp, ParticleBlendMode, ParticleEffect,
    RangeDef, ShapeDef, SortMode, SubEmitterDef,
};

use super::{presets, EmitterView, ParticleEditApp, UiActions};

const ACCENT: Color32 = Color32::from_rgb(255, 176, 88);

/// Read-only frame data the closure needs alongside the mutable document.
struct Frame {
    time: f32,
    loop_end: f32,
    playing: bool,
    speed: f32,
    looping: bool,
    alive_total: usize,
    per_emitter: Vec<(usize, usize)>,
    step_ms: f32,
    dirty: bool,
    file: String,
    status: Option<String>,
    backdrop: &'static str,
    show_grid: bool,
    show_gizmos: bool,
    auto_orbit: bool,
    can_undo: bool,
    can_redo: bool,
    validation: Option<String>,
}

pub fn render_egui(app: &mut ParticleEditApp, target_view: &wgpu::TextureView) -> UiActions {
    let mut actions = UiActions::default();
    let Some(window) = app.window.clone() else {
        return actions;
    };
    let (Some(context), Some(egui_winit), Some(egui_renderer)) = (
        app.render_context.as_ref(),
        app.egui_winit.as_mut(),
        app.egui_renderer.as_mut(),
    ) else {
        return actions;
    };

    let before = app.effect.clone();
    let frame = Frame {
        time: app.sim.time(),
        loop_end: app.play.loop_end,
        playing: app.play.playing,
        speed: app.play.speed,
        looping: app.play.looping,
        alive_total: app.sim.alive(),
        per_emitter: app.sim.per_emitter_alive(),
        step_ms: app.sim.step_ms(),
        dirty: app.effect != app.saved,
        file: app.path.display().to_string(),
        status: app
            .status
            .as_ref()
            .filter(|(_, t)| t.elapsed().as_secs_f32() < 4.0)
            .map(|(m, _)| m.clone()),
        backdrop: app.backdrop.label(),
        show_grid: app.show_grid,
        show_gizmos: app.show_gizmos,
        auto_orbit: app.orbit.auto_orbit,
        can_undo: !app.undo.is_empty(),
        can_redo: !app.redo.is_empty(),
        validation: app.effect.validate().err(),
    };

    let effect = &mut app.effect;
    let views = &mut app.views;
    let solo = &mut app.solo;
    let mut selected = app.selected;
    let egui_ctx = &app.egui_ctx;

    let raw_input = egui_winit.take_egui_input(&window);
    let full_output = egui_ctx.run(raw_input, |ctx| {
        draw_menu_bar(ctx, &frame, &mut actions);
        draw_left_panel(
            ctx,
            effect,
            views,
            solo,
            &mut selected,
            &frame,
            &mut actions,
        );
        draw_timeline(ctx, effect, &frame, &mut actions);
        draw_overlays(ctx, effect, &frame, &mut actions);
    });
    if selected != app.selected {
        actions.select = Some(selected);
    }
    if actions.changed || actions.structural {
        app.pre_change_snapshot = Some(before);
    }

    // Paint.
    egui_winit.handle_platform_output(&window, full_output.platform_output);
    let paint_jobs = egui_ctx.tessellate(full_output.shapes, full_output.pixels_per_point);
    let screen = egui_wgpu::ScreenDescriptor {
        size_in_pixels: [context.config.width, context.config.height],
        pixels_per_point: full_output.pixels_per_point,
    };
    for (id, delta) in &full_output.textures_delta.set {
        egui_renderer.update_texture(&context.device, &context.queue, *id, delta);
    }
    let mut encoder = context
        .device
        .create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("Particle Editor egui"),
        });
    egui_renderer.update_buffers(
        &context.device,
        &context.queue,
        &mut encoder,
        &paint_jobs,
        &screen,
    );
    {
        let mut pass = encoder
            .begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("Particle Editor egui pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: target_view,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Load,
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
            })
            .forget_lifetime();
        egui_renderer.render(&mut pass, &paint_jobs, &screen);
    }
    context.queue.submit(std::iter::once(encoder.finish()));
    for id in &full_output.textures_delta.free {
        egui_renderer.free_texture(id);
    }
    actions
}

// ─── Menu bar ───────────────────────────────────────────────────────────────

fn draw_menu_bar(ctx: &egui::Context, f: &Frame, a: &mut UiActions) {
    egui::TopBottomPanel::top("menu_bar").show(ctx, |ui| {
        egui::menu::bar(ui, |ui| {
            ui.menu_button("File", |ui| {
                if ui.button("Save            Ctrl+S").clicked() {
                    a.save = true;
                    ui.close_menu();
                }
                if ui.button("Reload from disk  Ctrl+R").clicked() {
                    a.reload = true;
                    ui.close_menu();
                }
            });
            ui.menu_button("Edit", |ui| {
                if ui
                    .add_enabled(f.can_undo, egui::Button::new("Undo   Ctrl+Z"))
                    .clicked()
                {
                    a.undo = true;
                    ui.close_menu();
                }
                if ui
                    .add_enabled(f.can_redo, egui::Button::new("Redo   Ctrl+Y"))
                    .clicked()
                {
                    a.redo = true;
                    ui.close_menu();
                }
            });
            ui.menu_button("Presets", |ui| {
                ui.weak("Append emitters from a preset");
                for name in presets::names() {
                    if ui.button(name).clicked() {
                        a.add_preset = Some(name);
                        ui.close_menu();
                    }
                }
            });
            ui.menu_button("Help", |ui| {
                ui.monospace("Space  play / pause      R  restart");
                ui.monospace("Home/End  seek           Left/Right step (Shift = 0.1 s)");
                ui.monospace("L  loop                  [ ]  speed / orbit speed");
                ui.monospace("O  auto-orbit            G  grid   X  gizmos   B  backdrop");
                ui.monospace("H  hide UI               Ctrl+D duplicate   Del  delete emitter");
                ui.monospace("Ctrl+S save   Ctrl+Z / Ctrl+Y undo / redo   Esc quit");
                ui.separator();
                ui.monospace("Curves: drag keys · double-click adds · right-click removes");
            });
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let title = if f.dirty {
                    format!("{} *", f.file)
                } else {
                    f.file.clone()
                };
                ui.label(egui::RichText::new(title).weak());
            });
        });
    });
}

// ─── Left panel ─────────────────────────────────────────────────────────────

fn draw_left_panel(
    ctx: &egui::Context,
    effect: &mut ParticleEffect,
    views: &mut Vec<EmitterView>,
    solo: &mut Option<usize>,
    selected: &mut Option<usize>,
    f: &Frame,
    a: &mut UiActions,
) {
    egui::SidePanel::left("effect_panel")
        .default_width(360.0)
        .min_width(300.0)
        .resizable(true)
        .show(ctx, |ui| {
            // A solid (non-floating) scrollbar reserves its own column instead
            // of drawing over the right-hand controls.
            ui.style_mut().spacing.scroll.floating = false;
            // Drag-to-scroll would fight the curve/gradient key drags.
            egui::ScrollArea::vertical().drag_to_scroll(false).show(ui, |ui| {
                // Effect header
                ui.horizontal(|ui| {
                    ui.label(egui::RichText::new("EFFECT").small().weak());
                    let r =
                        ui.add(egui::TextEdit::singleline(&mut effect.name).desired_width(160.0));
                    if r.changed() {
                        a.changed = true;
                    }
                    ui.label("seed");
                    let mut seed = effect.seed as i64;
                    if ui
                        .add(egui::DragValue::new(&mut seed).range(0..=i64::from(u32::MAX)))
                        .changed()
                    {
                        effect.seed = seed.max(0) as u32;
                        a.changed = true;
                    }
                });
                if let Some(err) = &f.validation {
                    ui.colored_label(Color32::from_rgb(255, 120, 90), err);
                }
                ui.add_space(4.0);

                // Emitter list
                ui.label(egui::RichText::new("EMITTERS").small().weak());
                let n = effect.emitters.len();
                views.resize(
                    n,
                    EmitterView {
                        muted: false,
                        show_gizmo: true,
                    },
                );
                let mut reorder: Option<(usize, usize)> = None;
                let mut remove: Option<usize> = None;
                let mut duplicate: Option<usize> = None;
                #[allow(clippy::needless_range_loop)]
                for i in 0..n {
                    let is_sel = *selected == Some(i);
                    let (alive, cap) = f.per_emitter.get(i).copied().unwrap_or((0, 0));
                    ui.horizontal(|ui| {
                        let swatch = first_color(&effect.emitters[i]);
                        let (r, _) = ui.allocate_exact_size(Vec2::splat(10.0), Sense::hover());
                        ui.painter().circle_filled(r.center(), 5.0, swatch);
                        let muted = views[i].muted || solo.is_some_and(|s| s != i);
                        let name = effect.emitters[i].name.clone();
                        let text = if muted {
                            egui::RichText::new(&name).weak()
                        } else {
                            egui::RichText::new(&name)
                        };
                        let resp = ui.selectable_label(is_sel, text);
                        if resp.clicked() {
                            *selected = Some(i);
                        }
                        resp.context_menu(|ui| {
                            if ui.button("Duplicate").clicked() {
                                duplicate = Some(i);
                                ui.close_menu();
                            }
                            if ui.add_enabled(n > 1, egui::Button::new("Delete")).clicked() {
                                remove = Some(i);
                                ui.close_menu();
                            }
                        });
                        ui.weak(format!("{alive}/{cap}"));
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if ui
                                .add_enabled(i + 1 < n, egui::Button::new("v").small())
                                .clicked()
                            {
                                reorder = Some((i, i + 1));
                            }
                            if ui
                                .add_enabled(i > 0, egui::Button::new("^").small())
                                .clicked()
                            {
                                reorder = Some((i, i - 1));
                            }
                            let mut gz = views[i].show_gizmo;
                            if ui
                                .toggle_value(&mut gz, "◎")
                                .on_hover_text("Show shape gizmo")
                                .changed()
                            {
                                views[i].show_gizmo = gz;
                            }
                            let mut m = views[i].muted;
                            if ui
                                .toggle_value(&mut m, "M")
                                .on_hover_text("Mute (preview only)")
                                .changed()
                            {
                                views[i].muted = m;
                                a.structural = true; // re-sim; not a document change
                                a.changed = false;
                            }
                            let mut s = *solo == Some(i);
                            if ui
                                .toggle_value(&mut s, "S")
                                .on_hover_text("Solo (preview only)")
                                .changed()
                            {
                                *solo = if s { Some(i) } else { None };
                                a.structural = true;
                            }
                        });
                    });
                }
                ui.horizontal(|ui| {
                    if ui.button("+ Emitter").clicked() {
                        let em = EmitterDef {
                            name: unique(effect, "emitter"),
                            ..Default::default()
                        };
                        effect.emitters.push(em);
                        views.push(EmitterView {
                            muted: false,
                            show_gizmo: true,
                        });
                        *selected = Some(effect.emitters.len() - 1);
                        a.changed = true;
                        a.structural = true;
                    }
                    ui.menu_button("New from preset...", |ui| {
                        for name in presets::names() {
                            if ui.button(name).clicked() {
                                a.add_preset = Some(name);
                                ui.close_menu();
                            }
                        }
                    });
                });
                if let Some((from, to)) = reorder {
                    effect.emitters.swap(from, to);
                    views.swap(from, to);
                    if *selected == Some(from) {
                        *selected = Some(to);
                    }
                    a.changed = true;
                    a.structural = true;
                }
                if let Some(i) = duplicate {
                    let mut copy = effect.emitters[i].clone();
                    copy.name = unique(effect, &copy.name);
                    effect.emitters.insert(i + 1, copy);
                    views.insert(i + 1, views[i]);
                    *selected = Some(i + 1);
                    a.changed = true;
                    a.structural = true;
                }
                if let Some(i) = remove {
                    effect.emitters.remove(i);
                    views.remove(i);
                    if *selected == Some(i) {
                        *selected = effect.emitters.len().checked_sub(1);
                    }
                    a.changed = true;
                    a.structural = true;
                }

                ui.separator();

                // Selected emitter sections
                let sibling_names: Vec<String> =
                    effect.emitters.iter().map(|e| e.name.clone()).collect();
                if let Some(i) = *selected {
                    if let Some(em) = effect.emitters.get_mut(i) {
                        let mut changed = false;
                        changed |= section_emission(ui, em);
                        changed |= section_shape(ui, em);
                        changed |= section_motion(ui, em);
                        changed |= section_lifetime(ui, em, i);
                        let (c, browse) = section_rendering(ui, em);
                        changed |= c;
                        if browse {
                            a.browse_texture = Some(i);
                        }
                        changed |= section_bursts(ui, em);
                        changed |= section_sub_emitters(ui, em, &sibling_names, i);
                        if changed {
                            a.changed = true;
                        }
                    }
                } else {
                    ui.weak("Select an emitter to edit it.");
                }
                ui.add_space(12.0);
            });
        });
}

fn unique(effect: &ParticleEffect, base: &str) -> String {
    if effect.emitter_index(base).is_none() {
        return base.to_string();
    }
    for i in 2.. {
        let c = format!("{base}_{i}");
        if effect.emitter_index(&c).is_none() {
            return c;
        }
    }
    unreachable!()
}

fn first_color(em: &EmitterDef) -> Color32 {
    let c = match &em.color {
        Some(def) => def.to_curve().map(|c| c.first()).unwrap_or([1.0; 4]),
        None => em.color_start.unwrap_or([1.0, 1.0, 1.0, 1.0]),
    };
    Color32::from_rgb(
        (c[0].clamp(0.0, 1.0) * 255.0) as u8,
        (c[1].clamp(0.0, 1.0) * 255.0) as u8,
        (c[2].clamp(0.0, 1.0) * 255.0) as u8,
    )
}

// ─── Range helpers ──────────────────────────────────────────────────────────

fn range_pair(
    r: Option<RangeDef>,
    legacy_min: Option<f32>,
    legacy_max: Option<f32>,
    d: (f32, f32),
) -> (f32, f32) {
    match r {
        Some(r) => r.pair(),
        None => (legacy_min.unwrap_or(d.0), legacy_max.unwrap_or(d.1)),
    }
}

fn pack_range(min: f32, max: f32) -> RangeDef {
    if (min - max).abs() < 1e-6 {
        RangeDef::Const(min)
    } else {
        RangeDef::MinMax([min, max])
    }
}

fn edit_range(
    ui: &mut egui::Ui,
    label: &str,
    r: &mut RangeDef,
    speed: f64,
    range: std::ops::RangeInclusive<f64>,
) -> bool {
    let (mut lo, mut hi) = r.pair();
    if drag_range_f32(ui, label, &mut lo, &mut hi, speed, range) {
        *r = pack_range(lo, hi);
        return true;
    }
    false
}

fn edit_count(ui: &mut egui::Ui, label: &str, c: &mut CountDef, max: u32) -> bool {
    let (mut lo, mut hi) = c.pair();
    let before = (lo, hi);
    ui.horizontal(|ui| {
        ui.label(label);
        ui.add(egui::DragValue::new(&mut lo).range(0..=max));
        ui.label("-");
        ui.add(egui::DragValue::new(&mut hi).range(0..=max));
    });
    if hi < lo {
        hi = lo;
    }
    if (lo, hi) != before {
        *c = if lo == hi {
            CountDef::Const(lo)
        } else {
            CountDef::MinMax([lo, hi])
        };
        return true;
    }
    false
}

// ─── Sections ───────────────────────────────────────────────────────────────

fn section_emission(ui: &mut egui::Ui, em: &mut EmitterDef) -> bool {
    let mut c = false;
    egui::CollapsingHeader::new("Emission")
        .default_open(true)
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.label("name");
                if ui.text_edit_singleline(&mut em.name).changed() {
                    c = true;
                }
            });
            c |= drag_f32(ui, "rate /s", &mut em.emission_rate, 0.5, 0.0..=10000.0);
            c |= drag_f32(
                ui,
                "per metre",
                &mut em.emission_per_meter,
                0.1,
                0.0..=1000.0,
            );
            c |= drag_u32(ui, "max particles", &mut em.max_particles, 1..=10_000);
            let (mut lo, mut hi) =
                range_pair(em.lifetime, em.lifetime_min, em.lifetime_max, (1.0, 2.0));
            if drag_range_f32(ui, "lifetime s", &mut lo, &mut hi, 0.01, 0.01..=120.0) {
                em.lifetime = Some(pack_range(lo, hi));
                em.lifetime_min = None;
                em.lifetime_max = None;
                c = true;
            }
            c |= drag_f32(
                ui,
                "duration s (0 = ∞)",
                &mut em.duration,
                0.05,
                0.0..=3600.0,
            );
            ui.horizontal(|ui| {
                c |= check(ui, "loop", &mut em.looping);
                c |= check(ui, "autoplay", &mut em.autoplay);
            });
            c |= drag_f32(ui, "start delay s", &mut em.start_delay, 0.05, 0.0..=600.0);
        });
    c
}

fn shape_kind(em: &EmitterDef) -> &'static str {
    match &em.shape {
        ShapeField::Named(n) => match n.as_str() {
            "sphere" => "sphere",
            "cone" => "cone",
            "box" => "box",
            _ => "point",
        },
        ShapeField::Def(d) => match d {
            ShapeDef::Point => "point",
            ShapeDef::Sphere { .. } => "sphere",
            ShapeDef::Cone { .. } => "cone",
            ShapeDef::Box { .. } => "box",
        },
    }
}

/// Normalise the shape into the modern `ShapeDef` form (folding legacy keys).
fn shape_def(em: &EmitterDef) -> ShapeDef {
    match &em.shape {
        ShapeField::Def(d) => *d,
        ShapeField::Named(n) => match n.as_str() {
            "sphere" => ShapeDef::Sphere {
                radius: em.shape_radius.unwrap_or(0.5),
            },
            "cone" => ShapeDef::Cone {
                radius: em.shape_radius.unwrap_or(0.0),
                angle: em.shape_angle.unwrap_or(30.0),
            },
            "box" => ShapeDef::Box {
                extents: em.shape_extents.unwrap_or([0.5, 0.5, 0.5]),
            },
            _ => ShapeDef::Point,
        },
    }
}

fn set_shape(em: &mut EmitterDef, d: ShapeDef) {
    em.shape = ShapeField::Def(d);
    em.shape_radius = None;
    em.shape_angle = None;
    em.shape_extents = None;
}

fn section_shape(ui: &mut egui::Ui, em: &mut EmitterDef) -> bool {
    let mut c = false;
    egui::CollapsingHeader::new("Shape")
        .default_open(true)
        .show(ui, |ui| {
            let mut kind = shape_kind(em).to_string();
            if combo_str(ui, "shape", &mut kind, &["point", "sphere", "cone", "box"]) {
                let cur = shape_def(em);
                let new = match kind.as_str() {
                    "sphere" => ShapeDef::Sphere {
                        radius: match cur {
                            ShapeDef::Cone { radius, .. } if radius > 0.0 => radius,
                            _ => 0.5,
                        },
                    },
                    "cone" => ShapeDef::Cone {
                        radius: match cur {
                            ShapeDef::Sphere { radius } => radius,
                            _ => 0.0,
                        },
                        angle: 30.0,
                    },
                    "box" => ShapeDef::Box {
                        extents: [0.5, 0.5, 0.5],
                    },
                    _ => ShapeDef::Point,
                };
                set_shape(em, new);
                c = true;
            }
            let mut d = shape_def(em);
            let mut dc = false;
            match &mut d {
                ShapeDef::Point => {}
                ShapeDef::Sphere { radius } => {
                    dc |= drag_f32(ui, "radius", radius, 0.01, 0.0..=100.0);
                }
                ShapeDef::Cone { radius, angle } => {
                    dc |= drag_f32(ui, "disc radius", radius, 0.01, 0.0..=100.0);
                    dc |= drag_f32(ui, "angle °", angle, 0.5, 0.0..=180.0);
                }
                ShapeDef::Box { extents } => {
                    dc |= drag_xyz(ui, "half extents", extents, 0.01, 0.0..=100.0);
                    dc |= drag_xyz(ui, "axis u", &mut em.shape_axis_u, 0.01, -1.0..=1.0);
                    dc |= drag_xyz(ui, "axis v", &mut em.shape_axis_v, 0.01, -1.0..=1.0);
                }
            }
            if dc {
                set_shape(em, d);
                c = true;
            }
            c |= drag_xyz(ui, "offset", &mut em.shape_offset, 0.01, -100.0..=100.0);
            let mut local = em.local_axes.unwrap_or(true);
            if check(ui, "local axes (rotate with entity)", &mut local) {
                em.local_axes = Some(local);
                c = true;
            }
        });
    c
}

fn force_kind(f: &ForceDef) -> &'static str {
    match f {
        ForceDef::Wind { .. } => "wind",
        ForceDef::Drag { .. } => "drag",
        ForceDef::Noise { .. } => "noise",
        ForceDef::Vortex { .. } => "vortex",
        ForceDef::Attractor { .. } => "attractor",
    }
}

fn default_force(kind: &str) -> ForceDef {
    match kind {
        "wind" => ForceDef::Wind {
            velocity: [1.0, 0.0, 0.0],
            strength: 1.0,
        },
        "drag" => ForceDef::Drag { coefficient: 0.5 },
        "vortex" => ForceDef::Vortex {
            center: [0.0; 3],
            axis: [0.0, 1.0, 0.0],
            strength: 2.0,
            falloff: 0.5,
        },
        "attractor" => ForceDef::Attractor {
            position: [0.0, 1.0, 0.0],
            strength: 2.0,
            radius: 0.0,
        },
        _ => ForceDef::Noise {
            strength: 1.0,
            frequency: 1.0,
            speed: 0.5,
            octaves: 1,
        },
    }
}

fn section_motion(ui: &mut egui::Ui, em: &mut EmitterDef) -> bool {
    let mut c = false;
    egui::CollapsingHeader::new("Motion & Forces")
        .default_open(true)
        .show(ui, |ui| {
            let (mut lo, mut hi) = range_pair(em.speed, em.speed_min, em.speed_max, (1.0, 3.0));
            if drag_range_f32(ui, "speed", &mut lo, &mut hi, 0.05, 0.0..=1000.0) {
                em.speed = Some(pack_range(lo, hi));
                em.speed_min = None;
                em.speed_max = None;
                c = true;
            }
            c |= drag_xyz(ui, "direction", &mut em.direction, 0.01, -1.0..=1.0);
            c |= drag_f32(ui, "spread °", &mut em.spread, 0.5, 0.0..=180.0);
            c |= drag_xyz(ui, "gravity", &mut em.gravity, 0.05, -100.0..=100.0);
            c |= drag_f32(ui, "damping /s", &mut em.damping, 0.01, 0.0..=20.0);
            c |= drag_f32(
                ui,
                "inherit velocity",
                &mut em.inherit_velocity,
                0.01,
                -2.0..=2.0,
            );
            c |= check(
                ui,
                "world space (particles detach from emitter)",
                &mut em.world_space,
            );

            ui.add_space(4.0);
            ui.label(egui::RichText::new("Forces").small().weak());
            let mut remove: Option<usize> = None;
            for (i, f) in em.forces.iter_mut().enumerate() {
                ui.push_id(("force", i), |ui| {
                    egui::Frame::group(ui.style()).show(ui, |ui| {
                        ui.horizontal(|ui| {
                            let mut kind = force_kind(f).to_string();
                            if combo_str(
                                ui,
                                "",
                                &mut kind,
                                &["wind", "drag", "noise", "vortex", "attractor"],
                            ) {
                                *f = default_force(&kind);
                                c = true;
                            }
                            ui.with_layout(
                                egui::Layout::right_to_left(egui::Align::Center),
                                |ui| {
                                    if ui.small_button("x").clicked() {
                                        remove = Some(i);
                                    }
                                },
                            );
                        });
                        match f {
                            ForceDef::Wind { velocity, strength } => {
                                c |= drag_xyz(ui, "velocity", velocity, 0.05, -100.0..=100.0);
                                c |= drag_f32(ui, "strength", strength, 0.05, 0.0..=50.0);
                            }
                            ForceDef::Drag { coefficient } => {
                                c |= drag_f32(ui, "coefficient", coefficient, 0.01, 0.0..=50.0);
                            }
                            ForceDef::Noise {
                                strength,
                                frequency,
                                speed,
                                octaves,
                            } => {
                                c |= drag_f32(ui, "strength", strength, 0.05, 0.0..=100.0);
                                c |= drag_f32(ui, "frequency", frequency, 0.02, 0.01..=50.0);
                                c |= drag_f32(ui, "speed", speed, 0.02, 0.0..=20.0);
                                c |= drag_u32(ui, "octaves", octaves, 1..=6);
                            }
                            ForceDef::Vortex {
                                center,
                                axis,
                                strength,
                                falloff,
                            } => {
                                c |= drag_xyz(ui, "center", center, 0.02, -100.0..=100.0);
                                c |= drag_xyz(ui, "axis", axis, 0.01, -1.0..=1.0);
                                c |= drag_f32(ui, "strength", strength, 0.05, -100.0..=100.0);
                                c |= drag_f32(ui, "falloff", falloff, 0.02, 0.0..=50.0);
                            }
                            ForceDef::Attractor {
                                position,
                                strength,
                                radius,
                            } => {
                                c |= drag_xyz(ui, "position", position, 0.02, -100.0..=100.0);
                                c |= drag_f32(
                                    ui,
                                    "strength (- repels)",
                                    strength,
                                    0.05,
                                    -100.0..=100.0,
                                );
                                c |= drag_f32(ui, "radius (0 = ∞)", radius, 0.05, 0.0..=1000.0);
                            }
                        }
                    });
                });
            }
            if let Some(i) = remove {
                em.forces.remove(i);
                c = true;
            }
            ui.menu_button("+ force...", |ui| {
                for kind in ["wind", "drag", "noise", "vortex", "attractor"] {
                    if ui.button(kind).clicked() {
                        em.forces.push(default_force(kind));
                        c = true;
                        ui.close_menu();
                    }
                }
            });
        });
    c
}

/// Keys of a size curve, folding legacy scalar keys.
fn size_keys(em: &EmitterDef) -> (Vec<(f32, [f32; 2])>, Interp) {
    let curve = match &em.size {
        Some(def) => def.to_curve().ok(),
        None => None,
    }
    .unwrap_or_else(|| {
        let s = em.size_start.unwrap_or(0.1);
        let e = em.size_end.unwrap_or(0.0);
        Curve::start_end([s, s], [e, e])
    });
    (curve.keys().to_vec(), curve.interp())
}

fn color_keys(em: &EmitterDef) -> (Vec<(f32, [f32; 4])>, Interp) {
    let curve = match &em.color {
        Some(def) => def.to_curve().ok(),
        None => None,
    }
    .unwrap_or_else(|| {
        Curve::start_end(
            em.color_start.unwrap_or([1.0, 1.0, 1.0, 1.0]),
            em.color_end.unwrap_or([1.0, 1.0, 1.0, 0.0]),
        )
    });
    (curve.keys().to_vec(), curve.interp())
}

fn scalar_keys(def: &Option<CurveDef<f32>>) -> Option<(Vec<[f32; 2]>, Interp)> {
    let curve = def.as_ref()?.to_curve().ok()?;
    Some((
        curve.keys().iter().map(|(t, v)| [*t, *v]).collect(),
        curve.interp(),
    ))
}

fn interp_combo(ui: &mut egui::Ui, label: &str, interp: &mut Interp) -> bool {
    let mut s = match interp {
        Interp::Linear => "linear",
        Interp::Smooth => "smooth",
        Interp::Step => "step",
    }
    .to_string();
    if combo_str(ui, label, &mut s, &["linear", "smooth", "step"]) {
        *interp = match s.as_str() {
            "smooth" => Interp::Smooth,
            "step" => Interp::Step,
            _ => Interp::Linear,
        };
        return true;
    }
    false
}

fn section_lifetime(ui: &mut egui::Ui, em: &mut EmitterDef, idx: usize) -> bool {
    let mut c = false;
    egui::CollapsingHeader::new("Over Lifetime")
        .default_open(true)
        .show(ui, |ui| {
            // --- Size ---
            let (keys, mut interp) = size_keys(em);
            let aspect = keys
                .first()
                .map(|(_, v)| if v[0].abs() > 1e-6 { v[1] / v[0] } else { 1.0 })
                .unwrap_or(1.0);
            let mut w_keys: Vec<[f32; 2]> = keys.iter().map(|(t, v)| [*t, v[0]]).collect();
            let max_v = w_keys.iter().map(|k| k[1]).fold(0.2f32, f32::max) * 1.25;
            ui.horizontal(|ui| {
                ui.label("size (width)");
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if interp_combo(ui, "", &mut interp) {
                        c = true;
                        let curve = Curve::from_keys(keys.clone(), interp).ok();
                        if let Some(cv) = curve {
                            em.size = Some(CurveDef::from_curve(&cv));
                            em.size_start = None;
                            em.size_end = None;
                        }
                    }
                });
            });
            let r = CurveEditor::new(("size", idx), &mut w_keys)
                .range(0.0..=max_v)
                .height(80.0)
                .accent(ACCENT)
                .show(ui);
            let mut new_aspect = aspect;
            let ar = drag_f32(ui, "height / width", &mut new_aspect, 0.01, 0.01..=20.0);
            if r.changed || ar {
                let new_keys: Vec<(f32, [f32; 2])> = w_keys
                    .iter()
                    .map(|k| (k[0], [k[1], k[1] * new_aspect]))
                    .collect();
                if let Ok(cv) = Curve::from_keys(new_keys, interp) {
                    em.size = Some(CurveDef::from_curve(&cv));
                    em.size_start = None;
                    em.size_end = None;
                    c = true;
                }
            }
            c |= edit_range(ui, "size scale", &mut em.size_scale, 0.01, 0.0..=10.0);

            ui.add_space(6.0);
            // --- Colour ---
            let (mut ckeys, mut cinterp) = color_keys(em);
            ui.horizontal(|ui| {
                ui.label("color");
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if interp_combo(ui, "", &mut cinterp) {
                        if let Ok(cv) = Curve::from_keys(ckeys.clone(), cinterp) {
                            em.color = Some(CurveDef::from_curve(&cv));
                            em.color_start = None;
                            em.color_end = None;
                            c = true;
                        }
                    }
                });
            });
            let gr = GradientEditor::new(("color", idx), &mut ckeys).show(ui);
            if gr.changed {
                if let Ok(cv) = Curve::from_keys(ckeys, cinterp) {
                    em.color = Some(CurveDef::from_curve(&cv));
                    em.color_start = None;
                    em.color_end = None;
                    c = true;
                }
            }
            c |= edit_range(ui, "brightness", &mut em.brightness, 0.01, 0.0..=10.0);

            // --- Alpha multiplier (optional) ---
            let mut has_alpha = em.alpha.is_some();
            if check(ui, "alpha curve (multiplies colour alpha)", &mut has_alpha) {
                em.alpha = if has_alpha {
                    Some(CurveDef::Keys {
                        keys: vec![
                            flint_particles::Key { t: 0.0, v: 0.0 },
                            flint_particles::Key { t: 0.2, v: 1.0 },
                            flint_particles::Key { t: 1.0, v: 0.0 },
                        ],
                        interp: Interp::Smooth,
                    })
                } else {
                    None
                };
                c = true;
            }
            if let Some((mut akeys, ainterp)) = scalar_keys(&em.alpha) {
                let r = CurveEditor::new(("alpha", idx), &mut akeys)
                    .range(0.0..=1.0)
                    .height(60.0)
                    .accent(Color32::from_rgb(180, 200, 255))
                    .show(ui);
                if r.changed {
                    let keys: Vec<(f32, f32)> = akeys.iter().map(|k| (k[0], k[1])).collect();
                    if let Ok(cv) = Curve::from_keys(keys, ainterp) {
                        em.alpha = Some(CurveDef::from_curve(&cv));
                        c = true;
                    }
                }
            }

            // --- Speed multiplier (optional) ---
            let mut has_speed = em.speed_curve.is_some();
            if check(ui, "speed curve (multiplies velocity)", &mut has_speed) {
                em.speed_curve = if has_speed {
                    Some(CurveDef::StartEnd {
                        start: 1.0,
                        end: 0.2,
                    })
                } else {
                    None
                };
                c = true;
            }
            if let Some((mut skeys, sinterp)) = scalar_keys(&em.speed_curve) {
                let r = CurveEditor::new(("speed", idx), &mut skeys)
                    .range(0.0..=3.0)
                    .height(60.0)
                    .accent(Color32::from_rgb(160, 235, 180))
                    .show(ui);
                if r.changed {
                    let keys: Vec<(f32, f32)> = skeys.iter().map(|k| (k[0], k[1])).collect();
                    if let Ok(cv) = Curve::from_keys(keys, sinterp) {
                        em.speed_curve = Some(CurveDef::from_curve(&cv));
                        c = true;
                    }
                }
            }

            ui.add_space(4.0);
            // --- Rotation ---
            let (mut rlo, mut rhi) = match (em.rotation_min, em.rotation_max) {
                (None, None) => em.rotation.pair(),
                (a, b) => (a.unwrap_or(0.0), b.unwrap_or(360.0)),
            };
            if drag_range_f32(ui, "rotation °", &mut rlo, &mut rhi, 1.0, -720.0..=720.0) {
                em.rotation = pack_range(rlo, rhi);
                em.rotation_min = None;
                em.rotation_max = None;
                c = true;
            }
            let (mut alo, mut ahi) = match (em.angular_velocity_min, em.angular_velocity_max) {
                (None, None) => em.angular_velocity.pair(),
                (a, b) => (a.unwrap_or(0.0), b.unwrap_or(0.0)),
            };
            if drag_range_f32(ui, "spin °/s", &mut alo, &mut ahi, 1.0, -3600.0..=3600.0) {
                em.angular_velocity = pack_range(alo, ahi);
                em.angular_velocity_min = None;
                em.angular_velocity_max = None;
                c = true;
            }
        });
    c
}

fn section_rendering(ui: &mut egui::Ui, em: &mut EmitterDef) -> (bool, bool) {
    let mut c = false;
    let mut browse = false;
    egui::CollapsingHeader::new("Rendering")
        .default_open(true)
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.label("texture");
                if ui
                    .add(egui::TextEdit::singleline(&mut em.texture).desired_width(150.0))
                    .changed()
                {
                    c = true;
                }
                if ui.button("Browse…").clicked() {
                    browse = true;
                }
                if !em.texture.is_empty() && ui.small_button("x").clicked() {
                    em.texture.clear();
                    c = true;
                }
            });
            if em.texture.is_empty() {
                ui.weak("no texture: soft disc");
            }
            let mut blend = em.blend_mode.as_str().to_string();
            if combo_str(
                ui,
                "blend",
                &mut blend,
                &["alpha", "additive", "premultiplied", "multiply"],
            ) {
                if let Some(b) = ParticleBlendMode::parse(&blend) {
                    em.blend_mode = b;
                    c = true;
                }
            }
            let mut sort = match em.sort {
                SortMode::None => "none",
                SortMode::BackToFront => "back_to_front",
                SortMode::YoungestFirst => "youngest_first",
                SortMode::OldestFirst => "oldest_first",
            }
            .to_string();
            if combo_str(
                ui,
                "sort",
                &mut sort,
                &["none", "back_to_front", "youngest_first", "oldest_first"],
            ) {
                em.sort = match sort.as_str() {
                    "back_to_front" => SortMode::BackToFront,
                    "youngest_first" => SortMode::YoungestFirst,
                    "oldest_first" => SortMode::OldestFirst,
                    _ => SortMode::None,
                };
                c = true;
            }
            ui.horizontal(|ui| {
                c |= drag_u32(ui, "frames", &mut em.frames_x, 1..=64);
                c |= drag_u32(ui, "×", &mut em.frames_y, 1..=64);
            });
            ui.horizontal(|ui| {
                c |= check(ui, "animate over life", &mut em.animate_frames);
                c |= check(ui, "random start frame", &mut em.random_start_frame);
            });
            c |= drag_f32(
                ui,
                "frame rate (0 = life)",
                &mut em.frame_rate,
                0.5,
                0.0..=240.0,
            );
            c |= drag_f32(ui, "velocity stretch", &mut em.stretch, 0.001, 0.0..=1.0);
            ui.collapsing("Reserved (soft / fade / fog)", |ui| {
                ui.weak("Parsed and saved; the soft-particle pass lands with the depth-grab work.");
                c |= drag_f32(
                    ui,
                    "soft distance",
                    &mut em.soft_distance,
                    0.01,
                    0.0..=100.0,
                );
                c |= drag_f32(ui, "fade near", &mut em.fade_near, 0.1, 0.0..=10000.0);
                c |= drag_f32(ui, "fade far", &mut em.fade_far, 0.1, 0.0..=10000.0);
                c |= drag_f32(ui, "lighting", &mut em.lighting, 0.01, 0.0..=1.0);
                c |= check(ui, "fog", &mut em.fog);
            });
        });
    (c, browse)
}

fn section_bursts(ui: &mut egui::Ui, em: &mut EmitterDef) -> bool {
    let mut c = false;
    egui::CollapsingHeader::new("Bursts")
        .default_open(!em.bursts.is_empty() || em.burst_count.is_some())
        .show(ui, |ui| {
            // Fold a legacy burst_count into the timeline on first touch.
            if let Some(n) = em.burst_count.take() {
                if n > 0 {
                    em.bursts.insert(
                        0,
                        BurstDef {
                            time: 0.0,
                            count: CountDef::Const(n),
                            cycles: 1,
                            interval: 0.0,
                            probability: 1.0,
                        },
                    );
                }
                c = true;
            }
            let mut remove = None;
            for (i, b) in em.bursts.iter_mut().enumerate() {
                ui.push_id(("burst", i), |ui| {
                    egui::Frame::group(ui.style()).show(ui, |ui| {
                        ui.horizontal(|ui| {
                            c |= drag_f32(ui, "t", &mut b.time, 0.01, 0.0..=3600.0);
                            c |= edit_count(ui, "count", &mut b.count, 10_000);
                            ui.with_layout(
                                egui::Layout::right_to_left(egui::Align::Center),
                                |ui| {
                                    if ui.small_button("x").clicked() {
                                        remove = Some(i);
                                    }
                                },
                            );
                        });
                        ui.horizontal(|ui| {
                            c |= drag_u32(ui, "cycles (0 = ∞)", &mut b.cycles, 0..=100_000);
                            c |= drag_f32(ui, "interval", &mut b.interval, 0.01, 0.0..=3600.0);
                            c |= drag_f32(ui, "p", &mut b.probability, 0.01, 0.0..=1.0);
                        });
                        if b.cycles != 1 && b.interval <= 0.0 {
                            ui.colored_label(
                                Color32::from_rgb(255, 120, 90),
                                "repeating bursts need an interval",
                            );
                        }
                    });
                });
            }
            if let Some(i) = remove {
                em.bursts.remove(i);
                c = true;
            }
            if ui.button("+ burst").clicked() {
                em.bursts.push(BurstDef {
                    time: 0.0,
                    count: CountDef::Const(10),
                    cycles: 1,
                    interval: 0.0,
                    probability: 1.0,
                });
                c = true;
            }
        });
    c
}

fn sub_emitter_ui(
    ui: &mut egui::Ui,
    label: &str,
    slot: &mut Option<SubEmitterDef>,
    siblings: &[String],
    me: usize,
) -> bool {
    let mut c = false;
    let targets: Vec<&str> = siblings
        .iter()
        .enumerate()
        .filter(|(i, _)| *i != me)
        .map(|(_, s)| s.as_str())
        .collect();
    let mut on = slot.is_some();
    if check(ui, label, &mut on) {
        *slot = if on {
            targets.first().map(|t| SubEmitterDef {
                emitter: t.to_string(),
                count: CountDef::Const(1),
                inherit_velocity: 0.0,
            })
        } else {
            None
        };
        c = true;
    }
    if let Some(sub) = slot {
        if targets.is_empty() {
            ui.weak("add another emitter to target");
        } else {
            ui.indent(label, |ui| {
                c |= combo_str(ui, "into", &mut sub.emitter, &targets);
                c |= edit_count(ui, "count", &mut sub.count, 1000);
                c |= drag_f32(
                    ui,
                    "inherit velocity",
                    &mut sub.inherit_velocity,
                    0.01,
                    -2.0..=2.0,
                );
            });
        }
    }
    c
}

fn section_sub_emitters(
    ui: &mut egui::Ui,
    em: &mut EmitterDef,
    siblings: &[String],
    me: usize,
) -> bool {
    let mut c = false;
    egui::CollapsingHeader::new("Sub-emitters")
        .default_open(em.on_death.is_some() || em.on_birth.is_some())
        .show(ui, |ui| {
            c |= sub_emitter_ui(ui, "on death", &mut em.on_death, siblings, me);
            c |= sub_emitter_ui(ui, "on birth", &mut em.on_birth, siblings, me);
        });
    c
}

// ─── Timeline ───────────────────────────────────────────────────────────────

fn draw_timeline(ctx: &egui::Context, effect: &ParticleEffect, f: &Frame, a: &mut UiActions) {
    egui::TopBottomPanel::bottom("timeline")
        .exact_height(58.0)
        .show(ctx, |ui| {
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                if ui.button("|<").on_hover_text("Seek start (Home)").clicked() {
                    a.seek = Some(0.0);
                }
                let play_label = if f.playing { "||" } else { ">" };
                if ui
                    .button(play_label)
                    .on_hover_text("Play / pause (Space)")
                    .clicked()
                {
                    a.toggle_play = true;
                }
                if ui.button("<<").on_hover_text("Restart (R)").clicked() {
                    a.restart = true;
                }
                ui.monospace(format!("{:6.2}s", f.time));

                // Scrub track
                let avail = ui.available_width() - 260.0;
                let (rect, resp) = ui.allocate_exact_size(
                    Vec2::new(avail.max(120.0), 26.0),
                    Sense::click_and_drag(),
                );
                let painter = ui.painter_at(rect);
                let track = Rect::from_min_max(
                    Pos2::new(rect.left() + 6.0, rect.center().y - 4.0),
                    Pos2::new(rect.right() - 6.0, rect.center().y + 4.0),
                );
                painter.rect_filled(track, 3.0, ui.visuals().extreme_bg_color);
                let x_of = |t: f32| {
                    track.left() + (t / f.loop_end.max(1e-3)).clamp(0.0, 1.0) * track.width()
                };
                // Emitter duration spans and burst markers
                for (i, em) in effect.emitters.iter().enumerate() {
                    let row_y = track.top() - 3.0 - 3.0 * (i % 3) as f32;
                    if em.duration > 0.0 {
                        let x0 = x_of(em.start_delay);
                        let x1 = x_of(em.start_delay + em.duration);
                        painter.line_segment(
                            [Pos2::new(x0, row_y), Pos2::new(x1, row_y)],
                            Stroke::new(2.0, ACCENT.linear_multiply(0.5)),
                        );
                    }
                    for b in &em.bursts {
                        let reps = if b.cycles == 0 { 64 } else { b.cycles.min(64) };
                        for k in 0..reps {
                            let t = em.start_delay + b.time + k as f32 * b.interval;
                            if t > f.loop_end || (k > 0 && b.interval <= 0.0) {
                                break;
                            }
                            let x = x_of(t);
                            let tri = vec![
                                Pos2::new(x, track.bottom() + 1.0),
                                Pos2::new(x - 4.0, track.bottom() + 8.0),
                                Pos2::new(x + 4.0, track.bottom() + 8.0),
                            ];
                            painter.add(egui::Shape::convex_polygon(tri, ACCENT, Stroke::NONE));
                        }
                    }
                    if let Some(n) = em.burst_count {
                        if n > 0 {
                            let x = x_of(em.start_delay);
                            painter.circle_filled(Pos2::new(x, track.bottom() + 4.0), 3.0, ACCENT);
                        }
                    }
                }
                // Played portion + playhead
                let px = x_of(f.time);
                painter.rect_filled(
                    Rect::from_min_max(track.min, Pos2::new(px, track.bottom())),
                    3.0,
                    ACCENT.linear_multiply(0.35),
                );
                painter.line_segment(
                    [
                        Pos2::new(px, rect.top() + 2.0),
                        Pos2::new(px, rect.bottom() - 2.0),
                    ],
                    Stroke::new(2.0, Color32::WHITE),
                );
                if resp.dragged() || resp.clicked() {
                    if let Some(p) = resp.interact_pointer_pos() {
                        let t = ((p.x - track.left()) / track.width()).clamp(0.0, 1.0) * f.loop_end;
                        a.seek = Some(t);
                    }
                }
                if let Some(p) = resp.hover_pos() {
                    let t = ((p.x - track.left()) / track.width()).clamp(0.0, 1.0) * f.loop_end;
                    resp.clone().on_hover_text(format!("{t:.2}s"));
                }

                let mut loop_end = f.loop_end;
                if ui
                    .add(
                        egui::DragValue::new(&mut loop_end)
                            .speed(0.05)
                            .range(0.1..=600.0)
                            .suffix(" s"),
                    )
                    .on_hover_text("Preview length")
                    .changed()
                {
                    a.set_loop_end = Some(loop_end);
                }
                let mut looping = f.looping;
                if ui.checkbox(&mut looping, "loop").changed() {
                    a.set_loop = Some(looping);
                }
                let mut speed = f.speed;
                if ui
                    .add(
                        egui::DragValue::new(&mut speed)
                            .speed(0.05)
                            .range(0.05..=8.0)
                            .suffix("×"),
                    )
                    .on_hover_text("Playback speed ([ / ])")
                    .changed()
                {
                    a.set_speed = Some(speed);
                }
            });
        });
}

// ─── Overlays ───────────────────────────────────────────────────────────────

fn overlay_frame() -> egui::Frame {
    egui::Frame::none()
        .fill(Color32::from_rgba_unmultiplied(0, 0, 0, 170))
        .stroke(Stroke::new(
            1.0,
            Color32::from_rgba_unmultiplied(255, 255, 255, 30),
        ))
        .rounding(egui::Rounding::same(6.0))
        .inner_margin(egui::Margin::same(10.0))
}

fn draw_overlays(ctx: &egui::Context, effect: &ParticleEffect, f: &Frame, a: &mut UiActions) {
    // The viewport: whatever the menu bar, side panel and timeline left over.
    let view = ctx.available_rect();
    egui::Area::new(egui::Id::new("stats_overlay"))
        .fixed_pos(view.left_top() + egui::vec2(10.0, 10.0))
        .order(egui::Order::Foreground)
        .show(ctx, |ui| {
            overlay_frame().show(ui, |ui| {
                ui.style_mut().override_font_id = Some(egui::FontId::monospace(11.0));
                ui.colored_label(
                    Color32::from_gray(150),
                    egui::RichText::new("PREVIEW").size(9.0),
                );
                ui.label(format!(
                    "alive {:>5}   step {:.2} ms",
                    f.alive_total, f.step_ms
                ));
                for (i, em) in effect.emitters.iter().enumerate() {
                    let (alive, cap) = f.per_emitter.get(i).copied().unwrap_or((0, 0));
                    ui.horizontal(|ui| {
                        let (r, _) = ui.allocate_exact_size(Vec2::splat(8.0), Sense::hover());
                        ui.painter().circle_filled(r.center(), 4.0, first_color(em));
                        ui.label(format!(
                            "{:<14} {:>4}/{:<4}",
                            trunc(&em.name, 14),
                            alive,
                            cap
                        ));
                    });
                }
            });
        });

    egui::Area::new(egui::Id::new("view_overlay"))
        .pivot(egui::Align2::RIGHT_TOP)
        .fixed_pos(view.right_top() + egui::vec2(-10.0, 10.0))
        .order(egui::Order::Foreground)
        .show(ctx, |ui| {
            overlay_frame().show(ui, |ui| {
                ui.colored_label(
                    Color32::from_gray(150),
                    egui::RichText::new("VIEW").size(9.0),
                );
                let mut grid = f.show_grid;
                if ui.checkbox(&mut grid, "grid  (G)").changed() {
                    a.toggle_grid = true;
                }
                let mut gz = f.show_gizmos;
                if ui.checkbox(&mut gz, "shape gizmos  (X)").changed() {
                    a.toggle_gizmos = true;
                }
                let mut orbit = f.auto_orbit;
                if ui.checkbox(&mut orbit, "auto-orbit  (O)").changed() {
                    a.toggle_orbit = true;
                }
                if ui
                    .button(format!("backdrop: {}  (B)", f.backdrop))
                    .clicked()
                {
                    a.cycle_backdrop = true;
                }
            });
        });

    if let Some(msg) = &f.status {
        egui::Area::new(egui::Id::new("status_overlay"))
            .pivot(egui::Align2::LEFT_BOTTOM)
            .fixed_pos(view.left_bottom() + egui::vec2(10.0, -10.0))
            .order(egui::Order::Foreground)
            .show(ctx, |ui| {
                overlay_frame().show(ui, |ui| {
                    ui.label(msg);
                });
            });
    }
}

fn trunc(s: &str, n: usize) -> String {
    if s.chars().count() <= n {
        s.to_string()
    } else {
        let mut out: String = s.chars().take(n - 1).collect();
        out.push('…');
        out
    }
}
