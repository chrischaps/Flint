//! Script-driven UI rendering via egui layer painter.

use flint_script::context::DrawCommand;
use std::collections::HashMap;

pub(super) fn to_color32(c: &[f32; 4]) -> egui::Color32 {
    egui::Color32::from_rgba_unmultiplied(
        (c[0] * 255.0) as u8,
        (c[1] * 255.0) as u8,
        (c[2] * 255.0) as u8,
        (c[3] * 255.0) as u8,
    )
}

/// Render script-issued 2D draw commands via egui layer painter.
/// Uses `ctx.layer_painter()` directly instead of `egui::Area` to avoid
/// zero-size clipping when only painter calls are used (no widgets).
pub(super) fn render_draw_commands(
    ctx: &egui::Context,
    commands: &[DrawCommand],
    ui_textures: &HashMap<String, egui::TextureHandle>,
) {
    if commands.is_empty() {
        return;
    }

    // Sort by layer (stable sort preserves insertion order within same layer)
    let mut sorted: Vec<&DrawCommand> = commands.iter().collect();
    sorted.sort_by_key(|cmd| cmd.layer());

    let layer_id = egui::LayerId::new(egui::Order::Foreground, egui::Id::new("script_ui_overlay"));
    let painter = ctx.layer_painter(layer_id);

    for cmd in &sorted {
        match cmd {
            DrawCommand::Text {
                x,
                y,
                text,
                size,
                color,
                align,
                stroke,
                ..
            } => {
                let anchor = match align {
                    1 => egui::Align2::CENTER_TOP,
                    2 => egui::Align2::RIGHT_TOP,
                    _ => egui::Align2::LEFT_TOP,
                };
                let font = egui::FontId::proportional(*size);
                let pos = egui::Pos2::new(*x, *y);

                // Draw stroke (outline) by rendering text at 8 compass offsets
                if let Some((stroke_color, stroke_width)) = stroke {
                    let sc = to_color32(stroke_color);
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
                        painter.text(
                            egui::Pos2::new(pos.x + dx, pos.y + dy),
                            anchor,
                            text,
                            font.clone(),
                            sc,
                        );
                    }
                }

                painter.text(pos, anchor, text, font, to_color32(color));
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
                ..
            } => {
                let rect =
                    egui::Rect::from_min_size(egui::Pos2::new(*x, *y), egui::Vec2::new(*w, *h));
                painter.rect_stroke(rect, 0.0, egui::Stroke::new(*thickness, to_color32(color)));
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
