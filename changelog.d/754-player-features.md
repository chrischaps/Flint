### flint-player's `music`, `procgen` and `living` features (#754)
**What changed.** The music, procgen at run time and living props are Cargo
features of `flint-player`, on by default, as `debug-hud` and `rhythm` are.
`music` plays a `music_session`; `rhythm` now turns it on. `procgen` makes a
`model.asset` or texture from a `*.procgen.toml` spec and gives extensions
`register_generators`; `living` (which needs `procgen`) runs living props,
`[living]`, `[wooze]`, shallow wooze, woozy grass and their script modules.
A build without one links none of its crates (`--no-default-features` links
neither flint-music nor flint-procgen), and a scene that uses it prints one
line at its entry saying what won't run. flint-debug-ui's Living Props panel
is its own `living` feature, and the workspace now takes that crate with its
default features off.
**What to do.** Nothing for a game binary that takes `flint-player`'s
defaults. One that sets `default-features = false` adds the features its
scenes use, for example `features = ["music", "living"]`.
