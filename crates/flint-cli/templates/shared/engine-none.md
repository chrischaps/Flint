This project was made with `--engine none`: it names no engine. `flint.toml` lists only the game's own empty `schemas`, so an engine archetype resolves to nothing and `flint schema transform` finds nothing. The scene still validates and renders, because a component is a plain table whether or not a schema describes it, but a scene that leans on an archetype's defaults will not play as written.

To name an engine, carry it as a git subtree, the pattern Flint's games use:

```bash
git init
git add .
git commit -m "Start {{name}}"
git remote add flint-engine https://github.com/chrischaps/Flint.git
git subtree add --prefix=engine flint-engine main --squash
```

Then set `schemas = ["engine/schemas", "schemas"]` in `flint.toml`. The engine's changelog, schemas and book are then under engine/ in this project.
