// Swings its entity about z like a metronome's arm: a beat each way,
// 100 beats a minute. A frame that stalls shows as the arm jumping.

const REACH = 28.0;  // degrees either side
const BEAT = 0.6;    // seconds a beat (one way)

fn on_update() {
    let t = total_time() * PI() / global::BEAT;
    set_rotation(self_entity(), 0.0, 0.0, global::REACH * sin(t));
}
