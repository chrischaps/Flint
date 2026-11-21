//! Drawing a frame: the passes in order (moved from `ascii.rs`, #742).

use crate::gpu_counts::{CountedDevice, CountedQueue};
use super::*;

impl Ascii {
    /// The glyph pass's picture and mask: the mask while any per-entity
    /// variant is in the table, else the one-pixel stand-in.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn entity_bind(
        &self,
        frame: &StyleFrame<'_>,
        input: &StyleInput<'_>,
        with_mask: bool,
        edge_cells: &Owned,
        edge_slots: &Owned,
        stats: &Owned,
        variant_cells: [&Owned; 2],
        dissolve_ranks: &Owned,
        snap_owners: &Owned,
    ) -> Arc<wgpu::BindGroup> {
        let mask = frame.mask.filter(|_| with_mask);
        let make = |mask: &wgpu::TextureView| {
            frame.device.create_bind_group_counted(&wgpu::BindGroupDescriptor {
                label: Some("ASCII Entity BG"),
                layout: &self.entity_bgl,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: wgpu::BindingResource::TextureView(input.view),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: wgpu::BindingResource::TextureView(mask),
                    },
                    wgpu::BindGroupEntry {
                        binding: 2,
                        resource: wgpu::BindingResource::TextureView(&edge_cells.view),
                    },
                    wgpu::BindGroupEntry {
                        binding: 3,
                        resource: wgpu::BindingResource::TextureView(&stats.view),
                    },
                    wgpu::BindGroupEntry {
                        binding: 4,
                        resource: wgpu::BindingResource::TextureView(&variant_cells[0].view),
                    },
                    wgpu::BindGroupEntry {
                        binding: 5,
                        resource: wgpu::BindingResource::TextureView(&variant_cells[1].view),
                    },
                    wgpu::BindGroupEntry {
                        binding: 6,
                        resource: wgpu::BindingResource::TextureView(&dissolve_ranks.view),
                    },
                    wgpu::BindGroupEntry {
                        binding: 7,
                        resource: wgpu::BindingResource::TextureView(&edge_slots.view),
                    },
                    wgpu::BindGroupEntry {
                        binding: 8,
                        resource: wgpu::BindingResource::TextureView(&snap_owners.view),
                    },
                ],
            })
        };
        // The edge cells are remade when the grid changes size: their
        // generation is part of the key. Hashed, not shifted into fields: a
        // generation that outgrew its field wrapped onto an old key and
        // bound the texture that one had (#398's ranks did after 32
        // dissolves toggled on and off)
        let key = bind_key(&[
            edge_cells.generation,
            edge_slots.generation,
            stats.generation,
            variant_cells[0].generation,
            dissolve_ranks.generation,
            snap_owners.generation,
            input.id * 2 + mask.is_some() as u64,
        ]);
        frame.bind("ASCII Entity BG", key, || {
            make(mask.unwrap_or(&self.no_mask_view))
        })
    }


    /// The variant table's rows and slots written to its buffers (grown to
    /// fit, #336), and the bind groups over them: the uniform's group 0 of
    /// every ascii_common.wgsl pass, the edge cell pass's and the edge
    /// view's, and the width the tone statistics give a column each row.
    pub(super) fn upload_table(
        &self,
        frame: &StyleFrame<'_>,
        rows: &[VariantUniform],
        slots: &[u32],
    ) -> (Arc<wgpu::BindGroup>, [Arc<wgpu::BindGroup>; 2], u32) {
        let mut table = self.table.lock().unwrap_or_else(|e| e.into_inner());
        // Every slot is written, so an id a past frame held reads 0 again
        table.upload(frame.device, frame.queue, rows, slots);
        let main = frame.bind("ASCII Table BG", table.generation(), || {
            frame.device.create_bind_group_counted(&wgpu::BindGroupDescriptor {
                label: Some("ASCII Table BG"),
                layout: &self.table_bgl,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: self.uniforms.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: table.rows().as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 2,
                        resource: table.slots().as_entire_binding(),
                    },
                ],
            })
        });
        let edge_table = |name: &'static str, uniforms: &wgpu::Buffer| {
            frame.bind(name, table.generation(), || {
                frame.device.create_bind_group_counted(&wgpu::BindGroupDescriptor {
                    label: Some(name),
                    layout: &self.edge_bgl,
                    entries: &[
                        wgpu::BindGroupEntry {
                            binding: 0,
                            resource: uniforms.as_entire_binding(),
                        },
                        wgpu::BindGroupEntry {
                            binding: 1,
                            resource: table.slots().as_entire_binding(),
                        },
                    ],
                })
            })
        };
        let edge = [
            edge_table("ASCII Edge Cell Table BG", &self.edge_cell_uniforms),
            edge_table("ASCII Edge View Table BG", &self.edge_view_uniforms),
        ];
        let width = table.row_capacity().min(MAX_VARIANTS) as u32;
        (main, edge, width)
    }

    /// Row 0 (the whole frame's, drawn from the cell state) and a row for
    /// each per-entity variant (#266), with the slot of every mask id. A
    /// variant naming a font of its own (#335) adds it to `in_use`.
    pub(super) fn variant_table(
        &self,
        frame: &StyleFrame<'_>,
        fonts: &mut Fonts,
        in_use: &mut FontsInUse,
        ramps: &mut Ramps,
        palettes: &mut Palettes,
        base_ramp: (u32, u32),
    ) -> (Vec<VariantUniform>, Vec<u32>) {
        let mut rows = vec![VariantUniform::zeroed()];
        // A slot for every id up to the highest this style's variants hold
        let mut slots = vec![
            0u32;
            frame
                .variants()
                .map(|(id, _)| id as usize + 1)
                .max()
                .unwrap_or(1)
        ];
        let base = in_use.sets[0].clone();
        let palette_for = |fonts: &mut Fonts, color: &str, name: &str| -> Option<Arc<Palette>> {
            if color != "palette" {
                return None;
            }
            let name = if name.is_empty() {
                DEFAULT_PALETTE
            } else {
                name
            };
            Some(
                fonts
                    .palettes
                    .get(name)
                    .ok()
                    .unwrap_or_else(|| Palette::builtin(DEFAULT_PALETTE).expect("built-in")),
            )
        };
        for (id, v) in frame.variants() {
            // What a cap leaves out is said once, naming the entity (#740)
            let entity = frame.style_ids.entity(id).unwrap_or("?");
            let dropped = |cap: &str, limit: usize, instead: &str| {
                report_drop("ascii", cap, limit, entity, instead);
            };
            if rows.len() == MAX_VARIANTS {
                dropped(
                    "per-entity variants",
                    MAX_VARIANTS - 1,
                    "drawn like the whole frame",
                );
                continue;
            }
            let param = |n: &str| frame.variant_param(v, n);
            let option = |n: &str| frame.variant_option(v, n).to_string();
            // The variant's own font (#335): its ramp and its edge glyphs
            // packed beside the others, its glyph cell its own. One that
            // doesn't fit is drawn in the whole frame's font
            let font_name = option("font");
            let set = self.set_named(fonts, &font_name);
            let own = if Arc::ptr_eq(&set, &base) {
                None
            } else {
                let placed = in_use.index(&set).and_then(|font| {
                    ramps
                        .place_with_edges(font, &set, &option("ramp"))
                        .map(|(ramp, edges)| (set.atlas().cell_size(), ramp, edges))
                });
                if placed.is_none() {
                    dropped(
                        "fonts",
                        MAX_FONTS,
                        &format!(
                            "drawn in `{}`, not `{font_name}` (the frame's fonts also share \
                             {MAX_STACK_TEXELS} atlas rows and {MAX_RAMP} ramp steps)",
                            base.name()
                        ),
                    );
                }
                placed
            };
            let (glyph, ramp, edge_start) = match own {
                Some(placed) => placed,
                None => {
                    let ramp = ramps.place(0, &base, &option("ramp")).unwrap_or_else(|| {
                        dropped(
                            "ramp steps",
                            MAX_RAMP,
                            &format!("drawn with the scene's ramp, not `{}`", option("ramp")),
                        );
                        base_ramp
                    });
                    (base.atlas().cell_size(), ramp, NO_EDGE_START)
                }
            };
            let palette = match palette_for(fonts, &option("color"), &option("palette")) {
                None => PalettePlace::default(),
                Some(p) => palettes.place(&p).unwrap_or_else(|| {
                    dropped(
                        "palette inks",
                        MAX_COLORS,
                        &format!("drawn in the scene's palette, not `{}`", p.name()),
                    );
                    palettes
                        .placed
                        .first()
                        .map(|(_, at)| *at)
                        .unwrap_or_default()
                }),
            };
            let cell = cell_pixels(param("cell"), frame.pixels_per_point, glyph);
            let origin = v.anchor.and_then(|a| frame.project(a)).map_or_else(
                || grid_origin([frame.width, frame.height], cell),
                |at| anchored_origin(at, cell),
            );
            let flags = if v.off {
                PASS_THROUGH
            } else {
                let dissolve = if option("transition") == "dissolve" {
                    DISSOLVE
                } else {
                    0
                };
                let snap = if option("silhouette") == "cell" {
                    SNAP
                } else {
                    0
                };
                dissolve | snap
            };
            slots[id as usize] = rows.len() as u32;
            let mut row = variant_uniform(&param, &option, cell, origin, ramp, &palette, flags);
            row.tint = frame.variant_color(v, "tint");
            row.glyph = [glyph[0] as f32, glyph[1] as f32];
            row.edge_start = edge_start;
            // Its own font's corners (#348); one in the frame's font reads the frame's
            row.corners = corner_glyphs(frame, &set);
            // The row holds the outline in points; the shader wants pixels
            row.outline = outline_pixels(row.outline, frame.pixels_per_point);
            // Its own bounds (#507): the tone statistics and the variant
            // cell pass look for its pixels only there
            row.bounds = rect_corners(frame.variant_rect(id, 0), [frame.width, frame.height]);
            rows.push(row);
        }
        (rows, slots)
    }

    /// What the edge pixels are read with this frame (#743): the camera,
    /// the plain id, the style's thresholds against the shared ones, and
    /// whether the mask's outlines are looked for.
    fn edge_source(&self, frame: &StyleFrame<'_>) -> EdgeSource {
        let min = super::super::edges::MIN_THRESHOLD;
        EdgeSource {
            inv_view_proj: frame.camera.inv_view_proj,
            camera_pos: frame.camera.position,
            plain_id: frame.style_id(STYLE.name).map_or(0, u32::from),
            depth_scale: frame.config.edge_depth.max(min) / frame.param("edge_depth").max(min),
            normal_scale: frame.config.edge_normal.max(min) / frame.param("edge_normal").max(min),
            outlines: frame.mask.is_some() as u32,
            _pad: 0,
        }
    }

    /// The edge pixels' inputs (#743, `ascii_edge_source.wgsl`'s group 1):
    /// the shared edges (or no edge, if their pass couldn't be built), the
    /// mask (or one unstyled pixel) and the depth, kept over all three by
    /// identity (#960).
    fn edge_source_bind(&self, frame: &StyleFrame<'_>) -> Arc<wgpu::BindGroup> {
        let over = [
            frame.edges_or(&self.no_edges_view),
            crate::bind_cache::Bound::View(frame.mask.unwrap_or(&self.no_mask_view)),
        ];
        // With and without the mask kept side by side: it comes and goes
        let variant = frame.mask.is_some() as u64;
        frame.depth_bind_over("ASCII Edge Source BG", variant, &over, || {
            frame.device.create_bind_group_counted(&wgpu::BindGroupDescriptor {
                label: Some("ASCII Edge Source BG"),
                layout: &self.source_bgl,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: wgpu::BindingResource::TextureView(
                            frame
                                .edges
                                .as_ref()
                                .map_or(&self.no_edges_view, |e| &e.view),
                        ),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: wgpu::BindingResource::TextureView(
                            frame.mask.unwrap_or(&self.no_mask_view),
                        ),
                    },
                    wgpu::BindGroupEntry {
                        binding: 2,
                        resource: wgpu::BindingResource::TextureView(frame.depth),
                    },
                ],
            })
        })
    }

    /// Each cell's line glyph from the edge pixels (#267: the shared edges
    /// and the mask's outlines, #743), then the glyph each cell draws,
    /// thinned and cornered, into `edge_slots` (#910).
    #[allow(clippy::too_many_arguments)]
    pub(super) fn encode_edges(
        &self,
        frame: &StyleFrame<'_>,
        encoder: &mut wgpu::CommandEncoder,
        input: &StyleInput<'_>,
        [edge_cells, edge_slots]: [&Owned; 2],
        cell: [f32; 2],
        [table, edge_table]: [&wgpu::BindGroup; 2],
        variants: usize,
        [glyph_rect, edge_area]: [Option<kit::PixelRect>; 2],
        imported: Option<&crate::stylize::history::ImportedHistory>,
    ) {
        let origin = grid_origin([frame.width, frame.height], cell);
        // What the glyph pass reads of the edges inside its rect (#385):
        // the edge cells under it and those its cells' corners reach
        // (#930), and the edge pixels under those cells (the outline
        // pass's)
        let regions_in = |rect: Option<kit::PixelRect>| {
            rect.map(|g| {
                edge_regions(
                    g,
                    cell,
                    origin,
                    [frame.width, frame.height],
                    edge_cells.size,
                    frame.depth_size,
                )
            })
        };
        let regions = regions_in(glyph_rect);
        // The edge cell pass is cut to the same cells with the scissor off
        // (#930): only the cells round the entities hold lines, so last
        // frame's cells and slots, which this frame's start from (#477),
        // are what they are with it on, where the pass never wrote the
        // rest. The glyphs read no cell past them (`EDGE_CORNER_REACH`)
        let kept_cells = regions_in(edge_area).map(|r| r.edge_cells);
        let source = self.edge_source_bind(frame);
        // Last frame's edge cells and slots (#477), copied aside before
        // this frame's passes write over them, when they're its past: the
        // passes keep last frame's decisions until an edge has clearly
        // moved on. Screen space under `temporal = "reproject"` too
        let mut past = self.edge_past.lock().unwrap_or_else(|e| e.into_inner());
        let key = EdgeKey {
            epoch: frame.history_epoch,
            size: [frame.width, frame.height],
            depth_size: frame.depth_size,
            cell: cell.map(f32::to_bits),
            origin: origin.map(f32::to_bits),
            textures: [edge_cells.generation, edge_slots.generation],
        };
        // History a frame capture handed back (#1136): this frame's past
        // when the key is the captured frame's. Written into this frame's
        // textures, which the copy below takes as last frame's
        if let Some((h, part)) = imported.and_then(|h| h.used_part("edges").map(|p| (h, p))) {
            match crate::stylize::history::key_difference(&part.key, &key.exported()) {
                Some(why) => h.say(format!(
                    "ascii: the captured edge history was another frame's ({why}): edges drawn fresh"
                )),
                None => match h
                    .write(frame.queue, part, "cells", &edge_cells.texture)
                    .and_then(|()| h.write(frame.queue, part, "slots", &edge_slots.texture))
                {
                    Ok(()) => {
                        past.last = Some((key.clone(), frame.frame.wrapping_sub(1)));
                        h.say("ascii: edge history taken from the capture");
                    }
                    Err(why) => h.say(format!("ascii: edge history not taken: {why}")),
                },
            }
        }
        let history = past.advance(key, frame.frame, frame.option("temporal") != "off");
        past.used = history;
        let EdgePast { cells, slots, .. } = &mut *past;
        let past_cells = Owned::ensure(
            cells,
            frame.device,
            "ASCII Past Edge Cells",
            EDGE_CELL_FORMAT,
            edge_cells.size,
            &self.edges_in_bgl,
        );
        let past_slots = Owned::ensure(
            slots,
            frame.device,
            "ASCII Past Edge Slots",
            EDGE_SLOT_FORMAT,
            edge_slots.size,
            &self.cells_bgl,
        );
        if history {
            copy_whole(encoder, edge_cells, past_cells);
            copy_whole(encoder, edge_slots, past_slots);
        }
        let hold = frame.param("hysteresis").max(0.0);

        // The mask's outlines (#743), where the glyph pass reads them
        let mut outline_slot = self.outlines.lock().unwrap_or_else(|e| e.into_inner());
        let outlines = frame.mask.is_some().then(|| {
            let target = Owned::ensure(
                &mut outline_slot,
                frame.device,
                "ASCII Edge Outlines",
                super::super::edges::EDGE_FORMAT,
                frame.depth_size,
                &self.edges_in_bgl,
            );
            kit::fullscreen_pass_in(
                encoder,
                "ASCII Edge Outline Pass",
                &target.view,
                target.size,
                &self.outline_pipeline,
                &[edge_table, &source],
                regions.map(|r| r.edge_pixels),
            );
            target
        });
        let past_and_outlines = frame.bind(
            "ASCII Past Cells And Outlines BG",
            bind_key(&[
                past_cells.generation,
                outlines.map_or(u64::MAX, |o| o.generation),
            ]),
            || {
                frame.device.create_bind_group_counted(&wgpu::BindGroupDescriptor {
                    label: Some("ASCII Past Cells And Outlines BG"),
                    layout: &self.past_outline_bgl,
                    entries: &[
                        wgpu::BindGroupEntry {
                            binding: 0,
                            resource: wgpu::BindingResource::TextureView(&past_cells.view),
                        },
                        wgpu::BindGroupEntry {
                            binding: 1,
                            resource: wgpu::BindingResource::TextureView(
                                outlines.map_or(&self.no_edges_view, |o| &o.view),
                            ),
                        },
                    ],
                })
            },
        );

        let cell_uniforms = EdgeCellUniforms {
            src: self.edge_source(frame),
            cell,
            origin,
            size: [frame.width as f32, frame.height as f32],
            threshold: frame.param("edge_threshold").max(0.01),
            max_len: frame.param("edge_max"),
            edge_size: frame.depth_size.map(|n| n as f32),
            srgb: frame.srgb() as u32,
            corner: frame.param("edge_corner").clamp(0.0, 1.0),
            history: history as u32,
            hold,
            smoothing: edge_smoothing(frame.param("smoothing"), frame.dt),
            _pad: 0,
        };
        frame.queue.write_buffer_counted(
            &self.edge_cell_uniforms,
            0,
            bytemuck::bytes_of(&cell_uniforms),
        );
        let thin_uniforms = EdgeThinUniforms {
            history: history as u32,
            // Under half a cell, so the border never reaches a cell's centre
            reach: (hold * EDGE_REACH).min(0.45),
            variants: variants as u32,
            whole_frame: (frame.whole_frame() && frame.option("scope") != "entities") as u32,
            cell,
            origin,
            size: [frame.width as f32, frame.height as f32],
            _pad: [0; 2],
        };
        frame.queue.write_buffer_counted(
            &self.edge_thin_uniforms,
            0,
            bytemuck::bytes_of(&thin_uniforms),
        );
        kit::fullscreen_pass_in(
            encoder,
            "ASCII Edge Cell Pass",
            &edge_cells.view,
            edge_cells.size,
            &self.edge_cell_pipeline,
            &[
                edge_table,
                &source,
                &self.picture_bind(frame, input, false),
                &past_and_outlines,
            ],
            kept_cells,
        );

        // The glyph each cell draws (#910), decided here once a cell
        // rather than by every pixel of it. Over the whole grid: a cell
        // outside the regions reads the edge cells as the glyph pass's
        // pixels once did, and the passes cost next to nothing
        let pad = 2 * EDGE_THIN_PAD;
        let thin_uniform = frame.uniform(
            "ASCII Edge Thin Uniform BG",
            &self.uniform_bgl,
            &self.edge_thin_uniforms,
        );
        let mut thin_slot = self.edge_thin.lock().unwrap_or_else(|e| e.into_inner());
        let thin = Owned::ensure(
            &mut thin_slot,
            frame.device,
            "ASCII Edge Thin",
            EDGE_SLOT_FORMAT,
            [edge_cells.size[0] + pad, edge_cells.size[1] + pad],
            &self.cells_bgl,
        );
        // Last frame's slots and the mask (#493): with the variant table
        // they say which cells the glyph pass draws in this style
        let thin_mask = frame.mask.unwrap_or(&self.no_mask_view);
        let thin_inputs = frame.bind(
            "ASCII Edge Thin Inputs BG",
            bind_key(&[past_slots.generation, frame.mask.is_some() as u64]),
            || {
                frame.device.create_bind_group_counted(&wgpu::BindGroupDescriptor {
                    label: Some("ASCII Edge Thin Inputs BG"),
                    layout: &self.thin_in_bgl,
                    entries: &[
                        wgpu::BindGroupEntry {
                            binding: 0,
                            resource: wgpu::BindingResource::TextureView(&past_slots.view),
                        },
                        wgpu::BindGroupEntry {
                            binding: 1,
                            resource: wgpu::BindingResource::TextureView(thin_mask),
                        },
                    ],
                })
            },
        );
        kit::fullscreen_pass(
            encoder,
            "ASCII Edge Thin Pass",
            &thin.view,
            &self.edge_thin_pipeline,
            &[table, &edge_cells.bind, &thin_uniform, &thin_inputs],
        );
        kit::fullscreen_pass(
            encoder,
            "ASCII Edge Corner Pass",
            &edge_slots.view,
            &self.edge_corner_pipeline,
            &[&thin.bind, &edge_cells.bind, &thin_uniform, &past_slots.bind],
        );
    }

    /// The edge debug view (#349): the edge pixels by source and each
    /// cell's decision, over the picture in grey.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn encode_edge_view(
        &self,
        frame: &StyleFrame<'_>,
        encoder: &mut wgpu::CommandEncoder,
        input: &StyleInput<'_>,
        output: &wgpu::TextureView,
        [edge_cells, edge_slots]: [&Owned; 2],
        cell: [f32; 2],
        origin: [f32; 2],
        corners: u32,
        view_table: &wgpu::BindGroup,
    ) {
        // The outlines the edge passes drew this frame, while the mask is bound
        let outline_slot = self.outlines.lock().unwrap_or_else(|e| e.into_inner());
        let outlines = outline_slot.as_ref().filter(|_| frame.mask.is_some());
        let uniforms = EdgeViewUniforms {
            src: self.edge_source(frame),
            cell,
            origin,
            size: [frame.width as f32, frame.height as f32],
            edge_size: frame.depth_size.map(|n| n as f32),
            threshold: frame.param("edge_threshold").max(0.01),
            max_len: frame.param("edge_max"),
            srgb: if frame.srgb() { 1.0 } else { 0.0 },
            dim: 0.3,
            thick: frame.pixels_per_point.max(1.0).round(),
            corners,
            _pad: [0.0; 2],
        };
        frame
            .queue
            .write_buffer_counted(&self.edge_view_uniforms, 0, bytemuck::bytes_of(&uniforms));
        let cells = frame.bind(
            "ASCII Edge View Cells BG",
            bind_key(&[
                edge_cells.generation,
                edge_slots.generation,
                outlines.map_or(u64::MAX, |o| o.generation),
            ]),
            || {
                frame.device.create_bind_group_counted(&wgpu::BindGroupDescriptor {
                    label: Some("ASCII Edge View Cells BG"),
                    layout: &self.edge_view_cells_bgl,
                    entries: &[
                        wgpu::BindGroupEntry {
                            binding: 0,
                            resource: wgpu::BindingResource::TextureView(&edge_cells.view),
                        },
                        wgpu::BindGroupEntry {
                            binding: 1,
                            resource: wgpu::BindingResource::TextureView(&edge_slots.view),
                        },
                        wgpu::BindGroupEntry {
                            binding: 2,
                            resource: wgpu::BindingResource::TextureView(
                                outlines.map_or(&self.no_edges_view, |o| &o.view),
                            ),
                        },
                    ],
                })
            },
        );
        kit::fullscreen_pass(
            encoder,
            "ASCII Edge View Pass",
            output,
            &self.edge_view_pipeline,
            &[
                view_table,
                &self.edge_source_bind(frame),
                &frame.sampled_input("ASCII Edge View Input BG", &self.input_bgl, input),
                &cells,
            ],
        );
    }

    /// Each slot's tone range into `stats` (#289): per lattice row, then
    /// summed per slot, both from this frame's picture.
    pub(super) fn encode_stats(
        &self,
        frame: &StyleFrame<'_>,
        encoder: &mut wgpu::CommandEncoder,
        input: &StyleInput<'_>,
        uniform: &wgpu::BindGroup,
        with_mask: bool,
        stats: &Owned,
    ) {
        let mut runs_slot = self.stats_runs.lock().unwrap_or_else(|e| e.into_inner());
        let runs = Owned::ensure(
            &mut runs_slot,
            frame.device,
            "ASCII Tone Stat Runs",
            STATS_FORMAT,
            // A column a slot, as wide as the resolved stats
            [stats.size[0], STATS_ROWS * STATS_CHUNKS],
            &self.edges_in_bgl,
        );
        let mut rows_slot = self.stats_rows.lock().unwrap_or_else(|e| e.into_inner());
        let rows = Owned::ensure(
            &mut rows_slot,
            frame.device,
            "ASCII Tone Stat Rows",
            STATS_FORMAT,
            [stats.size[0], STATS_ROWS],
            &self.edges_in_bgl,
        );
        let input_bind = self.picture_bind(frame, input, with_mask);
        kit::fullscreen_pass(
            encoder,
            "ASCII Tone Stat Runs",
            &runs.view,
            &self.stats_pipeline,
            &[uniform, &input_bind],
        );
        kit::fullscreen_pass(
            encoder,
            "ASCII Tone Stat Rows",
            &rows.view,
            &self.stats_rows_pipeline,
            &[uniform, &runs.bind],
        );
        kit::fullscreen_pass(
            encoder,
            "ASCII Tone Stats",
            &stats.view,
            &self.stats_resolve_pipeline,
            &[uniform, &rows.bind],
        );
    }

    /// The picture and (while any per-entity variant is drawn) the mask,
    /// as the tone statistics and the variant cells read them.
    pub(super) fn picture_bind(
        &self,
        frame: &StyleFrame<'_>,
        input: &StyleInput<'_>,
        with_mask: bool,
    ) -> Arc<wgpu::BindGroup> {
        let mask = frame.mask.filter(|_| with_mask);
        let key = input.id * 2 + mask.is_some() as u64;
        frame.bind("ASCII Stats In BG", key, || {
            frame.device.create_bind_group_counted(&wgpu::BindGroupDescriptor {
                label: Some("ASCII Stats In BG"),
                layout: &self.stats_in_bgl,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: wgpu::BindingResource::TextureView(input.view),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: wgpu::BindingResource::TextureView(
                            mask.unwrap_or(&self.no_mask_view),
                        ),
                    },
                ],
            })
        })
    }

    /// Once a frame: the caches follow the scene's folder, and every
    /// [`POLL_FRAMES`] a changed palette or glyph set file clears its cache
    /// (#323). A reloaded set is a new Arc, so the stack re-uploads, and
    /// palettes are packed afresh every frame.
    pub(super) fn refresh_fonts(&self, frame: &StyleFrame<'_>, fonts: &mut Fonts) {
        self.follow_scope(frame, fonts);
        fonts.since_poll += 1;
        if fonts.since_poll >= POLL_FRAMES {
            fonts.since_poll = 0;
            let glyphs = fonts.cache.poll();
            let palettes = fonts.palettes.poll();
            if glyphs || palettes {
                tracing::info!(
                    "ascii: reloading {}",
                    match (glyphs, palettes) {
                        (true, true) => "glyph sets and palettes",
                        (true, false) => "glyph sets",
                        _ => "palettes",
                    }
                );
                fonts.warned = None;
            }
        }
    }

    /// The caches follow the scene's folder.
    fn follow_scope(&self, frame: &StyleFrame<'_>, fonts: &mut Fonts) {
        if fonts.cache.scope() != Some(frame.assets) {
            fonts.cache = GlyphSetCache::new(frame.assets.clone());
            fonts.palettes = PaletteCache::new(frame.assets.clone());
            fonts.stack = None;
        }
    }

    /// The whole frame's grid, as [`encode`](StylePass::encode) lays it:
    /// the cell for the frame's font and its origin.
    fn whole_frame_grid(
        &self,
        frame: &StyleFrame<'_>,
        fonts: &mut Fonts,
    ) -> ([f32; 2], [f32; 2]) {
        let glyph = self
            .set_named(fonts, frame.option("font"))
            .atlas()
            .cell_size();
        let cell = cell_pixels(frame.param("cell"), frame.pixels_per_point, glyph);
        (cell, grid_origin([frame.width, frame.height], cell))
    }

    /// The glyph set `name`, the built-in set when it can't be loaded (the
    /// cache has warned).
    pub(super) fn set_named(&self, fonts: &mut Fonts, name: &str) -> Arc<GlyphSet> {
        fonts
            .cache
            .get(name)
            .unwrap_or_else(|_| GlyphSet::builtin())
    }

    /// The atlases of `in_use` stacked on the GPU, uploaded again only
    /// when the frame's sets change (#335).
    pub(super) fn stack<'f>(
        &self,
        frame: &StyleFrame<'_>,
        fonts: &'f mut Fonts,
        in_use: &FontsInUse,
    ) -> &'f LoadedStack {
        let stale = fonts.stack.as_ref().is_none_or(|l| {
            l.sets.len() != in_use.sets.len()
                || l.sets
                    .iter()
                    .zip(&in_use.sets)
                    .any(|(a, b)| !Arc::ptr_eq(a, b))
        });
        if stale {
            let atlases: Vec<_> = in_use.sets.iter().map(|s| &**s.atlas()).collect();
            let gpu = GpuGlyphStack::upload(frame.device, frame.queue, &atlases, "ASCII Atlas");
            let bind = frame.device.create_bind_group_counted(&wgpu::BindGroupDescriptor {
                label: Some("ASCII Atlas BG"),
                layout: &self.atlas_bgl,
                entries: &[wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&gpu.view),
                }],
            });
            fonts.stack = Some(LoadedStack {
                sets: in_use.sets.clone(),
                gpu,
                bind,
            });
        }
        fonts.stack.as_ref().expect("loaded above")
    }
}

impl StylePass for Ascii {
    /// Its cells' tone and edge history (#277, #477) advance a frame each
    /// draw, so a second draw in one frame isn't a repeat (#1317)
    fn keeps_history(&self) -> bool {
        true
    }

    fn glyph_atlas_bytes(&self) -> u64 {
        // `try_lock`: a frame encoding on another thread holds it, and a
        // report never waits for a frame (a missed reading counts 0)
        self.fonts.try_lock().ok().map_or(0, |fonts| {
            fonts
                .stack
                .as_ref()
                .map_or(0, |s| crate::gpu_counts::bytes_of(&s.gpu.texture))
        })
    }

    fn encode(
        &self,
        frame: &StyleFrame<'_>,
        encoder: &mut wgpu::CommandEncoder,
        input: &StyleInput<'_>,
        output: &wgpu::TextureView,
    ) {
        // History a frame capture handed back (#1136), for this frame only
        let imported = self.imported.take();
        let mut fonts = self.fonts.lock().unwrap_or_else(|e| e.into_inner());
        let ramp_name = frame.option("ramp").to_string();
        self.refresh_fonts(frame, &mut fonts);
        let set = self.set_named(&mut fonts, frame.option("font"));
        let ramp = match set.ramp(&ramp_name) {
            Some(r) => r,
            None => {
                let key = (set.name().to_string(), ramp_name.clone());
                if !ramp_name.is_empty() && fonts.warned.as_ref() != Some(&key) {
                    tracing::warn!(
                        "ascii: glyph set `{}` has no ramp `{ramp_name}`; using `{}` ({})",
                        set.name(),
                        set.default_ramp().name(),
                        set.ramps().map(|r| r.name()).collect::<Vec<_>>().join(", ")
                    );
                    fonts.warned = Some(key);
                }
                set.default_ramp()
            }
        };
        let atlas = set.atlas();
        let glyph = atlas.cell_size();
        let steps = fit_ramp(ramp);
        // Ramps and palettes are packed, the whole frame's first so its
        // steps and inks sit where the cell state expects them
        let mut ramps = Ramps::default();
        let base_ramp = ramps
            .place(0, &set, ramp.name())
            .expect("the first ramp is resampled to fit");
        let palette = if frame.option("color") == "palette" {
            let name = match frame.option("palette") {
                "" => DEFAULT_PALETTE,
                n => n,
            };
            fonts.palettes.get(name).ok()
        } else {
            None
        }
        .unwrap_or_else(|| Palette::builtin(DEFAULT_PALETTE).expect("built-in"));
        let mut palettes = Palettes::default();
        let _ = palettes.place(&palette);
        let mut in_use = FontsInUse::new(set.clone());
        let (mut variants, slots) = self.variant_table(
            frame,
            &mut fonts,
            &mut in_use,
            &mut ramps,
            &mut palettes,
            base_ramp,
        );
        let cell = cell_pixels(frame.param("cell"), frame.pixels_per_point, glyph);
        let size = [frame.width, frame.height];
        let origin = grid_origin(size, cell);
        let grid = cell_grid(size, cell, origin);
        // Each per-entity variant's band of cells (#313)
        let bounded = frame.mask_rect(0).is_some();
        let min_footprint = if bounded {
            MIN_BAND_FOOTPRINT_BOUNDED
        } else {
            MIN_BAND_FOOTPRINT
        };
        let bands = assign_bands(&mut variants, size, min_footprint);
        let snapping = snap_grids(&variants);
        // Each snapping grid's band of cells (#929)
        let owner_bands = assign_owner_bands(&mut variants, size);
        // Each dissolving variant's band of cells (#398)
        let dissolving = assign_dissolve(&mut variants, size);
        report_left_out(frame, &variants, &slots, &snapping, dissolving.len());
        // Per entity, only the entities' cells change (#385): the glyph
        // pass runs inside their rect grown by two of the largest cell in
        // play, which holds a whole cell (a snapped silhouette) around
        // any styled pixel. The edge debug view draws everything.
        let reach = variants
            .iter()
            .map(|v| v.cell)
            .chain(bands.iter().map(|b| b.cell))
            .chain(snapping.iter().map(|g| [g[0], g[1]]))
            .chain([cell])
            .map(|c| c[0].max(c[1]))
            .fold(0.0f32, f32::max);
        let glyph_pad = (GLYPH_RECT_CELLS as f32 * reach).ceil() as u32;
        let edge_view = frame.debug_mode == crate::DebugMode::StyleEdges;
        let glyph_rect = frame.styled_rect(glyph_pad).filter(|_| !edge_view);
        // Where the edge cells are kept (#930): the glyph rect's, scissor
        // or not, so a cell's edge history is the same either way
        let edge_area = frame.styled_area(glyph_pad).filter(|_| !edge_view);
        // Where the mask can hold any variant's id, opt-outs too (#507): a
        // pixel further than a cell from it asks no grid of the mask, so a
        // whole frame over a few props reads the mask only near them
        let mask_rect = frame.mask_rect((2.0 * reach).ceil() as u32);

        // Edges (#267): line glyphs from geometry, one texel a cell on the
        // same grid as the cell state
        let mut edge_glyphs = [[0u32; 4]; EDGE_SLOTS.div_ceil(2)];
        for (i, ch) in set.edges().slots().iter().enumerate() {
            // A set's edge glyphs are always in its atlas (GlyphSet checks)
            let [x, y] = atlas
                .index_of(*ch)
                .map_or([0, 0], |index| atlas.cell_origin(index));
            edge_glyphs[i / 2][(i % 2) * 2] = x;
            edge_glyphs[i / 2][(i % 2) * 2 + 1] = y;
        }
        let edges = frame.param("edges").clamp(0.0, 1.0);
        let mut edge_cells_slot = self.edge_cells.lock().unwrap_or_else(|e| e.into_inner());
        let edge_cells = Owned::ensure(
            &mut edge_cells_slot,
            frame.device,
            "ASCII Edge Cells",
            EDGE_CELL_FORMAT,
            grid,
            &self.edges_in_bgl,
        );
        // Each edge cell's glyph (#910), bound even with the edges off
        let mut edge_slots_slot = self.edge_slots.lock().unwrap_or_else(|e| e.into_inner());
        let edge_slots = Owned::ensure(
            &mut edge_slots_slot,
            frame.device,
            "ASCII Edge Slots",
            EDGE_SLOT_FORMAT,
            grid,
            &self.cells_bgl,
        );
        // The variant table (#336), written before any pass reads it
        let (uniform, [edge_table, view_table], stats_width) =
            self.upload_table(frame, &variants, &slots);
        // The edge debug view (#349) runs the edge passes even with the
        // glyphs' edges off, and draws them in place of the glyphs
        if edges > 0.0 || edge_view {
            self.encode_edges(
                frame,
                encoder,
                input,
                [edge_cells, edge_slots],
                cell,
                [&uniform, &edge_table],
                variants.len(),
                [glyph_rect, edge_area],
                imported.as_ref(),
            );
        }
        if edge_view {
            let corners = corner_glyphs(frame, &set);
            self.encode_edge_view(
                frame,
                encoder,
                input,
                output,
                [edge_cells, edge_slots],
                cell,
                origin,
                corners,
                &view_table,
            );
            return;
        }

        // Last frame's cells and this frame's, ping-ponged (#277)
        let mut history = self.history.lock().unwrap_or_else(|e| e.into_inner());
        let textures = [
            frame.target_sized("ascii.cells.a", CELL_FORMAT, grid[0], grid[1]),
            frame.target_sized("ascii.cells.b", CELL_FORMAT, grid[0], grid[1]),
        ];
        let depths = [
            frame.target_sized("ascii.cell_depth.a", CELL_DEPTH_FORMAT, grid[0], grid[1]),
            frame.target_sized("ascii.cell_depth.b", CELL_DEPTH_FORMAT, grid[0], grid[1]),
        ];
        let fresh = history.adopt(textures, depths);
        let key = HistoryKey {
            epoch: frame.history_epoch,
            size,
            grid,
            cell: cell.map(f32::to_bits),
            origin: origin.map(f32::to_bits),
            font: set.name().to_string(),
            ramp: ramp.name().to_string(),
            ramp_len: steps.len() as u32,
        };
        let temporal = frame.option("temporal") != "off";
        let reproject = frame.option("temporal") == "reproject";
        // History a frame capture handed back (#1136): last frame's cells
        // and depths, and its camera, when the key is the captured frame's
        let mut taken = false;
        if let Some((h, part)) = imported.as_ref().and_then(|h| h.used_part("tone").map(|p| (h, p))) {
            let read = 1 - history.parity;
            let cells = history.textures.as_ref().expect("adopted above")[read].clone();
            let depth = history.depths.as_ref().expect("adopted above")[read].clone();
            match crate::stylize::history::key_difference(&part.key, &key.exported()) {
                Some(why) => h.say(format!(
                    "ascii: the captured tone history was another frame's ({why}): drawn fresh"
                )),
                None => match h
                    .write(frame.queue, part, "cells", &cells.texture)
                    .and_then(|()| h.write(frame.queue, part, "depths", &depth.texture))
                {
                    Ok(()) => {
                        history.last = Some((key.clone(), frame.frame.wrapping_sub(1)));
                        if let Some(m) = part.values.get("view_proj").filter(|m| m.len() == 16) {
                            history.last_view_proj =
                                Some(std::array::from_fn(|c| std::array::from_fn(|r| m[c * 4 + r])));
                        }
                        taken = true;
                        h.say("ascii: tone history taken from the capture");
                    }
                    Err(why) => h.say(format!("ascii: tone history not taken: {why}")),
                },
            }
        }
        let past = history.advance(key, frame.frame, temporal) && (!fresh || taken);
        let prev_view_proj = history.swap_view_proj(frame.camera.view_proj);
        history.used_past = past;
        history.used_view_proj = prev_view_proj;
        let (write, read) = (history.parity, 1 - history.parity);
        history.parity = read;
        let generation = history.generation;
        let pair = history.textures.clone().expect("adopted above");
        let depth_pair = history.depths.clone().expect("adopted above");
        drop(history);

        let uniforms = AsciiUniforms {
            cell,
            origin,
            glyph: [glyph[0] as f32, glyph[1] as f32],
            size: [frame.width as f32, frame.height as f32],
            tint: frame.color("tint"),
            color_mode: color_mode(frame.option("color")),
            brightness: frame.param("brightness"),
            contrast: frame.param("contrast").max(0.0),
            background: frame.param("background").clamp(0.0, 1.0),
            ink: frame.param("ink").max(0.0),
            levels: frame.param("levels").max(0.0),
            footprint: frame.param("footprint").clamp(1.0, 8.0),
            ramp_len: steps.len() as u32,
            srgb: frame.srgb() as u32,
            glow: frame.param("glow").max(0.0),
            ground: ground_mode(frame.option("ground")),
            palette_len: palette.colors().len() as u32,
            palette_match: palette.matching().index(),
            palette_bg: palette.background(),
            edge_ink: edge_ink_mode(frame.option("edge_ink")),
            // `smoothing` is per 60 Hz frame; this frame keeps it raised to
            // the number of 60 Hz frames it stands for (#342). The cap keeps
            // a high rate's weight from parking the 16-bit history for good
            smoothing: super::super::per_60hz(frame.param("smoothing").clamp(0.0, 0.99), frame.dt)
                .min(SMOOTHING_MAX),
            hysteresis: frame.param("hysteresis").max(0.0),
            dither: frame.param("dither").max(0.0),
            history: past as u32,
            edges,
            edge_on: u32::from(edges > 0.0),
            exposure: frame.param("exposure").clamp(-8.0, 8.0),
            stretch: frame.param("normalize").clamp(0.0, 1.0),
            chroma: frame.param("chroma").clamp(0.0, 8.0),
            palette_dither: frame.param("palette_dither").clamp(0.0, 1.0),
            edge_tint: frame.param("edge_tint").clamp(0.0, 1.0),
            corners: corner_glyphs(frame, &set),
            edge_glyphs,
            ramp: [[0; 4]; MAX_RAMP / 2],
            palette: [[0.0; 4]; MAX_COLORS],
            palette_lab: [[0.0; 4]; MAX_COLORS],
            variant_meta: [
                variants.len() as u32,
                !(frame.whole_frame() && frame.option("scope") != "entities") as u32,
                bands.len() as u32,
                snapping.len() as u32,
            ],
            bands: [[0.0; 4]; MAX_BANDS],
            band_rows: [[0; 4]; MAX_BANDS],
            snap_grids: [[0.0; 4]; MAX_SNAP_GRIDS],
            inv_view_proj: frame.camera.inv_view_proj,
            prev_view_proj,
            reproject: [reproject as u32, 0, 0, 0],
            edge_color: {
                let [r, g, b] = frame.color("edge_color");
                [r, g, b, 0.0]
            },
            dissolve_meta: [dissolving.len() as u32, 0, 0, 0],
            dissolve: [[0; 4]; MAX_DISSOLVE],
            ramp_tone: [[EQUAL_BANDS; 4]; MAX_RAMP / 4],
            mask_rect: rect_corners(mask_rect, size),
            snap_bands: [[0; 4]; MAX_SNAP_GRIDS],
        };
        let mut uniforms = uniforms;
        // Every font the frame draws in, stacked; the whole frame's is at
        // the top, so its origins are its atlas's own
        let stack = self.stack(frame, &mut fonts, &in_use);
        for (i, (font, index)) in ramps.steps.iter().enumerate() {
            let [x, y] = stack.origin(*font, *index);
            let pair = &mut uniforms.ramp[i / 2];
            pair[(i % 2) * 2] = x;
            pair[(i % 2) * 2 + 1] = y;
        }
        for (i, t) in ramps.tones.iter().enumerate() {
            uniforms.ramp_tone[i / 4][i % 4] = *t;
        }
        uniforms.palette[..palettes.inks.len()].copy_from_slice(&palettes.inks);
        uniforms.palette_lab[..palettes.lab.len()].copy_from_slice(&palettes.lab);
        for (i, b) in bands.iter().enumerate() {
            uniforms.bands[i] = [b.cell[0], b.cell[1], b.origin[0], b.origin[1]];
            uniforms.band_rows[i] = [b.row, b.rows, b.cols, b.footprint.to_bits()];
        }
        uniforms.snap_grids[..snapping.len()].copy_from_slice(&snapping);
        uniforms.snap_bands = snap_bands(&snapping, &owner_bands);
        uniforms.dissolve[..dissolving.len()].copy_from_slice(&dissolving);
        frame
            .queue
            .write_buffer_counted(&self.uniforms, 0, bytemuck::bytes_of(&uniforms));
        let variant = |i: usize| generation * 2 + i as u64;

        // Tone statistics (#289): each slot's luminance range, from this
        // frame's picture, for the slots that normalise
        let mut stats_slot = self.stats.lock().unwrap_or_else(|e| e.into_inner());
        let stats = Owned::ensure(
            &mut stats_slot,
            frame.device,
            "ASCII Tone Stats",
            STATS_FORMAT,
            [stats_width, 1],
            &self.edges_in_bgl,
        );
        if uniforms.stretch > 0.0 || variants.iter().any(|v| v.stretch > 0.0) {
            self.encode_stats(frame, encoder, input, &uniform, variants.len() > 1, stats);
        }

        // Cell pass: each cell's colour and glyph, from the picture and
        // (when it follows on) last frame's cells
        let past_bind = frame.depth_bind("ASCII History BG", variant(read), || {
            frame.device.create_bind_group_counted(&wgpu::BindGroupDescriptor {
                label: Some("ASCII History BG"),
                layout: &self.history_bgl,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: wgpu::BindingResource::TextureView(&pair[read].view),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: wgpu::BindingResource::TextureView(&depth_pair[read].view),
                    },
                    wgpu::BindGroupEntry {
                        binding: 2,
                        resource: wgpu::BindingResource::TextureView(frame.depth),
                    },
                ],
            })
        });
        kit::fullscreen_pass_targets(
            encoder,
            "ASCII Cells",
            &[&pair[write].view, &depth_pair[write].view],
            &self.cells_pipeline,
            &[
                &uniform,
                &frame.sampled_input("ASCII Input BG", &self.input_bgl, input),
                &past_bind,
                &stats.bind,
            ],
        );

        // Variant cells (#313): each per-entity variant cell's colour sum,
        // once a cell rather than in each of its pixels
        let mut variant_cells_slot = self.variant_cells.lock().unwrap_or_else(|e| e.into_inner());
        let [first_slot, second_slot] = &mut *variant_cells_slot;
        let band_size = band_texture_size(&bands, first_slot.as_ref().map(|o| o.size));
        let variant_cells = [
            Owned::ensure(
                first_slot,
                frame.device,
                "ASCII Variant Cells A",
                VARIANT_CELL_FORMAT,
                band_size,
                &self.cells_bgl,
            ),
            Owned::ensure(
                second_slot,
                frame.device,
                "ASCII Variant Cells B",
                VARIANT_CELL_FORMAT,
                band_size,
                &self.cells_bgl,
            ),
        ];
        if !bands.is_empty() {
            // Only the cells whose lattice can meet one of the band's own
            // variants (#507); the rest stay cleared, which is what they
            // would sum to
            let rects: Vec<_> = bands
                .iter()
                .map(|b| {
                    let near = variants
                        .iter()
                        .skip(1)
                        .filter(|v| v.cell_row == b.row + 1)
                        .fold(kit::PixelRect::EMPTY, |r, v| r.union(corners_rect(v.bounds)));
                    band_cells_in(b, near)
                })
                .collect();
            kit::fullscreen_pass_targets_in(
                encoder,
                "ASCII Variant Cells",
                &[&variant_cells[0].view, &variant_cells[1].view],
                &self.variant_cells_pipeline,
                &[&uniform, &self.picture_bind(frame, input, true)],
                Some(&rects),
            );
        }

        // Snap owners (#929): which snapping variant holds each cell of each
        // snapping grid, once a cell rather than in each of its pixels
        let mut snap_owner_slot = self.snap_owners.lock().unwrap_or_else(|e| e.into_inner());
        let owner_size = owner_texture_size(&owner_bands, snap_owner_slot.as_ref().map(|o| o.size));
        let snap_owners = Owned::ensure(
            &mut snap_owner_slot,
            frame.device,
            "ASCII Snap Owners",
            SNAP_OWNER_FORMAT,
            owner_size,
            &self.cells_bgl,
        );
        if !owner_bands.is_empty() {
            let draws: Vec<_> = owner_bands.iter().map(|b| (b.rect(), b.slot)).collect();
            kit::fullscreen_pass_instances_in(
                encoder,
                "ASCII Snap Owners",
                &snap_owners.view,
                &self.snap_owner_pipeline,
                &[&uniform, &self.picture_bind(frame, input, true)],
                &draws,
            );
        }

        // The dissolve (#398): each dissolving variant's cells' order, then
        // each one's rank among the variant's cells near it
        let mut dissolve_slot = self.dissolve.lock().unwrap_or_else(|e| e.into_inner());
        let [order_slot, rank_slot] = &mut *dissolve_slot;
        let dissolve_size = dissolve_texture_size(&dissolving, rank_slot.as_ref().map(|o| o.size));
        let dissolve_order = Owned::ensure(
            order_slot,
            frame.device,
            "ASCII Dissolve Order",
            DISSOLVE_FORMAT,
            dissolve_size,
            &self.edges_in_bgl,
        );
        let dissolve_ranks = Owned::ensure(
            rank_slot,
            frame.device,
            "ASCII Dissolve Ranks",
            DISSOLVE_FORMAT,
            dissolve_size,
            &self.edges_in_bgl,
        );
        if !dissolving.is_empty() {
            kit::fullscreen_pass(
                encoder,
                "ASCII Dissolve Order",
                &dissolve_order.view,
                &self.dissolve_order_pipeline,
                &[&uniform, &self.picture_bind(frame, input, true)],
            );
            kit::fullscreen_pass(
                encoder,
                "ASCII Dissolve Rank",
                &dissolve_ranks.view,
                &self.dissolve_rank_pipeline,
                &[&uniform, &dissolve_order.bind],
            );
        }

        // Glyph pass: draw the cells
        let font = fonts.stack.as_ref().expect("stacked above");
        let cells = cells_bind(
            frame,
            "ASCII Cells BG",
            &self.cells_bgl,
            &pair[write],
            variant(write),
        );
        // A cell whose entity a later display style draws over isn't ours
        // in the finished frame (#317): the text pass reads the mask for it
        let capturing = self.capture.load(Ordering::Relaxed);
        let overdrawn = if capturing {
            overdrawn_ids(frame)
        } else {
            Vec::new()
        };
        let with_mask = variants.len() > 1 || !overdrawn.is_empty();
        let entity = self.entity_bind(
            frame,
            input,
            with_mask,
            edge_cells,
            edge_slots,
            stats,
            variant_cells,
            dissolve_ranks,
            snap_owners,
        );
        let groups = [&*uniform, &*cells, &font.bind, &*entity];
        let input_bind = frame.sampled_input("ASCII Input BG", &self.input_bgl, input);
        kit::scissored_pass(
            encoder,
            "ASCII Pass",
            output,
            size,
            &self.pipeline,
            &groups,
            glyph_rect,
            &self.blit,
            &input_bind,
        );

        // The same cells as text (#317), when asked
        if capturing {
            let mut slot = self.text.lock().unwrap_or_else(|e| e.into_inner());
            if slot.as_ref().is_none_or(|t| t.grid != grid) {
                *slot = Some(TextCapture::new(frame.device, grid));
            }
            let capture = slot.as_mut().expect("made above");
            kit::fullscreen_pass(
                encoder,
                "ASCII Text",
                &capture.view,
                &self.text_pipeline,
                &groups,
            );
            encoder.copy_texture_to_buffer(
                capture.texture.as_image_copy(),
                wgpu::TexelCopyBufferInfo {
                    buffer: &capture.buffer,
                    layout: wgpu::TexelCopyBufferLayout {
                        offset: 0,
                        bytes_per_row: Some(capture.row_bytes),
                        rows_per_image: None,
                    },
                },
                wgpu::Extent3d {
                    width: grid[0],
                    height: grid[1],
                    depth_or_array_layers: 1,
                },
            );
            capture.first = text_first(origin);
            // Every font's glyphs, at their places in the stack (#335)
            capture.glyphs = font
                .sets
                .iter()
                .enumerate()
                .flat_map(|(f, set)| {
                    set.atlas()
                        .glyphs()
                        .iter()
                        .enumerate()
                        .map(move |(i, ch)| ((f as u32, i as u32), *ch))
                })
                .map(|((f, i), ch)| {
                    let [x, y] = font.origin(f, i);
                    (x | (y << 16), ch)
                })
                .collect();
            capture.overdrawn = overdrawn;
            capture.copied = true;
        }
    }

    /// Per entity (#743), the edge cells it sums lie within two of the
    /// largest cell of the glyph pass's rect (itself two cells round the
    /// entities, #385), so the shared edges are drawn within four cells
    /// of them, a cell counted three times as tall as wide (no font is that
    /// tall) since this is asked before the fonts load. The edge view
    /// reads the whole frame.
    fn edges_rect(&self, frame: &StyleFrame<'_>) -> Option<kit::PixelRect> {
        if frame.debug_mode == crate::DebugMode::StyleEdges {
            return None;
        }
        let points = frame
            .variants()
            .map(|(_, v)| frame.variant_param(v, "cell"))
            .fold(frame.param("cell"), f32::max);
        let ppp = if frame.pixels_per_point.is_finite() && frame.pixels_per_point > 0.0 {
            frame.pixels_per_point
        } else {
            1.0
        };
        let reach = (points.max(1.0) * ppp).round().max(2.0) * 3.0;
        let size = [frame.width, frame.height];
        frame.styled_rect((4.0 * reach).ceil() as u32 + 2).map(|r| {
            r.rescaled(size, frame.depth_size)
                .padded(2, frame.depth_size[0], frame.depth_size[1])
        })
    }

    /// The tone cells and their depths, and the edge cells and slots,
    /// last frame's as the captured frame read them (#1136): textures the
    /// frame read and never wrote, so after it they still hold its past.
    fn export_history(&self, out: &mut crate::stylize::history::HistoryExport<'_>) {
        let history = self.history.lock().unwrap_or_else(|e| e.into_inner());
        if let (Some((key, drawn)), Some(cells), Some(depths)) =
            (&history.last, &history.textures, &history.depths)
        {
            if *drawn == out.frame {
                // The frame flipped the parity to the side it read
                let read = history.parity;
                let view_proj: Vec<f32> =
                    history.used_view_proj.iter().flatten().copied().collect();
                out.part(
                    "tone",
                    key.exported(),
                    history.used_past,
                    [("view_proj".to_string(), view_proj)].into(),
                    &[
                        ("cells", &cells[read].texture),
                        ("depths", &depths[read].texture),
                    ],
                );
            }
        }
        drop(history);
        let past = self.edge_past.lock().unwrap_or_else(|e| e.into_inner());
        if let (Some((key, drawn)), Some(cells), Some(slots)) =
            (&past.last, &past.cells, &past.slots)
        {
            if *drawn == out.frame {
                out.part(
                    "edges",
                    key.exported(),
                    past.used,
                    Default::default(),
                    &[("cells", &cells.texture), ("slots", &slots.texture)],
                );
            }
        }
    }

    fn import_history(&self, history: crate::stylize::history::ImportedHistory) {
        self.imported.put(history);
    }

    fn set_text_capture(&self, on: bool) {
        self.capture.store(on, Ordering::Relaxed);
        if !on {
            *self.text.lock().unwrap_or_else(|e| e.into_inner()) = None;
        }
    }

    fn read_text(&self, device: &wgpu::Device) -> Option<GlyphText> {
        self.text
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .as_mut()?
            .read(device)
    }

    /// The whole frame's grid under `silhouette = "cell"` (#534), so the
    /// edge of another style's entity is whole glyphs too: a cell at least
    /// `cell_share` not the other style's is drawn whole, any other is the
    /// other style's. The silhouette that snaps ASCII's own props (#338)
    /// then snaps the props of every other style the frame runs.
    fn ownership_cells(
        &self,
        frame: &StyleFrame<'_>,
    ) -> Option<crate::stylize::OwnershipCells> {
        if !frame.covers_frame() || frame.option("silhouette") != "cell" {
            return None;
        }
        let mut fonts = self.fonts.lock().unwrap_or_else(|e| e.into_inner());
        // Before `encode` polls this frame: a glyph set reloaded this
        // frame snaps to last frame's cell once
        self.follow_scope(frame, &mut fonts);
        let (cell, origin) = self.whole_frame_grid(frame, &mut fonts);
        Some(crate::stylize::OwnershipCells {
            cell,
            origin,
            share: frame.param("cell_share"),
        })
    }
}

/// Whether ASCII reads the style mask under `config` (#350), beside the
/// per-entity case every style with a component gets (an entity naming
/// ASCII: its variants are looked up by mask id). With no id there is no
/// boundary to outline and no entity to pass through; past that the mask
/// is read for its outlines while `edges > 0`, and for the pixels of
/// another style's entities, which a later style draws over (#317).
pub(super) fn reads_mask(
    config: &crate::postprocess::PostProcessConfig,
    ids: &crate::style_mask::StyleIds,
) -> bool {
    !ids.is_empty()
        && (STYLE.param(config, "edges") > 0.0
            || ids
                .iter()
                .any(|(id, _)| ids.variant(id).is_some_and(|v| v.style != STYLE.name)))
}

/// Whether ASCII reads the shared edges this frame (#743): while its
/// `edges` are on, or the edge debug view (#349) draws them in place of
/// the glyphs.
pub(super) fn needs_edges(frame: &StyleFrame<'_>) -> bool {
    STYLE.param(frame.config, "edges") > 0.0 || frame.debug_mode == crate::DebugMode::StyleEdges
}

/// The style mask ids whose pixels aren't ASCII's in the finished frame
/// (#317): an entity of another style that's on this frame, which the
/// framework hands back to that style (#384) whichever of the two runs
/// first.
pub(super) fn overdrawn_ids(frame: &StyleFrame<'_>) -> Vec<u32> {
    frame.foreign_ids().into_iter().map(u32::from).collect()
}

/// The text grid's first visible texel (#317): the cell state's texel 0
/// holds the part cell left of (above) the origin, which is off screen
/// when the grid starts at the frame's edge.
pub(super) fn text_first(origin: [f32; 2]) -> [u32; 2] {
    origin.map(|o| u32::from(o <= 0.0))
}
