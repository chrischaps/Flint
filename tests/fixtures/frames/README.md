# Frozen frame captures (#1087)

Two captures of one tiny scene, for `flint frame`'s tests
(`crates/flint-cli/tests/frame_cli.rs`): `two_crates.flintframe` is
`two_crates.scene.toml` (a ground, a crate, a lantern box and a sun), and
`one_crate.flintframe` is the same scene with the lantern taken out. Each
is about 35 KB: 64 x 36 images and a 128-texel shadow map, so they sit well
inside the tests' size budget (ADR 0122, `scripts/size-check.py`).

They were made by `flint render`, and that is how to make them again:

```bash
flint render tests/fixtures/frames/two_crates.scene.toml --schemas schemas \
  --width 64 --height 36 --no-grid --shadow-resolution 128 -o target/two.png \
  --frame-capture tests/fixtures/frames/two_crates.flintframe
flint render tests/fixtures/frames/one_crate.scene.toml --schemas schemas \
  --width 64 --height 36 --no-grid --shadow-resolution 128 -o target/one.png \
  --frame-capture tests/fixtures/frames/one_crate.flintframe
```

- **These are version 1 manifests, kept as written.** ADR 0126 has every
  reader keep reading every older version, and this pair is how that is
  held: when the manifest's version goes up, leave these alone (the tests
  must still read them) and add a capture of the new version beside them.
  Regenerating them in place would lose the old version's test.
- The manifests name the adapter that drew them (an RTX 5070 Laptop on
  Vulkan, 2026-11-16) and their GPU times: the tests read numbers back
  from the files and never compare them with a new render.
- A test that needs a capture from today's writer makes one itself
  (`frame_cli.rs`'s GPU test, which skips without a GPU), so the writer and
  the reader can't drift apart unseen.
