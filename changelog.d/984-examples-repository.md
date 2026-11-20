### `engine/examples/` is gone: the examples are the flint-examples repository (#984)
**What changed.** The curated example scenes left the engine's tree for
[flint-examples](https://github.com/chrischaps/flint-examples), with their
history, the scenes at its root and the engine as a squashed subtree at
`engine/`, like a game; the showcase film's rig (`tools/showcase/`) went with
them. A game's `engine/` no longer carries them (45 MB less). `flint
examples` with no `--dir` now reads the project root's `index.toml`, so it
works in a flint-examples checkout; in a game, pass `--dir`. The engine's
schemas, including the room-builder layer `engine/schemas/examples`, stay.
**What to do.** A game that names a file under `engine/examples/` (a model,
a texture, a scene in a transition) copies it into its own project first;
`flint check <dir>` lists the misses. A sparse-checkout line
`'!/engine/examples/'` can go. To read or run an example, clone
flint-examples.
