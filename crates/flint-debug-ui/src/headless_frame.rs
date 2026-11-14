//! An egui frame with no painter behind it, for tests (#944).

/// Run one egui frame of `ui` on `ctx` and return what it drew, with its
/// texture updates cleared.
///
/// Nothing uploads a test's textures, and egui 0.36 asserts in debug builds
/// when a `FullOutput` drops with texture updates in it (the first frame
/// always has the font atlas). Every test that runs an egui frame goes
/// through here, so the egui bump (#945) changes only this body (there,
/// `Context::run` is also `run_ui`). The output's shapes, platform output
/// and viewport output are left as they were.
pub fn run_headless_frame(
    ctx: &egui::Context,
    input: egui::RawInput,
    ui: impl FnMut(&egui::Context),
) -> egui::FullOutput {
    #[allow(clippy::disallowed_methods)] // the one test-side call
    let mut output = ctx.run(input, ui);
    output.textures_delta.clear();
    output
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_first_frame_has_its_font_atlas_cleared() {
        let ctx = egui::Context::default();
        // What the helper clears: a plain run's first frame uploads the atlas.
        #[allow(clippy::disallowed_methods)] // what the helper clears
        let mut plain = ctx.run(egui::RawInput::default(), |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| ui.label("glyphs"));
        });
        assert!(!plain.textures_delta.is_empty());
        plain.textures_delta.clear();

        let ctx = egui::Context::default();
        let output = run_headless_frame(&ctx, egui::RawInput::default(), |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| ui.label("glyphs"));
        });
        assert!(output.textures_delta.is_empty());
        assert!(!output.shapes.is_empty(), "the frame still drew");
    }
}
