//! Frame-rate pacing on the web (#140): draw on every refresh while the
//! frames keep up, and on every Nth when they can't, so a phone shows an
//! even 33 ms instead of 25, 33, 42, 25.
//!
//! The browser calls the player once per refresh (`requestAnimationFrame`,
//! #121). Each call asks [`FramePacer::due`]; a call that isn't due returns
//! at once, so the frames that are drawn fall a whole number of refreshes
//! apart. A target is a period, not a fraction of the refresh: 30 fps is
//! two refreshes at 60 Hz and four at 120 Hz.
//!
//! In `auto` the pacer judges the last ten seconds of drawn frames. If
//! their p90 is over 1.2 times the period, it drops to the fastest rate
//! the frames would hold. It climbs back one rate at a time once the
//! player's own work, p90, would fit in half the faster period; a climb
//! that falls back within two windows doubles the wait before the next.
//!
//! The refresh itself isn't something a page can ask for. It's read from
//! the rAF timestamps: the slowest common display rate whose multiples
//! explain nine in ten of the intervals.
//!
//! Times are milliseconds, as the browser gives them. Nothing here touches
//! the browser, so it's tested natively; `web_pacing.rs` feeds it.

use std::collections::VecDeque;

/// The window the auto pacer judges, ms.
pub const WINDOW_MS: f64 = 10_000.0;
/// Drop when the frames' p90 is over this many periods.
const SLOW: f64 = 1.2;
/// Climb when the work's p90 fits in this share of the faster period.
const ROOM: f64 = 0.5;
/// The first wait at a rate before climbing, and the longest after
/// failed climbs, ms.
const FIRST_CLIMB_MS: f64 = WINDOW_MS;
const MAX_CLIMB_MS: f64 = 16.0 * WINDOW_MS;
/// Fewest frames in a window worth judging.
const MIN_FRAMES: usize = 30;
/// An interval longer than this is a stall or a hidden tab, not a frame.
const STALL_MS: f64 = 250.0;
/// The rates auto steps through below the display's: `None` is every
/// refresh.
const LEVELS: [Option<f64>; 3] = [None, Some(60.0), Some(30.0)];
/// Display rates the refresh is snapped to, slowest first.
const DISPLAY_HZ: [f64; 8] = [60.0, 75.0, 90.0, 100.0, 120.0, 144.0, 165.0, 240.0];
/// rAF intervals kept for the refresh estimate.
const REFRESH_SAMPLES: usize = 120;

/// `?fps=`: what rate the web player draws at.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum FrameRateSetting {
    /// Every refresh, unpaced: the desktop's default, as before #140.
    Native,
    /// This many frames a second, as a whole number of refreshes.
    Fixed(f64),
    /// Every refresh while the frames hold it, fewer when they don't.
    /// The default on touch devices.
    Auto,
}

impl FrameRateSetting {
    /// `30`, `60` (any whole rate from 10 to 240), `auto` or `native`.
    pub fn parse(s: &str) -> Option<Self> {
        match s.trim() {
            "auto" => Some(Self::Auto),
            "native" => Some(Self::Native),
            n => n
                .parse::<u32>()
                .ok()
                .filter(|hz| (10..=240).contains(hz))
                .map(|hz| Self::Fixed(hz as f64)),
        }
    }

    /// The pacer for this setting; `Native` needs none.
    pub fn pacer(self) -> Option<FramePacer> {
        match self {
            Self::Native => None,
            _ => Some(FramePacer::new(self)),
        }
    }
}

/// The refresh period, from the intervals between rAF calls.
#[derive(Debug, Clone, Default)]
struct Refresh {
    intervals: VecDeque<f64>,
    period: Option<f64>,
    since_estimate: usize,
}

impl Refresh {
    fn push(&mut self, dt: f64) {
        if !(dt.is_finite() && dt > 2.0 && dt < STALL_MS) {
            return;
        }
        self.intervals.push_back(dt);
        if self.intervals.len() > REFRESH_SAMPLES {
            self.intervals.pop_front();
        }
        self.since_estimate += 1;
        // Often enough to follow a display that changes rate, rarely
        // enough to cost nothing.
        if self.period.is_none() || self.since_estimate >= 30 {
            self.period = estimate_refresh(self.intervals.make_contiguous());
            self.since_estimate = 0;
        }
    }
}

/// The slowest display rate whose whole multiples explain nine in ten of
/// the intervals, ms. At 120 Hz frames 25, 33 and 42 ms apart are three,
/// four and five refreshes; no rate slower than 120 explains them all.
/// With no rate fitting, the shortest interval.
fn estimate_refresh(intervals: &[f64]) -> Option<f64> {
    if intervals.len() < 8 {
        return None;
    }
    for hz in DISPLAY_HZ {
        let period = 1000.0 / hz;
        let tolerance = period * 0.12;
        let explained = intervals
            .iter()
            .filter(|&&dt| {
                let k = (dt / period).round();
                k >= 1.0 && (dt - k * period).abs() <= tolerance
            })
            .count();
        if explained as f64 >= intervals.len() as f64 * 0.9 {
            return Some(period);
        }
    }
    intervals.iter().copied().min_by(f64::total_cmp)
}

/// One drawn frame: when, how long since the last, and the player's work.
#[derive(Debug, Clone, Copy)]
struct Drawn {
    at: f64,
    interval: f64,
    work: f64,
}

/// p90 of a series.
fn p90(mut xs: Vec<f64>) -> f64 {
    xs.sort_by(f64::total_cmp);
    xs[((xs.len() - 1) as f64 * 0.9).round() as usize]
}

/// Decides which refreshes are drawn; see the module docs.
#[derive(Debug, Clone)]
pub struct FramePacer {
    setting: FrameRateSetting,
    refresh: Refresh,
    last_call: Option<f64>,
    /// Auto's rate, an index into `LEVELS`.
    level: usize,
    last_drawn: Option<f64>,
    /// The frame under way: its rAF time and when its work began.
    drawing: Option<(f64, f64)>,
    window: VecDeque<Drawn>,
    /// When the rate last changed (the window restarts then).
    switched_at: Option<f64>,
    climb_wait: f64,
    climbed_at: Option<f64>,
}

impl FramePacer {
    pub fn new(setting: FrameRateSetting) -> Self {
        Self {
            setting,
            refresh: Refresh::default(),
            last_call: None,
            level: 0,
            last_drawn: None,
            drawing: None,
            window: VecDeque::new(),
            switched_at: None,
            climb_wait: FIRST_CLIMB_MS,
            climbed_at: None,
        }
    }

    /// The display's refresh period, ms, once enough frames have shown it.
    pub fn refresh_ms(&self) -> Option<f64> {
        self.refresh.period
    }

    /// The target period of a rate, ms; `None` is every refresh.
    fn target_ms(&self, level: usize) -> Option<f64> {
        match self.setting {
            FrameRateSetting::Native => None,
            FrameRateSetting::Fixed(hz) => Some(1000.0 / hz),
            FrameRateSetting::Auto => LEVELS[level].map(|hz| 1000.0 / hz),
        }
    }

    /// Refreshes per drawn frame at a rate: the target as a whole number
    /// of refreshes, at least one.
    fn refreshes_at(&self, level: usize) -> u32 {
        match (self.target_ms(level), self.refresh.period) {
            (Some(target), Some(r)) => (target / r).round().max(1.0) as u32,
            _ => 1,
        }
    }

    /// Refreshes per drawn frame now.
    pub fn refreshes(&self) -> u32 {
        self.refreshes_at(self.level)
    }

    /// The period drawn frames fall apart, ms: the frame budget. (The
    /// clock is paced to the refresh, `refresh_ms`, so a frame that misses
    /// this still steps the whole refreshes it took.)
    pub fn period_ms(&self) -> Option<f64> {
        self.refresh.period.map(|r| r * self.refreshes() as f64)
    }

    /// A rAF call at `t` (its timestamp), with the work starting at `now`
    /// (`performance.now()`). True: draw this refresh.
    pub fn due(&mut self, t: f64, now: f64) -> bool {
        if let Some(last) = self.last_call {
            self.refresh.push(t - last);
        }
        self.last_call = Some(t);
        let due = match (self.last_drawn, self.refresh.period) {
            (Some(last), Some(r)) => {
                let n = self.refreshes() as f64;
                // Half a refresh early is this refresh, not the next.
                t - last >= (n - 0.5) * r
            }
            _ => true,
        };
        if due {
            self.drawing = Some((t, now));
        }
        due
    }

    /// The frame `due` let through has been drawn; `now` is when its work
    /// ended. Returns true when the rate changed.
    pub fn drew(&mut self, now: f64) -> bool {
        let Some((t, began)) = self.drawing.take() else {
            return false;
        };
        if let Some(last) = self.last_drawn {
            let interval = t - last;
            if interval > 0.0 && interval < STALL_MS {
                self.window.push_back(Drawn {
                    at: t,
                    interval,
                    work: (now - began).max(0.0),
                });
            }
        }
        self.last_drawn = Some(t);
        while self.window.front().is_some_and(|d| d.at < t - WINDOW_MS) {
            self.window.pop_front();
        }
        if self.switched_at.is_none() {
            self.switched_at = Some(t);
        }
        self.setting == FrameRateSetting::Auto && self.judge(t)
    }

    /// Auto: move to another rate if the window says to.
    fn judge(&mut self, t: f64) -> bool {
        let Some(r) = self.refresh.period else {
            return false;
        };
        let since = t - self.switched_at.unwrap_or(t);
        if since < WINDOW_MS - 1.0 || self.window.len() < MIN_FRAMES {
            return false;
        }
        let period = r * self.refreshes() as f64;
        let frames = p90(self.window.iter().map(|d| d.interval).collect());
        if frames > period * SLOW {
            // Straight to the fastest slower rate these frames would hold.
            let mut to = self.level;
            while to + 1 < LEVELS.len() {
                to += 1;
                let slower = r * self.refreshes_at(to) as f64;
                if slower > period && frames <= slower * SLOW {
                    break;
                }
            }
            if self.refreshes_at(to) <= self.refreshes() {
                return false;
            }
            // A climb that didn't hold: wait longer before the next.
            if self
                .climbed_at
                .is_some_and(|c| t - c <= 2.0 * WINDOW_MS + 1.0)
            {
                self.climb_wait = (self.climb_wait * 2.0).min(MAX_CLIMB_MS);
            }
            self.climbed_at = None;
            return self.switch(to, t);
        }
        if since < self.climb_wait - 1.0 {
            return false;
        }
        // The next faster rate that is really faster here.
        let Some(to) = (0..self.level)
            .rev()
            .find(|&l| self.refreshes_at(l) < self.refreshes())
        else {
            return false;
        };
        let faster = r * self.refreshes_at(to) as f64;
        let work = p90(self.window.iter().map(|d| d.work).collect());
        if work > faster * ROOM {
            return false;
        }
        self.climbed_at = Some(t);
        self.switch(to, t)
    }

    fn switch(&mut self, level: usize, t: f64) -> bool {
        self.level = level;
        self.window.clear();
        self.switched_at = Some(t);
        true
    }

    /// For the perf readout: "30 fps, 4 of 120 Hz (auto)".
    pub fn describe(&self) -> String {
        let how = match self.setting {
            FrameRateSetting::Native => "native",
            FrameRateSetting::Fixed(_) => "fixed",
            FrameRateSetting::Auto => "auto",
        };
        match self.refresh.period {
            Some(r) => {
                let n = self.refreshes();
                format!(
                    "{:.0} fps, {} of {:.0} Hz ({how})",
                    1000.0 / (r * n as f64),
                    if n == 1 {
                        "every".to_string()
                    } else {
                        format!("1 in {n}")
                    },
                    1000.0 / r
                )
            }
            None => format!("measuring the refresh ({how})"),
        }
    }
}

impl super::PlayerApp {
    /// Which refreshes to draw (#140): the web's `?fps=`. `Native`, the
    /// default, draws every one; the desktop's vsync pacing is unchanged.
    pub fn set_frame_rate(&mut self, setting: FrameRateSetting) {
        self.frame_pacer = setting.pacer();
    }

    /// The period drawn frames are paced to, seconds: the frame budget
    /// (`?scale=auto` judges frames against it). `None` when unpaced.
    pub(super) fn paced_period(&self) -> Option<f64> {
        self.frame_pacer
            .as_ref()
            .and_then(FramePacer::period_ms)
            .map(|ms| ms / 1000.0)
    }

    /// The pacing as the perf readout says it; `None` when unpaced.
    pub(super) fn frame_rate_line(&self) -> Option<String> {
        self.frame_pacer.as_ref().map(FramePacer::describe)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const HZ60: f64 = 1000.0 / 60.0;
    const HZ120: f64 = 1000.0 / 120.0;

    /// Drive a pacer through `secs` of a display at `refresh` ms, where a
    /// drawn frame costs `work(i)` ms; the browser skips the refreshes a
    /// frame overruns. Returns the intervals between drawn frames.
    fn play(
        p: &mut FramePacer,
        t: &mut f64,
        refresh: f64,
        secs: f64,
        mut work: impl FnMut(usize) -> f64,
    ) -> Vec<f64> {
        let end = *t + secs * 1000.0;
        let mut drawn = Vec::new();
        let mut last = None;
        let mut i = 0;
        while *t < end {
            let mut busy = 0.1;
            if p.due(*t, *t) {
                busy = work(i);
                i += 1;
                p.drew(*t + busy);
                if let Some(l) = last {
                    drawn.push(*t - l);
                }
                last = Some(*t);
            }
            // The next rAF is the first refresh after the work ends.
            *t += refresh * (busy / refresh).floor().max(0.0) + refresh;
        }
        drawn
    }

    fn spread(xs: &[f64]) -> (f64, f64) {
        let lo = xs.iter().copied().fold(f64::INFINITY, f64::min);
        let hi = xs.iter().copied().fold(0.0, f64::max);
        (lo, hi)
    }

    #[test]
    fn parses_the_query() {
        assert_eq!(
            FrameRateSetting::parse("30"),
            Some(FrameRateSetting::Fixed(30.0))
        );
        assert_eq!(
            FrameRateSetting::parse("60"),
            Some(FrameRateSetting::Fixed(60.0))
        );
        assert_eq!(
            FrameRateSetting::parse("auto"),
            Some(FrameRateSetting::Auto)
        );
        assert_eq!(
            FrameRateSetting::parse("native"),
            Some(FrameRateSetting::Native)
        );
        assert_eq!(FrameRateSetting::parse("0"), None);
        assert_eq!(FrameRateSetting::parse("fast"), None);
        assert!(FrameRateSetting::Native.pacer().is_none());
    }

    #[test]
    fn the_refresh_is_the_slowest_rate_that_explains_the_frames() {
        // Every refresh at 60 Hz.
        assert!((estimate_refresh(&[HZ60; 20]).unwrap() - HZ60).abs() < 1e-9);
        // 25, 33 and 42 ms on the Pixel: 3, 4 and 5 refreshes at 120 Hz.
        let pixel: Vec<f64> = (0..30).map(|i| HZ120 * (3 + i % 3) as f64).collect();
        assert!((estimate_refresh(&pixel).unwrap() - HZ120).abs() < 1e-9);
        // 2 and 3 refreshes at 60 Hz stay 60 Hz.
        let slow: Vec<f64> = (0..30).map(|i| HZ60 * (2 + i % 2) as f64).collect();
        assert!((estimate_refresh(&slow).unwrap() - HZ60).abs() < 1e-9);
        // 144 Hz isn't mistaken for 120.
        let fast = [1000.0 / 144.0; 20];
        assert!((estimate_refresh(&fast).unwrap() - 1000.0 / 144.0).abs() < 1e-9);
        assert!(estimate_refresh(&[HZ60; 3]).is_none());
    }

    #[test]
    fn thirty_is_every_other_refresh_at_60_and_every_fourth_at_120() {
        let mut t = 0.0;
        let mut p = FrameRateSetting::Fixed(30.0).pacer().unwrap();
        let drawn = play(&mut p, &mut t, HZ60, 3.0, |_| 5.0);
        let (lo, hi) = spread(&drawn[10..]);
        assert!(
            (lo - 2.0 * HZ60).abs() < 0.01 && (hi - 2.0 * HZ60).abs() < 0.01,
            "{lo} {hi}"
        );

        let mut t = 0.0;
        let mut p = FrameRateSetting::Fixed(30.0).pacer().unwrap();
        let drawn = play(&mut p, &mut t, HZ120, 3.0, |_| 5.0);
        let (lo, hi) = spread(&drawn[10..]);
        assert!(
            (lo - 4.0 * HZ120).abs() < 0.01 && (hi - 4.0 * HZ120).abs() < 0.01,
            "{lo} {hi}"
        );
        assert_eq!(p.refreshes(), 4);
        assert!((p.period_ms().unwrap() - 33.33).abs() < 0.01);
    }

    #[test]
    fn jitter_under_the_period_is_absorbed() {
        // The Pixel: frames that cost 17 to 30 ms at 120 Hz came 25, 33
        // and 42 ms apart. Paced to 30 they are all 33.
        let mut t = 0.0;
        let mut p = FrameRateSetting::Fixed(30.0).pacer().unwrap();
        let costs = [17.0, 24.0, 30.0, 20.0, 28.0];
        let drawn = play(&mut p, &mut t, HZ120, 5.0, |i| costs[i % costs.len()]);
        let (lo, hi) = spread(&drawn[20..]);
        assert!(
            (lo - 33.33).abs() < 0.01 && (hi - 33.33).abs() < 0.01,
            "{lo} {hi}"
        );
    }

    #[test]
    fn auto_holds_the_display_rate_when_frames_keep_up() {
        let mut t = 0.0;
        let mut p = FrameRateSetting::Auto.pacer().unwrap();
        let drawn = play(&mut p, &mut t, HZ60, 30.0, |_| 4.0);
        assert_eq!(p.refreshes(), 1);
        assert!(drawn.iter().all(|&d| (d - HZ60).abs() < 0.01));
    }

    #[test]
    fn auto_drops_a_slow_120_hz_phone_straight_to_30() {
        let mut t = 0.0;
        let mut p = FrameRateSetting::Auto.pacer().unwrap();
        let costs = [17.0, 24.0, 30.0, 20.0, 28.0];
        play(&mut p, &mut t, HZ120, 10.5, |i| costs[i % costs.len()]);
        assert_eq!(p.refreshes(), 4, "{}", p.describe());
        let drawn = play(&mut p, &mut t, HZ120, 60.0, |i| costs[i % costs.len()]);
        // A minute of play: p90 on 33.3, and it stays there.
        let p90 = p90(drawn.clone());
        assert!((p90 - 33.33).abs() < 2.0, "{p90}");
        assert_eq!(p.refreshes(), 4);
        assert_eq!(p.describe(), "30 fps, 1 in 4 of 120 Hz (auto)");
    }

    #[test]
    fn auto_climbs_back_when_the_work_shrinks_and_backs_off_a_failed_climb() {
        let mut t = 0.0;
        let mut p = FrameRateSetting::Auto.pacer().unwrap();
        play(&mut p, &mut t, HZ60, 10.5, |_| 25.0);
        assert_eq!(p.refreshes(), 2);
        // Light work again: back to every refresh after a window.
        play(&mut p, &mut t, HZ60, 11.0, |_| 4.0);
        assert_eq!(p.refreshes(), 1);

        // Work that fits half a period at 30 but not a whole one at 60:
        // the climb fails, and the next waits twice as long.
        let mut p = FrameRateSetting::Auto.pacer().unwrap();
        let mut t = 0.0;
        let mut work = 25.0;
        play(&mut p, &mut t, HZ60, 10.5, |_| work);
        assert_eq!(p.refreshes(), 2);
        work = 8.0;
        play(&mut p, &mut t, HZ60, 10.5, |_| work);
        assert_eq!(p.refreshes(), 1, "climbed");
        work = 25.0;
        play(&mut p, &mut t, HZ60, 10.5, |_| work);
        assert_eq!(p.refreshes(), 2, "fell back");
        assert_eq!(p.climb_wait, 2.0 * FIRST_CLIMB_MS);
        work = 8.0;
        play(&mut p, &mut t, HZ60, 12.0, |_| work);
        assert_eq!(p.refreshes(), 2, "still waiting");
        play(&mut p, &mut t, HZ60, 9.0, |_| work);
        assert_eq!(p.refreshes(), 1, "climbed again");
    }

    #[test]
    fn a_fixed_rate_never_moves() {
        let mut t = 0.0;
        let mut p = FrameRateSetting::Fixed(60.0).pacer().unwrap();
        play(&mut p, &mut t, HZ120, 2.0, |_| 5.0);
        assert_eq!(p.refreshes(), 2);
        // Frames too slow for it don't lower it (nor can they show the
        // refresh: every interval is four of 120 Hz, or two of 60)...
        let drawn = play(&mut p, &mut t, HZ120, 25.0, |_| 30.0);
        assert!(drawn[10..].iter().all(|&d| (d - 33.33).abs() < 0.01));
        // ...and light ones are back at 60 at once, no window to wait.
        let drawn = play(&mut p, &mut t, HZ120, 2.0, |_| 5.0);
        assert_eq!(p.refreshes(), 2);
        assert!(drawn[drawn.len() - 10..]
            .iter()
            .all(|&d| (d - HZ60).abs() < 0.01));
    }
}
