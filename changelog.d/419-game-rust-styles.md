### A game binary can add a Rust style: `stylize::register` and `PlayerExtension::styles` (#419)
**What changed.** `flint_render::stylize::register(&'static StyleDescriptor)`
adds a style written in the game's own crate, and a player extension's new
`styles()` hook hands descriptors to `add_extension`, which registers them. A
registered style is listed in F4 (tagged `game`), read from
`[post_process.styles.<name>]` and `--style`, drawn by the player, and listed
with `"source": "game"`. A taken name (a built-in, a loaded `.shader.toml`
style, another game style) or a taken `component` is refused. A user
`.shader.toml` style can no longer take a game style's name, and a hot
reload never replaces one. `PlayerExtension` gained a method with a default,
so existing extensions build unchanged.
**What to do.** Nothing, unless you want a Rust style: build a `static`
descriptor with `StyleDescriptor::new` and return it from `styles()`, adding
the extension before the event loop starts. Example:
`crates/flint-player/examples/cyanotype/`.
