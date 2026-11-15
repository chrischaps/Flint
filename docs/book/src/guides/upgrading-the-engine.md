# Upgrading the Engine

Most engine pulls need nothing beyond reading the changelog ([After a pull: read the changelog](building-a-game-project.md#after-a-pull-read-the-changelog)). A few move a dependency that a game's own Rust code uses too. This page holds the port for each of those, newest first.

## wgpu 30 and egui 0.36 (November 2026)

Flint moved from wgpu 23.0.1 to 30.0.2 and from egui 0.30 to 0.36.2 (#950: [#944](https://github.com/chrischaps/Flint/issues/944), [#945](https://github.com/chrischaps/Flint/issues/945), [#946](https://github.com/chrischaps/Flint/issues/946)). egui-wgpu and egui-winit moved to 0.36.2 with egui, and egui-snarl to 0.12.

### What needs no port

A game made only of scenes, scripts, schemas and user styles (`.shader.toml` and WGSL) builds and runs as before. What it can notice:

- **A script's translucent colours draw darker** since [#958](https://github.com/chrischaps/Flint/issues/958): script `draw_*` colours, text halos, world text and HUD sprites take egui 0.36's blend, as Flint's own UI does. Opaque colours, and dark translucent ones such as shadows and dimming panels, are unchanged. [HUD colours](#hud-colours) says how to retune. Text is a pixel different here and there under egui's new rasteriser.
- **User style errors are worded by naga 30.** `flint style check`, the log and the error card say ``no definition in scope for identifier: `x` `` where they said `'x'`, and ``expected `;`, found "let"`` where they said `expected ';', found 'let'`. The file, line, column, quoted line and Flint's hints are unchanged ([naga's wording](#nagas-wording)).
- **`pack2x16float`, `unpack2x16float` and `quantizeToF16` work in a user style.** naga 23 didn't know `quantizeToF16`.
- **A Kuwahara `radius` above 32 draws at 32.**

### What needs a port

A game with Rust of its own ports the parts that call wgpu or egui:

- a `PlayerExtension` that draws egui UI ([the game UI hook](#the-game-ui-hook) and [egui 0.30 to 0.36](#egui-030-to-036));
- a Rust style built on `stylize::kit`, or a renderer or window of its own ([wgpu 23 to 30](#wgpu-23-to-30), [WGSL](#wgsl) and [Flint's shaders are trusted code](#flints-shaders-are-trusted-code)).

**Cargo.** Name the same versions as the engine (`wgpu = "30.0.2"`, `egui = "0.36.2"`) or take them from the engine's workspace. A subtree game that builds for the web also adds the engine's patched egui-winit to its workspace root's `Cargo.toml`, which adds the one function the web build needs:

```toml
[patch.crates-io]
egui-winit = { path = "engine/vendor/egui-winit" }
```

### egui 0.30 to 0.36

#### The game UI hook

**`PlayerExtension::ui` takes the frame's root `Ui`.** egui 0.36 shows its one `egui::Panel` type on a `Ui`, not a `Context`, and eframe's `App::ui` takes `&mut egui::Ui` (eframe 0.34 deprecated `App::update(&Context)` and 0.35 removed it). `PlayerExtension::ui` took a `&egui::Context`, so a game's side and top panels had no port. It now takes the root `Ui` that egui's `Context::run_ui` hands out ([ADR 0121](https://github.com/chrischaps/Flint/blob/main/docs/adr/0121-extension-ui-takes-the-root-ui.md)): show panels on it and windows and areas from it.

```rust
// egui 0.30
fn ui(&mut self, ctx: &egui::Context) {
    egui::SidePanel::left("inventory").default_width(220.0).show(ctx, |ui| {
        ui.heading("Inventory");
    });
    egui::Window::new("Quests").show(ctx, |ui| ui.label("none yet"));
}

// egui 0.36
fn ui(&mut self, ui: &mut egui::Ui) {
    egui::Panel::left("inventory").default_size(220.0).show(ui, |ui| {
        ui.heading("Inventory");
    });
    egui::Window::new("Quests").show(ui, |ui| ui.label("none yet"));
}
```

An old `ui` no longer compiles (``method `ui` has an incompatible type for trait``): a Rust trait method can't be deprecated into a new signature. The port is the parameter's type. A `Ui` derefs to its `Context`, so with `fn ui(&mut self, ctx: &mut egui::Ui)` a body written for a `Context` compiles as it was: `Window::new(..).show(ctx, ..)`, `ctx.input(..)`, `ctx.clone()` (a `Context`) and passing `ctx` to a function that takes `&egui::Context`. Only the panels need the renames below.

A game that took `root_ui(&mut egui::Ui)`, the hook #946 added for panels before this change, keeps drawing: `root_ui` is deprecated, the player still calls it, and its default calls `ui`. Rename it to `ui`. rustc doesn't warn on implementing a deprecated trait method, so this line is the reminder.

Don't show a `CentralPanel` from `ui`. It covers the whole window and takes every click from the game.

The renames Flint's own code needed, checked against egui 0.36.2:

| egui 0.30 | egui 0.36 |
|---|---|
| `ctx.run(input, \|ctx\| ..)` | `ctx.run_ui(input, \|ui\| ..)`; the closure gets the root `Ui` (`ui.ctx()` for the context). In a test, `flint_debug_ui::run_headless_frame(ctx, input, \|ui\| ..)` |
| `SidePanel::left(id)`, `TopBottomPanel::top(id)` | `Panel::left(id)`, `Panel::top(id)` (and `right`, `bottom`), `.show(ui, ..)` |
| a panel's `default_width` / `default_height`, `min_`, `max_`, `exact_width` / `_height`, `width_range` / `height_range` | `default_size`, `min_size`, `max_size`, `exact_size`, `size_range` |
| a panel's `show_animated(ctx, open, ..)` | `show_collapsible(ui, &mut open, ..)` |
| `CentralPanel::default().show(ctx, ..)` | `CentralPanel::default().show(ui, ..)` |
| `ctx.style()`, `ctx.set_style(..)`, `ctx.style_mut(..)` | `ctx.global_style()`, `ctx.set_global_style(..)`, `ctx.global_style_mut(..)` |
| `ctx.screen_rect()`, `i.screen_rect()` | `ctx.content_rect()`, `i.content_rect()` |
| `ctx.available_rect()` | none on the context: in `root_ui`, `ui.available_rect_before_wrap()` |
| `ctx.fonts(\|f\| f.layout_no_wrap(..))` | `ctx.fonts_mut(\|f\| f.layout_no_wrap(..))`; laying text out needs the mutable view, and `fonts` reads only |
| `ui.output_mut(\|o\| o.copied_text = text)` | `ui.ctx().copy_text(text)` |
| `Frame::none()` | `Frame::NONE` |
| `.rounding(r)`, `Rounding` | `.corner_radius(r)`, `CornerRadius` (a `u8` a corner) |
| `Margin::same(10.0)`, `Margin::symmetric(12.0, 8.0)` | `Margin::same(10)`, `Margin::symmetric(12, 8)` (an `i8` a side) |
| `painter.rect_stroke(rect, radius, stroke)` | `painter.rect_stroke(rect, radius, stroke, egui::StrokeKind::Middle)`; `Middle` is 0.30's placement |
| `.id_source(..)`, `ComboBox::from_id_source(..)` | `.id_salt(..)`, `ComboBox::from_id_salt(..)` |
| `ui.close_menu()` | `ui.close()` |
| `egui::menu::bar(ui, ..)` | `egui::MenuBar::new().ui(ui, ..)` |
| `egui_wgpu::Renderer::new(device, format, depth, samples, dithering)` | `Renderer::new(device, format, egui_wgpu::RendererOptions { depth_stencil_format, msaa_samples, dithering, ..Default::default() })` |
| `for (id, delta) in &textures_delta.set` | `for (id, deltas) in &textures_delta.set { for delta in deltas { .. } }`: a list a texture |
| egui-snarl's `show_input` / `show_output` return `PinInfo` | they return `impl SnarlPin` (`PinInfo` is one) |

Two changes in behaviour:

- **Modifier keys are keys.** egui-winit 0.36 sends `egui::Key::ShiftLeft`, `ControlRight` and the rest as key presses. A game that reads `ctx.input(|i| i.events)` sees them; `i.modifiers` still holds the state.
- **A translucent colour blends in display space.** Since egui 0.31, `Color32::from_rgba_unmultiplied` and `from_white_alpha` premultiply the display values. egui 0.30 premultiplied in linear light, so a 50% white over black drew 188 there and draws 128 now. A game's own egui UI takes 0.36's blend, and since #958 so do scripts; to keep a light colour as it looked, raise its alpha to alpha^(1/2.2) ([HUD colours](#hud-colours)).

#### HUD colours

What scripts draw takes egui 0.36's blend ([#958](https://github.com/chrischaps/Flint/issues/958)): `draw_*` colours, text halos (`stroke` and `glow`), world text and HUD sprites. One frame now has one blend, the same as Flint's editors, debug panels and a game's `PlayerExtension` egui UI. Until #958, scripts kept egui 0.30's.

**What changes.** egui 0.30 premultiplied a colour in linear light, which made a translucent light colour brighter than its alpha said. Now the alpha is used as written:

- **A light colour over a darker ground draws darker.** That covers glows, halos, white text and hints, and highlights. A 50% white over black drew 188 and now draws 128, and a 0.3 cyan stroke turns a duller blue.
- **A dark colour is unchanged.** A black panel at 0.6, or a drop shadow, draws the same bytes, because 0.30's brightening only ever added the colour's own light.
- **Opaque colours are unchanged.** A glow at 0.9 changes only slightly.

**Getting a light colour's old look back: raise its alpha to alpha^(1/2.2).** egui 0.30 drew about `alpha^(1/2.2) × colour`, so this restores what the colour adds to the frame. The ground behind it shows a little less (`1 − alpha^(1/2.2)` in place of `1 − alpha`). Over black that difference is nothing, and over a dark ground it is a level or two.

| old alpha | 0.1 | 0.2 | 0.25 | 0.3 | 0.4 | 0.5 | 0.6 | 0.7 | 0.75 | 0.8 | 0.9 |
|---|---|---|---|---|---|---|---|---|---|---|---|
| new alpha | 0.35 | 0.48 | 0.53 | 0.58 | 0.66 | 0.73 | 0.79 | 0.85 | 0.88 | 0.90 | 0.95 |

Measured in the player (`--pixels-per-point 1`, a sweep of new alphas in steps of 0.01, picking the closest to each old swatch):

| colour over ground | old alpha | was | now, same alpha | closest new alpha | at alpha^(1/2.2) |
|---|---|---|---|---|---|
| white over black | 0.25 / 0.5 / 0.75 | 136 / 187 / 224 | 63 / 127 / 191 | 0.53 / 0.73 / 0.88 | 135 / 186 / 224 |
| cyan `[0.25, 0.85, 1]` over navy `[0.05, 0.04, 0.1]` | 0.25 / 0.5 / 0.75 | `(38, 122, 155)` / `(50, 163, 200)` / `(57, 193, 230)` | `(25, 61, 82)` / `(37, 113, 140)` / `(50, 165, 197)` | 0.56 / 0.76 / 0.89 | `(39, 119, 147)` / `(49, 161, 193)` / `(56, 191, 227)` |
| black over white | 0.25 / 0.5 / 0.75 | 192 / 128 / 64 | 192 / 128 / 64 | unchanged | |
| orange `[1, 0.55, 0.15]` over 50% grey | 0.25 / 0.5 / 0.75 | `(232, 168, 111)` / `(251, 165, 89)` / `(255, 155, 64)` | `(159, 131, 105)` / `(191, 134, 83)` / `(223, 137, 60)` | 0.82 / 0.97 / 1.0, still off | `(195, 134, 80)` / `(220, 136, 62)` / `(239, 138, 48)` |

The last row is the limit. Over a ground about as bright as the colour, egui 0.30 drew brighter than the colour itself: the orange's green is 140, and a 50% orange over grey drew 165. No alpha gets that back. Brighten the colour, or accept the change.

The engine's own colours were retuned this way: the `.style.toml` default `glow_color` is `[1, 1, 1, 0.73]` (it was 0.5), and `flint init`'s HUD hints and crosshair moved by the table.

### wgpu 23 to 30

For a game's own style, renderer or window code. Most are renames the compiler names:

- **Pipelines:** `depth_write_enabled: Some(true)` and `depth_compare: Some(..)`; `multiview_mask: None` in place of `multiview: None`; `immediate_size: 0` in place of `push_constant_ranges: &[]`.
- **Optional entries:** a pipeline layout's `bind_group_layouts: &[Some(&layout)]`, and a vertex state's `buffers: &[Some(layout)]`.
- **Passes:** each colour attachment has `depth_slice: None`, and each render pass descriptor has `multiview_mask: None`.
- **Copies:** the deprecated names are gone: `TexelCopyBufferLayout` for `ImageDataLayout`, `TexelCopyTextureInfo` for `ImageCopyTexture`, `TexelCopyBufferInfo` for `ImageCopyBuffer`.
- **Samplers:** `mipmap_filter: wgpu::MipmapFilterMode::Linear`.
- **Waiting:** `flint_render::wait_for_gpu(&device)` for `device.poll(Maintain::Wait)`, and `device.poll(wgpu::PollType::Poll)` for `Maintain::Poll`. Both return a `Result`. wgpu 23 gave up on a wait after 60 s; wgpu 30 waits as long as it takes. `wait_for_gpu` waits at most `FLINT_GPU_WAIT_TIMEOUT` seconds when that is set, and panics when it runs out, which turns a hung GPU in a test run into an error. Flint's own crates refuse `PollType::wait_indefinitely()` through clippy for that reason.
- **Handles are `Clone`:** every wgpu type has been `Clone` since wgpu 24 (an `Arc` inside), so a `Queue` or `Device` can be kept in a struct.
- **Reading back:** `slice.get_mapped_range()` returns a `Result`.
- **Devices:** `request_device(&desc)` takes no trace path; finish a `DeviceDescriptor` with `..Default::default()`. `InstanceDescriptor` has no `Default`: start from `InstanceDescriptor::new_without_display_handle()`.
- **Surfaces:** `SurfaceConfiguration` has `color_space` (`SurfaceColorSpace::Auto` is the sRGB every earlier wgpu chose). `get_current_texture()` returns a `CurrentSurfaceTexture` enum; take frames with `flint_render::AcquireFrame` (`surface.acquire()`), which returns a `Result` with `flint_render::SurfaceError`. It has no `OutOfMemory`, because wgpu 30 has none: drop that arm. Show a frame with `queue.present(frame)`.
- **Error scopes:** `push_error_scope` returns a guard that pops the scope when it drops. Bind it and `.pop()` it; an unbound `device.push_error_scope(..);` catches nothing.
- **Shader modules:** make modules from your own WGSL through `flint_render::TrustedShader` (`device.create_trusted_shader_module(desc)`, or `stylize::kit::shader`), as Flint's do.

### WGSL

- **`@interpolate(flat)` on integer inter-stage I/O.** naga 30 refuses an integer vertex output or fragment input without it (``@interpolate(flat) must be explicitly specified for integer I/O``). Vertex inputs and render targets don't need it.
- **Half floats in a validator of your own.** `pack2x16float`, `unpack2x16float` and `quantizeToF16` need `naga::valid::Capabilities::SHADER_FLOAT16_IN_FLOAT32` in a hand-built `naga::valid::Validator`. wgpu grants it from `DownlevelFlags::SHADER_F16_IN_F32`, which every backend has except Vulkan on Mesa's llvmpipe without `SHADER_F16`. Flint's validators grant it.

#### naga's wording

naga 30 rewords its errors: backticks where 23 had single quotes (``no definition in scope for identifier: `glow` ``), and double quotes for a token it found (``expected `;`, found "let"``). Code that matches the text of a naga error stops matching. Go by its structure instead: `ParseError::labels()` gives the spans, and for an unknown identifier the first label's span is the identifier itself. Flint's style hints read it that way now, so the `[images.<name>]` hint appears under naga 30 and in the browser's own compiler alike:

```text
error: styles\darkroom.wgsl:14:37: no definition in scope for identifier: `flint_image_tiled_canvas`
    14 |     let paper = mix(vec3<f32>(1.0), flint_image_tiled_canvas(paper_uv).rgb, params.paper_strength);
    hint: an image is declared as [images.<name>] or an option with kind = "image"; this style's images: grade, paper
```

### Flint's shaders are trusted code

wgpu 25 and later bound every shader loop by default, which made ASCII 2.6 times slower on the GPU. Flint builds its own WGSL with loop bounding off, through `TrustedShader` (#945), after checking that each of its loops ends. `stylize::kit::shader` is public and goes the same way, so **a game's own Rust style built on the kit is trusted code too.** Its loops must end: a loop that never does hangs the GPU until the driver resets it. A loop counted to a constant, or to a value the CPU clamps, is safe. A bound a scene can push past `i32::MAX` with `<=` is not, which is why Kuwahara's `radius` is capped at 32.

Set `FLINT_SHADER_CHECKS=all` to turn loop bounding back on for every built-in module: for an A/B, or as the first test when a GPU hangs. User styles (`.shader.toml`) are not trusted. They keep wgpu's checks and Flint's own loop guard ([Stylize Passes](../concepts/stylize-passes.md)).
