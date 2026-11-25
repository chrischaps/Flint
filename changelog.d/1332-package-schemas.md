### `living_prop`, `shallow_wooze`, `music_session`, `[living]` and `[wooze]` live in their packages (#1332)
**What changed.** Their schema files moved from `schemas/` to
`packages/living/schemas/` and `packages/music/schemas/`. A schema directory
with the engine's package manifests beside it (`engine/schemas` in a game
with the engine as a subtree, a `--schemas` path to a Flint checkout) now
brings each package's `schemas/` in right after it, so the same components
and tables load, in the same layer order, for every command, the player, a
bundle, Android and the web. `flint schema living_prop` names the new file.
**What to do.** Nothing. A game that lists `engine/schemas` gets them as
before; only a game that copied the engine's `schemas/` folder somewhere
without `packages/` beside it loses them, and copies `packages/` too.
