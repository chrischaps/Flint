# ADR 0065: The opening is a treadmill flight with credits in the fog

Status: accepted, 2026-08-27 (Starchild; no Flint commit).
Backfilled 2026-11-12 (#374) from the Vault note "Starchild Cinematic
Opening Treadmill" (2026-08-27) and its Session Log line. Nothing in Flint
cites it.
The original record was in the Starchild repo, which no longer exists; this
is a reconstruction.

## Context

A Starchild game decision: the game's opening, an endless flight through
the cloud sea ([ADR 0063](0063-cloud-sea-as-sculpted-meshes.md)) with the
opening credits drifting out of the fog. Chris wanted to fly the loop while
composing the intro music.

## Decision

From the Vault note:

- A treadmill. The camera only bobs at the origin while the world scrolls
  past and wraps every 220 units, so the loop is seamless and nothing
  accumulates float error.
- The sea is a z-periodic tile from the cloud generator; opposite edges
  match and the seam is closed bitwise-exact with welded normals. Two copies
  of the tile leapfrog, so the swap lands on identical geometry.
- Credits are generated PNGs on world-space quads that ride the scroll,
  faded by fog and a scripted opacity envelope.
- Everything is a function of `total_time()`, with the clock read marked as
  the place conducted beats will be substituted when a `music_session` is
  wired in.
- Two scenes (`intro.scene.toml` and a clay-look variant) share one script.

## Alternatives considered

Not recorded in the surviving text.

## Consequences

- The note records two Flint limits met on the way: box primitives have
  world-space UVs and only a scalar `texture_scale`, with V flipped against
  the PNG, and transparent draws cast fully opaque shadows with no
  per-entity opt-out.
- The look and whether the ~31 s sea repeat shows were left as Chris's [H]
  call.

## Sources

- Vault: "2026-08-27 Starchild Cinematic Opening Treadmill"; "Session Log"
