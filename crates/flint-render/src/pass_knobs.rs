//! Cost a part of the frame without timestamps (#1317): skip it, or record
//! it N times, by the label the GPU timer, F2 and a RenderDoc capture
//! already give it (`Shadows`, `Scene`, `Style edges`, `ASCII`, `Bloom`,
//! `Composite`; [`crate::gpu_labels`]).
//!
//! Where GPU timestamps read 0 (Mali, #568) the only way to cost a pass is
//! to take it away and watch the frame time. #568 did that by patching
//! same-length WGSL into the web player's wasm, a site copy a pass. These
//! knobs do it from a flag or a URL on any GPU:
//!
//! - **skip**: the part records no work. Its targets keep what they last
//!   held: zero (wgpu clears a texture it makes) when it is skipped from
//!   the first frame, else what it wrote before the skip began. What reads
//!   them still runs, on that, so the frame's cost is the part's own cost
//!   less the full-screen write its pass made. A skipped part logs as
//!   skipped in a frame's description, and opens no timer scope.
//! - **repeat N**: the part records its work N times inside one timer
//!   scope and one debug group, so `--gpu-time` reads N times its cost and
//!   the frame time grows by N - 1 of it. Only parts that redraw their
//!   targets whole each time take it: a part that adds into what it wrote
//!   (the grass compute's blade count, an overlay blended over the
//!   picture) or keeps history from frame to frame (ASCII's cells, a user
//!   style's `history` target) refuses, with a warning naming it, and
//!   draws once.
//!
//! Off by default: a renderer with no knobs carries `None` and asks one
//! `Option` a part. With knobs on, a frame allocates nothing after the
//! first few (the labels seen are kept once each).
//!
//! Hosts: `flint-player` / `flint play` `--skip-pass` / `--repeat-pass`,
//! `flint render` the same, the web page's `?skip_pass=` /
//! `?repeat_pass=`; each hands [`PassKnobs`] to
//! [`SceneRenderer::set_pass_knobs`](crate::SceneRenderer::set_pass_knobs).

/// The most a part may be repeated.
pub const MAX_REPEAT: u32 = 256;

/// Frames drawn before a knob naming no part the frame opened is warned
/// about: the first, since a still draws one (a part switched on later
/// still takes the knob; the warning says "yet").
const UNMATCHED_AFTER_FRAMES: u32 = 1;

/// Which parts of the frame to skip or repeat, by label (case-blind).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PassKnobs {
    skip: Vec<String>,
    repeat: Vec<(String, u32)>,
}

impl PassKnobs {
    /// No knobs: every part draws once.
    pub fn new() -> Self {
        Self::default()
    }

    /// Whether every part draws once.
    pub fn is_empty(&self) -> bool {
        self.skip.is_empty() && self.repeat.is_empty()
    }

    /// Skip each part `list` names: one label, or several split by commas
    /// (`Style edges,Bloom`). Empty names are ignored.
    pub fn skip_list(&mut self, list: &str) {
        for label in list.split(',').map(str::trim).filter(|l| !l.is_empty()) {
            if !self.skip.iter().any(|s| s.eq_ignore_ascii_case(label)) {
                self.skip.push(label.to_string());
            }
        }
    }

    /// Repeat parts as `list` says: `<label>:<N>`, several split by commas
    /// (`Scene:4,Bloom:2`). A later word for a label replaces an earlier.
    pub fn repeat_list(&mut self, list: &str) -> Result<(), String> {
        for word in list.split(',').map(str::trim).filter(|w| !w.is_empty()) {
            let (label, n) = parse_repeat(word)?;
            self.repeat.retain(|(l, _)| !l.eq_ignore_ascii_case(&label));
            self.repeat.push((label, n));
        }
        Ok(())
    }

    /// The labels asked about, skipped then repeated.
    pub fn labels(&self) -> impl Iterator<Item = &str> {
        self.skip
            .iter()
            .map(String::as_str)
            .chain(self.repeat.iter().map(|(l, _)| l.as_str()))
    }

    /// How many times the part named `label` records its work: 0 skipped,
    /// N repeated, else 1. A label both skipped and repeated is skipped.
    pub fn times(&self, label: &str) -> u32 {
        if self.skip.iter().any(|s| s.eq_ignore_ascii_case(label)) {
            return 0;
        }
        self.repeat
            .iter()
            .find(|(l, _)| l.eq_ignore_ascii_case(label))
            .map_or(1, |(_, n)| *n)
    }

    /// One line saying what the knobs do, for a host's start-up log.
    pub fn describe(&self) -> String {
        let mut parts = Vec::new();
        if !self.skip.is_empty() {
            parts.push(format!("skipping {}", self.skip.join(", ")));
        }
        if !self.repeat.is_empty() {
            let r: Vec<String> = self
                .repeat
                .iter()
                .map(|(l, n)| format!("{l} x{n}"))
                .collect();
            parts.push(format!("repeating {}", r.join(", ")));
        }
        parts.join("; ")
    }
}

/// `<label>:<N>` into its label and count, 1 to [`MAX_REPEAT`].
fn parse_repeat(word: &str) -> Result<(String, u32), String> {
    let (label, n) = word.rsplit_once(':').ok_or_else(|| {
        format!("repeat_pass `{word}`: expected <label>:<N>, e.g. `Style edges:4`")
    })?;
    let label = label.trim();
    if label.is_empty() {
        return Err(format!("repeat_pass `{word}`: no label before the `:`"));
    }
    let n: u32 = n
        .trim()
        .parse()
        .map_err(|_| format!("repeat_pass `{word}`: `{}` is not a whole number", n.trim()))?;
    if n == 0 {
        return Err(format!(
            "repeat_pass `{word}`: 0 times is skip_pass={label}"
        ));
    }
    if n > MAX_REPEAT {
        return Err(format!("repeat_pass `{word}`: at most {MAX_REPEAT} times"));
    }
    Ok((label.to_string(), n))
}

/// Why a part draws once though a repeat was asked for it: it adds into
/// what it wrote (the grass compute's blade count).
pub const ADDS_INTO_ITS_TARGET: &str =
    "adds into what it wrote, so a second run isn't the same work";
/// ... it keeps history from frame to frame (ASCII, a user style's
/// `history` target).
pub const KEEPS_HISTORY: &str =
    "keeps history from frame to frame, so a second run in one frame changes the picture";
/// ... it blends over the finished picture (world lines, the selection
/// outline, a host's HUD).
pub const DRAWS_OVER_THE_FRAME: &str =
    "blends over the finished picture, so a second run changes it";

/// The knobs as a renderer holds them while drawing: what it has said, and
/// which labels its frames opened, so a knob that names no part is warned
/// about once.
#[derive(Debug)]
pub(crate) struct KnobRun {
    pub knobs: PassKnobs,
    /// Labels the frames opened (each once; static, so no copies)
    seen: Vec<&'static str>,
    /// Labels a repeat was refused for, said once each
    refused: Vec<&'static str>,
    frames: u32,
    said_unmatched: bool,
}

impl KnobRun {
    pub fn new(knobs: PassKnobs) -> Self {
        Self {
            knobs,
            seen: Vec::new(),
            refused: Vec::new(),
            frames: 0,
            said_unmatched: false,
        }
    }

    /// How many times `label` records its work this frame; its repeat
    /// refused, with `why` said once, when `refuse` names a reason.
    pub fn times(&mut self, label: &'static str, refuse: Option<&str>) -> u32 {
        if !self.seen.contains(&label) {
            self.seen.push(label);
        }
        let n = self.knobs.times(label);
        match refuse {
            Some(why) if n > 1 => {
                if !self.refused.contains(&label) {
                    self.refused.push(label);
                    tracing::warn!(
                        target: "render",
                        "repeat_pass: `{label}` draws once: it {why}"
                    );
                }
                1
            }
            _ => n,
        }
    }

    /// A frame is drawn: after a few, say once which knobs named no part.
    pub fn end_frame(&mut self) {
        self.frames = self.frames.saturating_add(1);
        if self.said_unmatched || self.frames < UNMATCHED_AFTER_FRAMES {
            return;
        }
        self.said_unmatched = true;
        let unmatched: Vec<&str> = self
            .knobs
            .labels()
            .filter(|l| !self.seen.iter().any(|s| s.eq_ignore_ascii_case(l)))
            .collect();
        if !unmatched.is_empty() {
            tracing::warn!(
                target: "render",
                "skip_pass / repeat_pass: no part of the frame is named {} (yet); \
                 the parts drawn: {}",
                unmatched
                    .iter()
                    .map(|l| format!("`{l}`"))
                    .collect::<Vec<_>>()
                    .join(", "),
                self.seen.join(", ")
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_part_draws_once_unless_a_knob_names_it() {
        let mut k = PassKnobs::new();
        assert!(k.is_empty());
        assert_eq!(k.times("Scene"), 1);
        k.skip_list(" Style edges , bloom,,");
        k.repeat_list("Scene:4, Shadows:2").unwrap();
        assert!(!k.is_empty());
        assert_eq!(k.times("Style edges"), 0);
        assert_eq!(k.times("STYLE EDGES"), 0);
        assert_eq!(k.times("Bloom"), 0);
        assert_eq!(k.times("Scene"), 4);
        assert_eq!(k.times("Shadows"), 2);
        assert_eq!(k.times("Composite"), 1);
        assert_eq!(
            k.describe(),
            "skipping Style edges, bloom; repeating Scene x4, Shadows x2"
        );
    }

    #[test]
    fn a_later_repeat_replaces_an_earlier_and_skip_wins() {
        let mut k = PassKnobs::new();
        k.repeat_list("Scene:4").unwrap();
        k.repeat_list("scene:8").unwrap();
        assert_eq!(k.times("Scene"), 8);
        k.skip_list("Scene");
        assert_eq!(k.times("Scene"), 0);
    }

    #[test]
    fn a_label_may_hold_a_colon() {
        let mut k = PassKnobs::new();
        k.repeat_list("a:b:3").unwrap();
        assert_eq!(k.times("a:b"), 3);
    }

    #[test]
    fn a_bad_repeat_says_what_it_wants() {
        let mut k = PassKnobs::new();
        for (word, says) in [
            ("Scene", "expected <label>:<N>"),
            (":4", "no label"),
            ("Scene:x", "not a whole number"),
            ("Scene:0", "skip_pass=Scene"),
            ("Scene:9999", "at most 256"),
        ] {
            let e = k.repeat_list(word).unwrap_err();
            assert!(e.contains(says), "{word}: {e}");
        }
        assert!(k.is_empty());
    }

    #[test]
    fn a_refused_repeat_draws_once() {
        let mut k = PassKnobs::new();
        k.repeat_list("Grass:3,Scene:3").unwrap();
        let mut run = KnobRun::new(k);
        assert_eq!(run.times("Grass", Some(ADDS_INTO_ITS_TARGET)), 1);
        assert_eq!(run.times("Scene", None), 3);
        // A skip of a part that refuses repeat still skips
        let mut k = PassKnobs::new();
        k.skip_list("Grass");
        let mut run = KnobRun::new(k);
        assert_eq!(run.times("Grass", Some(ADDS_INTO_ITS_TARGET)), 0);
    }

    #[test]
    fn the_labels_seen_are_kept_once() {
        let mut k = PassKnobs::new();
        k.skip_list("Nothing");
        let mut run = KnobRun::new(k);
        for _ in 0..5 {
            run.times("Scene", None);
            run.times("Scene", None);
            run.end_frame();
        }
        assert_eq!(run.seen, ["Scene"]);
        assert!(run.said_unmatched);
    }
}
