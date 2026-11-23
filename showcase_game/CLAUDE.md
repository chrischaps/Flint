# showcase_game: The Overgrown Ruins (moved out of the root CLAUDE.md by #992)

The example game, kept in the checkout as a project of its own. The film rig moved with the examples (#984): `flint-examples/film/CLAUDE.md`, in that repository.

## In the checkout (#760)

- `showcase_game/` -- The Overgrown Ruins, the example game: a project with its own `flint.toml`, assets and sounds (its bartender's model is its own copy, #984), the engine beside it (`../schemas`) rather than under it. A top-level directory with a `flint.toml` is a game kept in the checkout; tests find one by its manifest, so engine code never names it (#760), and `crates/flint-cli/tests/checkout_games_cli.rs` fails when one names a file it doesn't have
