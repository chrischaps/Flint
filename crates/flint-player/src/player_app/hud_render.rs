//! Script-driven UI rendering via egui layer painter.

use super::fonts::{resolve_font_id, text_layout_job};
use flint_script::context::{DrawCommand, DrawItem};
use flint_script::draw2d::{ring_indices, ring_points, ring_steps};
use std::collections::{HashMap, HashSet};

pub(super) fn to_color32(c: &[f32; 4]) -> egui::Color32 {
    egui::Color32::from_rgba_unmultiplied(
        (c[0] * 255.0) as u8,
        (c[1] * 255.0) as u8,
        (c[2] * 255.0) as u8,
        (c[3] * 255.0) as u8,
    )
}

fn pos2(p: &[f32; 2]) -> egui::Pos2 {
    egui::Pos2::new(p[0], p[1])
}

/// Render script-issued 2D draw commands via egui layer painter.
/// Uses `ctx.layer_painter()` directly instead of `egui::Area` to avoid
/// zero-size clipping when only painter calls are used (no widgets).
pub(super) fn render_draw_commands(
    ctx: &egui::Context,
    commands: &[DrawItem],
    ui_textures: &HashMap<String, egui::TextureHandle>,
    ui_fonts: &HashSet<String>,
    ui_fonts_warned: &mut HashSet<String>,
) {
    if commands.is_empty() {
        return;
    }

    // Sort by layer (stable sort preserves insertion order within same layer)
    let mut sorted: Vec<&DrawItem> = commands.iter().collect();
    sorted.sort_by_key(|item| item.layer());

    // Paint into the SAME layer egui panels use (`LayerId::background()`),
    // and rely on the caller issuing this before any panel `show()`: shapes
    // in one layer draw in insertion order, so debug panels land on top of
    // script UI (title card, HUD). Distinct layers within the same Order
    // composite in hash-map order — not deterministic — so don't split this
    // into its own layer.
    let base_painter = ctx.layer_painter(egui::LayerId::background());
    let screen = ctx.screen_rect();

    for item in &sorted {
        // A clipped item paints through a painter whose clip rect is the
        // stamped rect intersected with the screen; everything else shares
        // the base painter. Clipping is per item, so it composes with the
        // layer sort.
        let clipped;
        let painter = match item.clip {
            Some([x, y, w, h]) => {
                let rect = egui::Rect::from_min_size(
                    egui::Pos2::new(x, y),
                    egui::Vec2::new(w.max(0.0), h.max(0.0)),
                )
                .intersect(screen);
                clipped = base_painter.with_clip_rect(rect);
                &clipped
            }
            None => &base_painter,
        };
        let cmd = &item.cmd;
        match cmd {
            DrawCommand::Text {
                x,
                y,
                text,
                size,
                color,
                align,
                stroke,
                font,
                letter_spacing,
                shadow,
                ..
            } => {
                let anchor = match align {
                    1 => egui::Align2::CENTER_TOP,
                    2 => egui::Align2::RIGHT_TOP,
                    _ => egui::Align2::LEFT_TOP,
                };
                let font_id = resolve_font_id(*size, font.as_deref(), ui_fonts, ui_fonts_warned);
                let main = to_color32(color);
                // Galleys bake their section colour, so shadow / stroke /
                // main each get their own (the galley cache makes repeats
                // cheap). Position comes from the main galley's size.
                let galley = ctx.fonts(|f| {
                    f.layout_job(text_layout_job(
                        text,
                        font_id.clone(),
                        main,
                        *letter_spacing,
                    ))
                });
                let rect = anchor.anchor_size(egui::Pos2::new(*x, *y), galley.size());
                let pos = rect.min;

                if let Some((shadow_color, dx, dy)) = shadow {
                    let sc = to_color32(shadow_color);
                    let shadow_galley = ctx.fonts(|f| {
                        f.layout_job(text_layout_job(text, font_id.clone(), sc, *letter_spacing))
                    });
                    painter.galley(egui::Pos2::new(pos.x + dx, pos.y + dy), shadow_galley, sc);
                }

                // Draw stroke (outline) by rendering the text at 8 compass offsets
                if let Some((stroke_color, stroke_width)) = stroke {
                    let sc = to_color32(stroke_color);
                    let stroke_galley = ctx.fonts(|f| {
                        f.layout_job(text_layout_job(text, font_id.clone(), sc, *letter_spacing))
                    });
                    let w = *stroke_width;
                    for &(dx, dy) in &[
                        (-w, 0.0),
                        (w, 0.0),
                        (0.0, -w),
                        (0.0, w),
                        (-w, -w),
                        (w, -w),
                        (-w, w),
                        (w, w),
                    ] {
                        painter.galley(
                            egui::Pos2::new(pos.x + dx, pos.y + dy),
                            stroke_galley.clone(),
                            sc,
                        );
                    }
                }

                painter.galley(pos, galley, main);
            }

            DrawCommand::RectFilled {
                x,
                y,
                w,
                h,
                color,
                rounding,
                ..
            } => {
                let rect =
                    egui::Rect::from_min_size(egui::Pos2::new(*x, *y), egui::Vec2::new(*w, *h));
                painter.rect_filled(rect, *rounding, to_color32(color));
            }

            DrawCommand::RectOutline {
                x,
                y,
                w,
                h,
                color,
                thickness,
                rounding,
                ..
            } => {
                let rect =
                    egui::Rect::from_min_size(egui::Pos2::new(*x, *y), egui::Vec2::new(*w, *h));
                painter.rect_stroke(
                    rect,
                    *rounding,
                    egui::Stroke::new(*thickness, to_color32(color)),
                );
            }

            DrawCommand::RectRounded4 {
                x,
                y,
                w,
                h,
                color,
                rounding,
                ..
            } => {
                let rect =
                    egui::Rect::from_min_size(egui::Pos2::new(*x, *y), egui::Vec2::new(*w, *h));
                // Script order is tl, tr, br, bl; egui names them by compass.
                let r = egui::Rounding {
                    nw: rounding[0],
                    ne: rounding[1],
                    se: rounding[2],
                    sw: rounding[3],
                };
                painter.rect_filled(rect, r, to_color32(color));
            }

            DrawCommand::RectGradient {
                x,
                y,
                w,
                h,
                color_a,
                color_b,
                vertical,
                ..
            } => {
                // epaint `Vertex.color` is premultiplied sRGBA;
                // `from_rgba_unmultiplied` (via to_color32) premultiplies.
                let ca = to_color32(color_a);
                let cb = to_color32(color_b);
                let (x0, y0, x1, y1) = (*x, *y, *x + *w, *y + *h);
                // Corners: top-left, top-right, bottom-right, bottom-left.
                let corners = [(x0, y0), (x1, y0), (x1, y1), (x0, y1)];
                let colors = if *vertical {
                    [ca, ca, cb, cb]
                } else {
                    [ca, cb, cb, ca]
                };
                let mut mesh = egui::Mesh::default();
                for ((px, py), col) in corners.iter().zip(colors) {
                    mesh.colored_vertex(egui::Pos2::new(*px, *py), col);
                }
                mesh.add_triangle(0, 1, 2);
                mesh.add_triangle(0, 2, 3);
                painter.add(egui::Shape::mesh(mesh));
            }

            DrawCommand::Ring {
                cx,
                cy,
                r_inner,
                r_outer,
                start_deg,
                end_deg,
                color,
                ..
            } => {
                let steps = ring_steps(*start_deg, *end_deg);
                if *end_deg <= *start_deg || *r_outer <= 0.0 {
                    continue;
                }
                let pts = ring_points(*cx, *cy, *r_inner, *r_outer, *start_deg, *end_deg, steps);
                let col = to_color32(color);
                let mut mesh = egui::Mesh::default();
                for (px, py) in pts {
                    mesh.colored_vertex(egui::Pos2::new(px, py), col);
                }
                mesh.indices = ring_indices(steps);
                painter.add(egui::Shape::mesh(mesh));
            }

            DrawCommand::Polygon { points, color, .. } => {
                let pts: Vec<egui::Pos2> = points.iter().map(pos2).collect();
                painter.add(egui::Shape::convex_polygon(
                    pts,
                    to_color32(color),
                    egui::Stroke::NONE,
                ));
            }

            DrawCommand::PolygonOutline {
                points,
                color,
                thickness,
                closed,
                ..
            } => {
                let pts: Vec<egui::Pos2> = points.iter().map(pos2).collect();
                let stroke = egui::Stroke::new(*thickness, to_color32(color));
                if *closed {
                    painter.add(egui::Shape::closed_line(pts, stroke));
                } else {
                    painter.add(egui::Shape::line(pts, stroke));
                }
            }

            DrawCommand::CircleFilled {
                x,
                y,
                radius,
                color,
                ..
            } => {
                painter.circle_filled(egui::Pos2::new(*x, *y), *radius, to_color32(color));
            }

            DrawCommand::CircleOutline {
                x,
                y,
                radius,
                color,
                thickness,
                ..
            } => {
                painter.circle_stroke(
                    egui::Pos2::new(*x, *y),
                    *radius,
                    egui::Stroke::new(*thickness, to_color32(color)),
                );
            }

            DrawCommand::Line {
                x1,
                y1,
                x2,
                y2,
                color,
                thickness,
                ..
            } => {
                painter.line_segment(
                    [egui::Pos2::new(*x1, *y1), egui::Pos2::new(*x2, *y2)],
                    egui::Stroke::new(*thickness, to_color32(color)),
                );
            }

            DrawCommand::Sprite {
                x,
                y,
                w,
                h,
                name,
                uv,
                tint,
                ..
            } => {
                if let Some(tex_handle) = ui_textures.get(name.as_str()) {
                    let rect =
                        egui::Rect::from_min_size(egui::Pos2::new(*x, *y), egui::Vec2::new(*w, *h));
                    let uv_rect = egui::Rect::from_min_max(
                        egui::Pos2::new(uv[0], uv[1]),
                        egui::Pos2::new(uv[2], uv[3]),
                    );
                    painter.image(tex_handle.id(), rect, uv_rect, to_color32(tint));
                }
            }
        }
    }
}
