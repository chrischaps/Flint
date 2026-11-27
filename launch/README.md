# launch/

`flint build` writes a game's launcher here, `launch/<game id>/`, when a package the game lists has Rust of its own (#1562, ADR 0137 decision 6). It is a member of this workspace, so the engine's crates it builds on stay compiled, and it is gitignored. This file keeps the folder there: a `members` glob whose folder is missing fails every cargo command.
