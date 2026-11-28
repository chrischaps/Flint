### A terrain noise layer takes `range = [from, to]`, so `multiply` by perlin no longer flips the land (#1695)
**Package.** terrain
**What changed.** A `.terrain.toml` noise layer's `range` maps its raw values (perlin, simplex and value -1..1, ridged 0..1, now listed by `flint gen --describe terrain_v1`) onto `from..to`, held there, before `amplitude`. A `multiply` by a raw perlin layer turned the land upside down wherever the noise was negative: Relief's recipe made speckle instead of an island for 4 seeds in 16. With `range = [0.0, 1.0]` every seed is land. The terrain editor has a Range box under Amplitude. A layer without `range` bakes the same bytes as before.
**What to do.** Give a `multiply` layer `range = [0.0, 1.0]`.
