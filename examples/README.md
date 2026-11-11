# Flint examples

Curated scenes that show what the engine does, each listed in
[`index.toml`](index.toml) with a topic and a one-line purpose.

```bash
cargo run --bin flint -- examples                 # every example, by topic
cargo run --bin flint -- examples --topic living  # one topic
cargo run --bin flint -- play examples/showcase_tavern.scene.toml
cargo run --release --bin flint -- render examples/pbr_test.scene.toml -o pbr.png
```

Improve them freely. Nothing pins their pixels: the render goldens and CLI
tests load frozen copies under [`tests/fixtures/scenes/`](../tests/fixtures/scenes/README.md).
A new scene here needs an entry in `index.toml`, and
`flint examples --check` (run by CI) fails until it has one and passes
`flint validate`. A scene that only a test needs belongs in the fixtures
instead.

The folders beside the scenes (`models/`, `textures/`, `scripts/`, `particles/`,
`styles/`, ...) are the examples' assets, found by the engine's asset search.
`film/` is the showcase film's set (`tools/showcase`).
