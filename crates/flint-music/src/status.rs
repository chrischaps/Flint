//! Console readout of musical position and per-bus state.
//!
//! Shared by `flint play-suite` (live) and `flint render-suite` (offline) so
//! realtime and headless evidence are textually comparable. This is a dev
//! surface only — nothing here may reach a player-facing build (GDD: the
//! world is the interface).

use crate::conductor::MusicalPosition;
use crate::manifest::Section;
use crate::mixer::{BusMixer, LPF_OPEN_HZ};

/// One status line: musical position, active section, per-bus state.
/// Bars and beats are displayed 1-based (bar 1 beat 1 = suite start);
/// internally both count from 0.
pub fn status_line(pos: &MusicalPosition, section: Option<&Section>, mixer: &BusMixer) -> String {
    let mut line = format!(
        "bar {:>3} beat {:>4.2} | section {:<10} | sample {:>9}",
        pos.bar + 1,
        pos.beat_in_bar + 1.0,
        section.map(|s| s.name.as_str()).unwrap_or("-"),
        pos.sample,
    );
    for (name, state) in mixer.states() {
        let mut s = format!(" | {name} {:+.1}dB", state.gain_db);
        if state.lpf_hz < LPF_OPEN_HZ {
            s.push_str(&format!("/{:.0}Hz", state.lpf_hz));
        }
        if state.detune_semitones != 0.0 {
            s.push_str(&format!("/{:+.2}st", state.detune_semitones));
        }
        if !state.playing {
            s.push_str("(silent)");
        }
        line.push_str(&s);
    }
    line
}

/// The Phase 2 bare-feedback readout: value plus a ten-cell text meter,
/// e.g. `coherence 0.73 [#######...]`. The tuning instrument until the
/// world itself renders coherence (Phase 4 kills this on sight).
pub fn coherence_meter(value: f64) -> String {
    let cells = (value.clamp(0.0, 1.0) * 10.0).round() as usize;
    format!(
        "coherence {value:.2} [{}{}]",
        "#".repeat(cells),
        ".".repeat(10 - cells)
    )
}

/// [`status_line`] plus the coherence meter — used by play-chart and
/// replay-chart; play-suite/render-suite keep the plain line.
pub fn status_line_with_coherence(
    pos: &MusicalPosition,
    section: Option<&Section>,
    mixer: &BusMixer,
    coherence: f64,
) -> String {
    format!("{} | {}", status_line(pos, section, mixer), coherence_meter(coherence))
}
