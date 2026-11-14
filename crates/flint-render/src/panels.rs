//! Comic panels (#880, ADR 0120): several cameras and looks in one frame.
//!
//! A scene's `[panels]` table lays the frame out as a page: each
//! `[panels.panel.<name>]` is a share of the frame with its own camera and,
//! if it names one, its own look. The renderer draws each panel's camera
//! into a target of the panel's size, with the whole post chain (styles,
//! mask, depth) at that size, then lays the panels onto the frame over a
//! gutter, each inside an inked border. The HUD still draws over the top.
//!
//! ```toml
//! [panels]
//! gutter = 10                  # points between panels and round the page
//! gutter_color = [0.96, 0.94, 0.89]  # the page
//! border = 2.5                 # points of ink round each panel
//! border_color = [0.08, 0.07, 0.06]
//!
//! [panels.panel.wide]
//! rect = [0.0, 0.0, 0.62, 1.0] # x, y, width, height as shares of the page, y down
//!
//! [panels.panel.close]
//! rect = [0.62, 0.0, 0.38, 1.0]
//! follow = "mannequin"         # position and target are then from its origin
//! position = [0.6, 1.6, 1.4]
//! target = [0.0, 1.4, 0.0]
//! fov = 35
//! look = "rotoscope"           # laid over the scene's [post_process]
//! ```
//!
//! This module is the description: parsing and editing a layout, its pixel
//! rectangles and each panel's camera, and the compositor that lays the
//! panels on the page. Drawing a panel is the renderer's
//! (`scene_renderer/panels.rs`).

use crate::camera::Camera;
use flint_core::Vec3;

/// The most panels a page draws; more are left out with a warning.
pub const MAX_PANELS: usize = 16;

/// The page's defaults: a warm paper gutter and a near-black ink border.
pub const DEFAULT_GUTTER: f32 = 10.0;
pub const DEFAULT_GUTTER_COLOR: [f32; 3] = [0.957, 0.937, 0.886];
pub const DEFAULT_BORDER: f32 = 2.5;
pub const DEFAULT_BORDER_COLOR: [f32; 3] = [0.078, 0.071, 0.063];

/// The keys `[panels]` takes, and each `[panels.panel.<name>]`.
pub const PAGE_KEYS: &[&str] = &["gutter", "gutter_color", "border", "border_color", "panel"];
pub const PANEL_KEYS: &[&str] = &[
    "rect",
    "order",
    "position",
    "target",
    "fov",
    "follow",
    "look",
    "post_process",
];

/// A page of panels, parsed from a `[panels]` table.
#[derive(Clone, Debug, PartialEq)]
pub struct PanelLayout {
    /// Points between two panels and between a panel and the frame's edge.
    pub gutter: f32,
    /// The page's colour, in display values.
    pub gutter_color: [f32; 3],
    /// Points of ink round each panel, inside its share.
    pub border: f32,
    /// The ink's colour, in display values.
    pub border_color: [f32; 3],
    /// The panels in drawing order: by `order`, then by name.
    pub panels: Vec<PanelSpec>,
    /// The table it was parsed from, which edits are merged into.
    source: toml::Table,
}

/// One panel: where on the page, what it looks at, and how it's drawn.
#[derive(Clone, Debug, PartialEq, Default)]
pub struct PanelSpec {
    pub name: String,
    /// x, y, width, height as shares of the page (0..1), y down.
    pub rect: [f32; 4],
    /// Drawing order, lower first; ties by name. A later panel covers an
    /// earlier one where they overlap.
    pub order: i64,
    /// Where the camera is. Unset: the frame camera's. With `follow`, from
    /// the followed entity's origin.
    pub position: Option<[f32; 3]>,
    /// What it looks at. Unset: the frame camera's, or with `follow` the
    /// followed entity's origin.
    pub target: Option<[f32; 3]>,
    /// Field of view in degrees. Unset: the frame camera's.
    pub fov: Option<f32>,
    /// An entity by name whose world origin `position` and `target` are from.
    pub follow: Option<String>,
    /// A look laid over the scene's `[post_process]` for this panel alone.
    pub look: Option<String>,
    /// `[post_process]` keys over that, for this panel alone.
    pub post_process: Option<toml::Table>,
}

impl PanelSpec {
    /// Whether the panel draws under a look of its own; else it draws with
    /// the frame's post-processing as it stands (F4, scripts and all).
    pub fn has_own_look(&self) -> bool {
        self.look.is_some() || self.post_process.is_some()
    }
}

/// A panel's place in the frame, in pixels: `outer` is its share less the
/// gutter, `inner` that less the border, where the panel's picture goes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PanelRect {
    pub outer: [u32; 4],
    pub inner: [u32; 4],
}

impl PanelRect {
    /// The picture's size, never 0.
    pub fn size(&self) -> (u32, u32) {
        (self.inner[2].max(1), self.inner[3].max(1))
    }
}

impl PanelLayout {
    /// The layout a scene's top-level tables describe, if they hold a
    /// `[panels]` table with at least one panel. A table that doesn't parse
    /// is warned about and the scene draws as one view.
    pub fn from_scene_tables(tables: &toml::Table, scene: &str) -> Option<PanelLayout> {
        let table = tables.get("panels")?;
        let Some(table) = table.as_table() else {
            tracing::warn!("{scene}: [panels] is not a table (drawn as one view)");
            return None;
        };
        match PanelLayout::parse(table) {
            Ok(layout) => layout,
            Err(e) => {
                tracing::warn!("{scene}: [panels]: {e} (drawn as one view)");
                None
            }
        }
    }

    /// Parse a `[panels]` table. `Ok(None)` when it names no panel.
    pub fn parse(table: &toml::Table) -> Result<Option<PanelLayout>, String> {
        for key in table.keys() {
            if !PAGE_KEYS.contains(&key.as_str()) {
                return Err(unknown_key("[panels]", key, PAGE_KEYS));
            }
        }
        let number = |key: &str, default: f32| -> Result<f32, String> {
            match table.get(key) {
                None => Ok(default),
                Some(v) => flint_core::toml_util::toml_f32(v)
                    .filter(|n| n.is_finite() && *n >= 0.0)
                    .ok_or_else(|| format!("{key} must be a number of points, 0 or more")),
            }
        };
        let colour = |key: &str, default: [f32; 3]| -> Result<[f32; 3], String> {
            match table.get(key) {
                None => Ok(default),
                Some(v) => {
                    parse_color(v).ok_or_else(|| format!("{key} must be [r, g, b], each 0..1"))
                }
            }
        };
        let mut panels = Vec::new();
        if let Some(list) = table.get("panel") {
            let list = list
                .as_table()
                .ok_or("panel must be a table of [panels.panel.<name>] tables")?;
            for (name, panel) in list {
                let panel = panel
                    .as_table()
                    .ok_or_else(|| format!("panel.{name} must be a table"))?;
                panels.push(parse_panel(name, panel).map_err(|e| format!("panel.{name}: {e}"))?);
            }
        }
        if panels.is_empty() {
            return Ok(None);
        }
        panels.sort_by(|a, b| a.order.cmp(&b.order).then_with(|| a.name.cmp(&b.name)));
        if panels.len() > MAX_PANELS {
            tracing::warn!(
                "[panels]: {} panels, {MAX_PANELS} drawn (the rest are left out)",
                panels.len()
            );
            panels.truncate(MAX_PANELS);
        }
        Ok(Some(PanelLayout {
            gutter: number("gutter", DEFAULT_GUTTER)?,
            gutter_color: colour("gutter_color", DEFAULT_GUTTER_COLOR)?,
            border: number("border", DEFAULT_BORDER)?,
            border_color: colour("border_color", DEFAULT_BORDER_COLOR)?,
            panels,
            source: table.clone(),
        }))
    }

    /// The table this layout came from (edits included).
    pub fn table(&self) -> &toml::Table {
        &self.source
    }

    /// The panel named `name`.
    pub fn panel(&self, name: &str) -> Option<&PanelSpec> {
        self.panels.iter().find(|p| p.name == name)
    }

    /// This layout with `keys` merged into panel `name` (made if it isn't
    /// there), as a script's `set_panel` asks. `keys` are a panel's keys.
    pub fn with_panel(&self, name: &str, keys: &toml::Table) -> Result<PanelLayout, String> {
        let mut table = self.source.clone();
        merge_panel(&mut table, name, keys);
        PanelLayout::parse(&table)?.ok_or_else(|| "no panels".to_string())
    }

    /// This layout less panel `name`; `None` when it was the last.
    pub fn without_panel(&self, name: &str) -> Result<Option<PanelLayout>, String> {
        let mut table = self.source.clone();
        if let Some(toml::Value::Table(list)) = table.get_mut("panel") {
            list.remove(name);
        }
        PanelLayout::parse(&table)
    }

    /// Each panel's rectangle in a `width` x `height` frame at
    /// `pixels_per_point`. The page is the frame less half a gutter all
    /// round, and each panel its share of the page less half a gutter, so
    /// two panels side by side have one gutter between them and the frame's
    /// edge has one too.
    pub fn rects(&self, width: u32, height: u32, pixels_per_point: f32) -> Vec<PanelRect> {
        let ppp = if pixels_per_point > 0.0 {
            pixels_per_point
        } else {
            1.0
        };
        let half = self.gutter * ppp * 0.5;
        let border = (self.border * ppp).round().max(0.0) as u32;
        let (fw, fh) = (width as f32, height as f32);
        let page = [
            half,
            half,
            (fw - 2.0 * half).max(1.0),
            (fh - 2.0 * half).max(1.0),
        ];
        self.panels
            .iter()
            .map(|p| {
                let [x, y, w, h] = p.rect;
                let edge = |at: f32, from: f32, span: f32, limit: f32| {
                    (from + at * span).round().clamp(0.0, limit) as u32
                };
                let x0 = edge(x, page[0] + half, page[2], fw);
                let y0 = edge(y, page[1] + half, page[3], fh);
                let x1 = edge(x + w, page[0] - half, page[2], fw).max(x0 + 1);
                let y1 = edge(y + h, page[1] - half, page[3], fh).max(y0 + 1);
                let (ow, oh) = (x1 - x0, y1 - y0);
                // At most half the panel is border, so a picture always shows
                let b = border
                    .min(ow.saturating_sub(1) / 2)
                    .min(oh.saturating_sub(1) / 2);
                PanelRect {
                    outer: [x0, y0, ow, oh],
                    inner: [x0 + b, y0 + b, ow - 2 * b, oh - 2 * b],
                }
            })
            .collect()
    }
}

/// The camera a panel draws with: the frame's camera with the panel's
/// position, target and field of view laid over it, `position` and
/// `target` from `followed` (the followed entity's world origin) when the
/// panel follows one, and the panel's own aspect.
pub fn panel_camera(
    spec: &PanelSpec,
    frame: &Camera,
    followed: Option<Vec3>,
    aspect: f32,
) -> Camera {
    let mut camera = frame.clone();
    let base = followed.unwrap_or(Vec3::ZERO);
    let v = |a: [f32; 3]| Vec3::new(a[0], a[1], a[2]);
    if let Some(p) = spec.position {
        camera.position = base + v(p);
    }
    match (spec.target, followed) {
        (Some(t), _) => camera.target = base + v(t),
        (None, Some(f)) => camera.target = f,
        (None, None) => {}
    }
    if let Some(fov) = spec.fov {
        camera.fov = fov.clamp(1.0, 170.0);
    }
    if aspect.is_finite() && aspect > 0.0 {
        camera.aspect = aspect;
    }
    // An orthographic panel with no ortho_height sizes itself by distance
    if spec.position.is_some() || spec.target.is_some() || followed.is_some() {
        camera.distance = (camera.position - camera.target).length().max(0.001);
    }
    camera
}

/// Merge `keys` into `[panels.panel.<name>]` of `table`, making it if
/// needed; tables merge, everything else replaces.
pub fn merge_panel(table: &mut toml::Table, name: &str, keys: &toml::Table) {
    let list = table
        .entry("panel")
        .or_insert_with(|| toml::Value::Table(toml::Table::new()));
    if !list.is_table() {
        *list = toml::Value::Table(toml::Table::new());
    }
    let list = list.as_table_mut().expect("made a table above");
    let panel = list
        .entry(name.to_string())
        .or_insert_with(|| toml::Value::Table(toml::Table::new()));
    if !panel.is_table() {
        *panel = toml::Value::Table(toml::Table::new());
    }
    crate::look::merge(panel.as_table_mut().expect("made a table above"), keys);
}

fn parse_panel(name: &str, t: &toml::Table) -> Result<PanelSpec, String> {
    for key in t.keys() {
        if !PANEL_KEYS.contains(&key.as_str()) {
            return Err(unknown_key("a panel", key, PANEL_KEYS));
        }
    }
    let rect = t.get("rect").ok_or("needs rect = [x, y, width, height]")?;
    let rect = numbers::<4>(rect).ok_or("rect must be [x, y, width, height] (shares, 0..1)")?;
    if rect.iter().any(|n| !(0.0..=1.0).contains(n)) || rect[2] <= 0.0 || rect[3] <= 0.0 {
        return Err(format!(
            "rect {rect:?}: each share is 0..1, and width and height are above 0"
        ));
    }
    let vec3 = |key: &str| -> Result<Option<[f32; 3]>, String> {
        t.get(key)
            .map(|v| numbers::<3>(v).ok_or_else(|| format!("{key} must be [x, y, z]")))
            .transpose()
    };
    let text = |key: &str| -> Result<Option<String>, String> {
        t.get(key)
            .map(|v| {
                v.as_str()
                    .map(str::to_string)
                    .ok_or_else(|| format!("{key} must be a string"))
            })
            .transpose()
            .map(|s| s.filter(|s| !s.trim().is_empty()))
    };
    Ok(PanelSpec {
        name: name.to_string(),
        rect,
        order: match t.get("order") {
            None => 0,
            Some(v) => v.as_integer().ok_or("order must be a whole number")?,
        },
        position: vec3("position")?,
        target: vec3("target")?,
        fov: t
            .get("fov")
            .map(|v| flint_core::toml_util::toml_f32(v).ok_or("fov must be a number of degrees"))
            .transpose()?,
        follow: text("follow")?,
        look: text("look")?,
        post_process: t
            .get("post_process")
            .map(|v| {
                v.as_table()
                    .cloned()
                    .ok_or("post_process must be a table of [post_process] keys")
            })
            .transpose()?,
    })
}

fn numbers<const N: usize>(v: &toml::Value) -> Option<[f32; N]> {
    let a = v.as_array()?;
    if a.len() != N {
        return None;
    }
    let mut out = [0.0; N];
    for (o, v) in out.iter_mut().zip(a) {
        *o = flint_core::toml_util::toml_f32(v).filter(|n| n.is_finite())?;
    }
    Some(out)
}

/// `[r, g, b]` (or `[r, g, b, a]`, the alpha unused) in 0..1, as
/// `toml_util::toml_color` reads a colour, so the schema's `color` holds it.
pub fn parse_color(v: &toml::Value) -> Option<[f32; 3]> {
    let c = flint_core::toml_util::toml_color(v)?;
    Some([c[0], c[1], c[2]].map(|x| x.clamp(0.0, 1.0)))
}

fn unknown_key(place: &str, key: &str, known: &[&str]) -> String {
    format!("{place} has no key '{key}' (it takes {})", known.join(", "))
}

/// A display value to the linear value an sRGB target stores it from.
fn to_linear(c: f32) -> f32 {
    if c <= 0.04045 {
        c / 12.92
    } else {
        ((c + 0.055) / 1.055).powf(2.4)
    }
}

/// A colour for a target of `format`: linear for an sRGB format, which
/// encodes on write, as it is otherwise.
pub(crate) fn color_for(format: wgpu::TextureFormat, c: [f32; 3]) -> [f32; 3] {
    if format.is_srgb() {
        c.map(to_linear)
    } else {
        c
    }
}

// ── The compositor ──

const SLOT: u64 = 256;

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct PanelUniform {
    /// The picture's rectangle in frame pixels: x, y, width, height
    inner: [u32; 4],
    /// The border's colour, as the target stores it
    border: [f32; 4],
}

/// Lays the panels' pictures on the page: one pass over the frame that
/// clears it to the gutter's colour, then a full-screen triangle a panel,
/// clipped to its outer rectangle, showing its picture texel for texel
/// inside and the border's ink round it.
pub(crate) struct PanelCompositor {
    format: wgpu::TextureFormat,
    pipeline: wgpu::RenderPipeline,
    texture_layout: wgpu::BindGroupLayout,
    uniform_buffer: wgpu::Buffer,
    uniform_bind: wgpu::BindGroup,
}

impl PanelCompositor {
    pub(crate) fn new(device: &wgpu::Device, format: wgpu::TextureFormat) -> Self {
        let uniform_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("Panel Uniform Layout"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: true,
                    min_binding_size: wgpu::BufferSize::new(
                        std::mem::size_of::<PanelUniform>() as u64
                    ),
                },
                count: None,
            }],
        });
        let texture_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("Panel Picture Layout"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Texture {
                    sample_type: wgpu::TextureSampleType::Float { filterable: false },
                    view_dimension: wgpu::TextureViewDimension::D2,
                    multisampled: false,
                },
                count: None,
            }],
        });
        let uniform_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Panel Uniforms"),
            size: SLOT * MAX_PANELS as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let uniform_bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("Panel Uniform Bind"),
            layout: &uniform_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::Buffer(wgpu::BufferBinding {
                    buffer: &uniform_buffer,
                    offset: 0,
                    size: wgpu::BufferSize::new(std::mem::size_of::<PanelUniform>() as u64),
                }),
            }],
        });
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("Panel Compositor"),
            source: wgpu::ShaderSource::Wgsl(SHADER.into()),
        });
        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("Panel Compositor Layout"),
            bind_group_layouts: &[Some(&uniform_layout), Some(&texture_layout)],
            immediate_size: 0,
        });
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("Panel Compositor"),
            layout: Some(&layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_main"),
                buffers: &[],
                compilation_options: Default::default(),
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs_main"),
                targets: &[Some(wgpu::ColorTargetState {
                    format,
                    blend: None,
                    write_mask: wgpu::ColorWrites::ALL,
                })],
                compilation_options: Default::default(),
            }),
            primitive: wgpu::PrimitiveState::default(),
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            multiview_mask: None,
            cache: None,
        });
        Self {
            format,
            pipeline,
            texture_layout,
            uniform_buffer,
            uniform_bind,
        }
    }

    pub(crate) fn format(&self) -> wgpu::TextureFormat {
        self.format
    }

    /// The bind group over one panel's picture, made with the picture.
    pub(crate) fn picture_bind(
        &self,
        device: &wgpu::Device,
        view: &wgpu::TextureView,
    ) -> wgpu::BindGroup {
        device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("Panel Picture Bind"),
            layout: &self.texture_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::TextureView(view),
            }],
        })
    }

    /// Lay `panels` (each rectangle with its picture's bind group) on
    /// `target`, a frame of `size`, over `layout`'s gutter.
    pub(crate) fn encode(
        &self,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        target: &wgpu::TextureView,
        size: (u32, u32),
        layout: &PanelLayout,
        panels: &[(PanelRect, &wgpu::BindGroup)],
    ) {
        let border = color_for(self.format, layout.border_color);
        let mut bytes = vec![0u8; SLOT as usize * panels.len().max(1)];
        for (i, (rect, _)) in panels.iter().enumerate() {
            let u = PanelUniform {
                inner: rect.inner,
                border: [border[0], border[1], border[2], 1.0],
            };
            let at = i * SLOT as usize;
            bytes[at..at + std::mem::size_of::<PanelUniform>()]
                .copy_from_slice(bytemuck::bytes_of(&u));
        }
        queue.write_buffer(&self.uniform_buffer, 0, &bytes);
        let g = color_for(self.format, layout.gutter_color);
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("Panel Page"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: target,
                resolve_target: None,
                depth_slice: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(wgpu::Color {
                        r: g[0] as f64,
                        g: g[1] as f64,
                        b: g[2] as f64,
                        a: 1.0,
                    }),
                    store: wgpu::StoreOp::Store,
                },
            })],
            depth_stencil_attachment: None,
            timestamp_writes: None,
            occlusion_query_set: None,
            multiview_mask: None,
        });
        pass.set_pipeline(&self.pipeline);
        for (i, (rect, bind)) in panels.iter().enumerate() {
            let [x, y, w, h] = rect.outer;
            if x >= size.0 || y >= size.1 {
                continue;
            }
            let (w, h) = (w.min(size.0 - x), h.min(size.1 - y));
            pass.set_scissor_rect(x, y, w, h);
            pass.set_bind_group(0, &self.uniform_bind, &[(i as u64 * SLOT) as u32]);
            pass.set_bind_group(1, *bind, &[]);
            pass.draw(0..3, 0..1);
        }
    }
}

const SHADER: &str = r#"
struct Panel {
    inner: vec4<u32>,
    border: vec4<f32>,
};
@group(0) @binding(0) var<uniform> panel: Panel;
@group(1) @binding(0) var picture: texture_2d<f32>;

@vertex
fn vs_main(@builtin(vertex_index) i: u32) -> @builtin(position) vec4<f32> {
    let uv = vec2<f32>(f32((i << 1u) & 2u), f32(i & 2u));
    return vec4<f32>(uv * 2.0 - 1.0, 0.0, 1.0);
}

@fragment
fn fs_main(@builtin(position) at: vec4<f32>) -> @location(0) vec4<f32> {
    let p = vec2<u32>(at.xy);
    let lo = panel.inner.xy;
    let hi = lo + panel.inner.zw;
    if (all(p >= lo) && all(p < hi)) {
        return textureLoad(picture, vec2<i32>(p - lo), 0);
    }
    return panel.border;
}
"#;

#[cfg(test)]
mod tests {
    use super::*;

    fn layout(text: &str) -> PanelLayout {
        let table: toml::Table = text.parse().unwrap();
        PanelLayout::parse(table["panels"].as_table().unwrap())
            .unwrap()
            .unwrap()
    }

    const TWO: &str = r#"
        [panels]
        gutter = 10
        border = 2
        [panels.panel.wide]
        rect = [0.0, 0.0, 0.5, 1.0]
        [panels.panel.close]
        rect = [0.5, 0.0, 0.5, 1.0]
        follow = "hero"
        position = [0, 1, 2]
        look = "rotoscope"
        order = -1
    "#;

    #[test]
    fn panels_parse_in_order_then_name() {
        let l = layout(TWO);
        let names: Vec<_> = l.panels.iter().map(|p| p.name.as_str()).collect();
        assert_eq!(names, ["close", "wide"]);
        assert_eq!(l.gutter, 10.0);
        assert_eq!(l.gutter_color, DEFAULT_GUTTER_COLOR);
        assert!(l.panel("close").unwrap().has_own_look());
        assert!(!l.panel("wide").unwrap().has_own_look());
    }

    #[test]
    fn two_halves_share_one_gutter() {
        let l = layout(TWO);
        let rects = l.rects(1000, 500, 1.0);
        let wide = rects[1];
        let close = rects[0];
        // A gutter of 10 round the page and between the two
        assert_eq!(wide.outer, [10, 10, 485, 480]);
        assert_eq!(close.outer, [505, 10, 485, 480]);
        assert_eq!(close.outer[0] - (wide.outer[0] + wide.outer[2]), 10);
        assert_eq!(wide.inner, [12, 12, 481, 476]);
        // Points scale with the display
        let hi = l.rects(2000, 1000, 2.0);
        assert_eq!(hi[1].outer, [20, 20, 970, 960]);
    }

    #[test]
    fn a_whole_page_panel_with_no_gutter_is_the_frame() {
        let l =
            layout("[panels]\ngutter = 0\nborder = 0\n[panels.panel.all]\nrect = [0, 0, 1, 1]\n");
        let r = l.rects(640, 360, 1.5)[0];
        assert_eq!(r.outer, [0, 0, 640, 360]);
        assert_eq!(r.inner, r.outer);
    }

    #[test]
    fn mistakes_are_named() {
        let bad = |t: &str| {
            let table: toml::Table = t.parse().unwrap();
            PanelLayout::parse(table["panels"].as_table().unwrap()).unwrap_err()
        };
        assert!(bad("[panels]\ngutters = 1\n").contains("no key 'gutters'"));
        assert!(bad("[panels.panel.a]\nrect = [0, 0, 2, 1]\n").contains("panel.a: rect"));
        assert!(bad("[panels.panel.a]\nfov = 30\n").contains("needs rect"));
        assert!(bad("[panels.panel.a]\nrect = [0,0,1,1]\ncamera = 1\n").contains("no key 'camera'"));
        assert!(
            bad("[panels]\ngutter_color = \"#ff0000\"\n[panels.panel.a]\nrect = [0,0,1,1]\n")
                .contains("gutter_color")
        );
        // No panels is one view, not an error
        let table: toml::Table = "[panels]\ngutter = 4\n".parse().unwrap();
        assert!(PanelLayout::parse(table["panels"].as_table().unwrap())
            .unwrap()
            .is_none());
    }

    #[test]
    fn a_panel_is_edited_and_removed_by_name() {
        let l = layout(TWO);
        let mut keys = toml::Table::new();
        keys.insert(
            "rect".into(),
            toml::Value::try_from([0.0, 0.0, 1.0, 1.0]).unwrap(),
        );
        let moved = l.with_panel("wide", &keys).unwrap();
        assert_eq!(moved.panel("wide").unwrap().rect, [0.0, 0.0, 1.0, 1.0]);
        // The other keys and the page are as they were
        assert_eq!(moved.panel("close"), l.panel("close"));
        assert_eq!(moved.gutter, 10.0);
        let one = l.without_panel("close").unwrap().unwrap();
        assert_eq!(one.panels.len(), 1);
        assert!(one.without_panel("wide").unwrap().is_none());
        // A new name is a new panel
        let three = l.with_panel("inset", &keys).unwrap();
        assert_eq!(three.panels.len(), 3);
    }

    #[test]
    fn a_followed_panel_looks_from_its_entity() {
        let l = layout(TWO);
        let mut frame = Camera::new();
        frame.position = Vec3::new(0.0, 5.0, 10.0);
        frame.target = Vec3::ZERO;
        frame.fov = 60.0;
        let close = l.panel("close").unwrap();
        let cam = panel_camera(close, &frame, Some(Vec3::new(3.0, 0.0, 0.0)), 0.5);
        assert_eq!(cam.position, Vec3::new(3.0, 1.0, 2.0));
        assert_eq!(cam.target, Vec3::new(3.0, 0.0, 0.0));
        assert_eq!(cam.fov, 60.0);
        assert_eq!(cam.aspect, 0.5);
        // A panel with no camera keys is the frame's camera at its own aspect
        let wide = panel_camera(l.panel("wide").unwrap(), &frame, None, 2.0);
        assert_eq!(wide.position, frame.position);
        assert_eq!(wide.aspect, 2.0);
    }

    #[test]
    fn colours_are_arrays() {
        assert_eq!(
            parse_color(&toml::Value::try_from([0.5, 2.0, 0.0]).unwrap()),
            Some([0.5, 1.0, 0.0])
        );
        assert_eq!(
            parse_color(&toml::Value::try_from([1.0, 0.0, 0.0, 0.5]).unwrap()),
            Some([1.0, 0.0, 0.0])
        );
        assert_eq!(parse_color(&toml::Value::String("#ff0000".into())), None);
    }
}
