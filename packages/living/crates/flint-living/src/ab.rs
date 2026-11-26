//! A second look at the same grove (#238): a fork on the same clocks, a
//! shared pose cache, and forgetting what the bone buffers held.

use flint_wooze::Cycle;

use super::ring::PoseCache;
use super::{LivingProps, LivingStats, TINT_UNSENT};

impl LivingProps {
    /// The same props on the same clocks, for a second look at them beside these
    /// (#238): the knobs of one can then turn without the other's. Its lineages,
    /// rings and poses are these, and shared, since a pose is an `Arc`. Its pose
    /// cache starts empty: the two take turns with one, through
    /// [`process_beside`](LivingProps::process_beside),
    /// since a pose doesn't depend on any knob and either side can use the other's.
    pub fn fork(&self) -> Self {
        Self {
            lineages: self.lineages.clone(),
            props: self.props.clone(),
            // Each side keeps its own species values (#246), as it does the rest.
            species: self.species.clone(),
            cache: PoseCache::new(self.cache.max_bytes),
            time: self.time,
            pose_estimate_ms: self.pose_estimate_ms,
            stats: LivingStats::default(),
            defaults: self.defaults.clone(),
            reholding: self.reholding,
            rehold_cursor: self.rehold_cursor,
            rehold_pass: self.rehold_pass,
            rehold_turn: self.rehold_turn,
        }
    }

    /// Lend each other a pose cache: what one side made, the other finds.
    pub fn swap_cache(&mut self, other: &mut LivingProps) {
        std::mem::swap(&mut self.cache, &mut other.cache);
    }

    /// Forget what every bone buffer, echo and tint was last given, so that the
    /// next blend writes all of it: the buffers now hold something else, such as
    /// the other side of an A/B (#238). No clock moves and no cycle starts over, so
    /// what's written is what would have been anyway.
    pub fn redraw_all(&mut self) {
        for prop in &mut self.props {
            prop.redraw();
            // Not cleared: a cleared tint reads as step 0, and a part really on
            // step 0 would then never be sent, keeping the other side's.
            prop.drawn.tints.fill(TINT_UNSENT);
            prop.echoes_changed = true;
        }
    }

    /// Start every part's cycle over at the beat it's on, reading the field as it
    /// is from the next tick: what a part keeps of the glide and drift it started
    /// its cycle with is gone, and it shows the knobs as they are now. For a look
    /// that has to be what its preset says at once, not from each part's next
    /// beat (#238). Where the knobs haven't changed since a cycle began, nothing
    /// it draws changes.
    pub fn restart_cycles(&mut self) {
        for prop in &mut self.props {
            prop.drawn.bones.parts.clear();
            prop.shadow_cycle = Cycle::default();
            for echo in &mut prop.echoes {
                echo.drawn.parts.clear();
            }
            prop.redraw();
        }
    }
}
