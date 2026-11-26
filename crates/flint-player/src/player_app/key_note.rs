//! The debug keys' note (#367) is the kernel's `flint_package::Note` (ADR
//! 0137): every subsystem's `StageCtx` lends the host's one. Its tests stay
//! here, beside the headless egui frame they paint with.

pub(crate) use flint_package::Note as KeyNote;

#[cfg(test)]
mod tests {
    use super::*;
    use flint_debug_ui::run_headless_frame;
    use flint_package::note::{FADE, HOLD};

    /// One egui pass at time `t` with the note painted; whether it drew.
    fn frame_at(note: &mut KeyNote, ctx: &egui::Context, t: f64) -> bool {
        let input = egui::RawInput {
            time: Some(t),
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(1280.0, 720.0),
            )),
            ..Default::default()
        };
        let out = run_headless_frame(ctx, input, |ui| note.paint(ui.ctx(), 24.0));
        !out.shapes.is_empty()
    }

    #[test]
    fn a_note_shows_holds_fades_and_goes() {
        let ctx = egui::Context::default();
        let mut note = KeyNote::default();
        assert!(
            !frame_at(&mut note, &ctx, 0.0),
            "nothing said, nothing drawn"
        );

        // Said at t=10 (between frames): its clock starts at the next paint
        note.say("F3", "no debug panels in this scene");
        assert!(frame_at(&mut note, &ctx, 10.0));
        assert_eq!(note.strength(10.0 + HOLD - 0.01), Some(1.0), "held");
        let mid = note.strength(10.0 + HOLD + FADE / 2.0).unwrap();
        assert!(mid > 0.0 && mid < 1.0, "fading: {mid}");
        assert!(frame_at(&mut note, &ctx, 10.0 + HOLD + FADE / 2.0));
        assert!(
            !frame_at(&mut note, &ctx, 10.0 + HOLD + FADE + 0.01),
            "gone"
        );
        assert!(note.showing().is_none(), "and cleared");
    }

    #[test]
    fn a_second_note_replaces_the_first_and_starts_over() {
        let ctx = egui::Context::default();
        let mut note = KeyNote::default();
        note.say("F3", "no debug panels in this scene");
        frame_at(&mut note, &ctx, 0.0);
        note.say("F4", "no renderer active — nothing to tune");
        assert_eq!(
            note.showing(),
            Some(("F4", "no renderer active — nothing to tune"))
        );
        frame_at(&mut note, &ctx, HOLD + 0.1);
        assert_eq!(note.strength(2.0 * HOLD), Some(1.0), "its own hold");
    }
}
