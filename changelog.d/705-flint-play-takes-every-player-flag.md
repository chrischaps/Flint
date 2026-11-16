### `flint play` takes every `flint-player` flag (#705)
**What changed.** `flint play` and `flint-player` share one set of flags,
`flint_player::PlayerArgs`, and one launch, `flint_player::launch`. Their
`--help` now differs only by `flint-player`'s `--bundle`. `flint play` gains
`--msaa`, `--quality`, `--render-scale`, `--look`, `--seed`, `--frame-step`,
`--screenshot`, `--screenshot-at`, `--screenshot-every`, `--exit-after`,
`--input-script`, `--stats`, `--perf`, `--compare`, `--split`,
`--pixels-per-point`, `--procgen-defer`, `--touch-controls`, `--safe-area`
and `--touch-device`. A scene plays the same under either command: on the same
scene and flags the two wrote byte-identical screenshots. A `--style` word with
bad syntax is refused when the flag is parsed, on both. Before, only
`flint play` checked it that early. A game binary that embeds the player can
flatten `PlayerArgs` into its own clap command and call `launch`, or call
`PlayerApp::step_frames(step, seed)` for `--frame-step`'s seeding rule.
**What to do.** Nothing. A capture script that called `flint-player` only to
reach these flags can call `flint play` now.
