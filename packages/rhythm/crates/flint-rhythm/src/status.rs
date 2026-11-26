//! The rhythm harnesses' console readout: the general status line
//! (`flint_music::status::status_line`) plus a coherence meter. A dev surface
//! only, as the general one is.

use flint_music::conductor::MusicalPosition;
use flint_music::manifest::Section;
use flint_music::mixer::BusMixer;
use flint_music::status::status_line;

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
    format!(
        "{} | {}",
        status_line(pos, section, mixer),
        coherence_meter(coherence)
    )
}
