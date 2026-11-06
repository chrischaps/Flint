# Woozy grass: the first target of #252's experiment pass

The Woozy Grove (`demo/woozy_grove.scene.toml`, #217) grown over with a meadow whose
grass keeps the trees' time. It's the third target on #251's shortlist
(`docs/design/living-props-beyond-trees.md`), tried first because it's the one
everyone walks through.

```sh
cargo build --release --bin flint
python demo/woozy_grove_grass/experiment.py      # every variant, three places, ~5 minutes
cargo run --release --bin flint -- play demo/woozy_grove_grass.scene.toml
```

`make_mask.py` rebuilds `mask.png`, the grass's density: worn along the paths and
round the bench, a bare foot at every trunk, thinner in the hush.

## What it does

`"grass.wooze" = true` on a grass field does two things (the book's Living Props
chapter, *Woozy grass*, has the detail):

- **The clock.** The sway runs on the wooze field's hold-and-glide clock, read
  where the camera stands, instead of the wall clock. A beat is worth the step
  where the clock started, so the authored wind speed holds on average and the
  sway slows or speeds where the field does.
- **The comb.** Each beat leans every tip by shallow wooze's world-space noise,
  read at the blade's root. A patch leans together, and the next beat lays the
  meadow down another way. The noise and the beat are the bench's.

Grass without it draws exactly as before: 31 of 31 demo scenes render
byte-identically, and the three that differ from themselves run to run
(`mesh_meadow` at 2 s, `rolling_meadow` at 0 and 2 s) differ by 0 to 6 pixels,
as much as the old binary differs from itself.

## The variants, measured

`experiment.py` renders each one from the north path (the scene's camera, in the
clearing's `linklater` field), from inside the hush and from inside the shiver.
The measure takes a crop that holds only grass, with the shading boil off, since
the boil redraws every pixel 12 times a second. With no grass at all, the hush
and shiver crops move on 0 of 47 frame pairs. The north one moves a little
(0.21), which is the trees' shadows crossing bare ground.

Frame pairs at 12 fps that change, over 4 s (`#` moves, `.` holds):

| Variant | Knobs | North (linklater) | Hush (step 1.6, glide 0.6) | Shiver (step 0.125, no glide) |
|---|---|---|---|---|
| `wall` | as it was | 47/47, mean 1.79 | 47/47, 0.37 | 47/47, 0.85 |
| `clock` | `wooze = true`, `wooze_amplitude = 0` | 38/47, 1.68 | 23/47, 0.46 | 31/47, 1.05 |
| `comb` | as authored: 0.14 m, 2.5 m patches | 37/47, 2.16 | 25/47, 0.47 | 32/47, 2.75 |
| `deep` | 0.3 m, 1.2 m patches | 40/47, 2.42 | 24/47, 0.52 | 32/47, 3.52 |

The holds land where the field says. In the hush a beat is 19.2 frames, and the
grass holds for about 8 and moves for 11: `........###########........###########`.
In the shiver it snaps two frames in every three. The comb doesn't change when
the grass moves, only how much, and it more than doubles the change in the shiver.
A unit test (`the_comb_takes_the_fields_amplitude_and_a_benchs_beat`) holds the
grass and a bench standing in the same field to the same target and weight
every frame.

## Verdicts, for Chris to confirm by eye

| Variant | Verdict | Why |
|---|---|---|
| The clock | **keep** | Measured: it rests on the field's beat in all three places, where the wall clock never rests. In the hush it holds 8 frames in 19, which is that zone's point |
| The comb at 0.14 m / 2.5 m | **keep**, as the default | Measured: same beats as the clock alone, about a quarter more change on the north path and more than double in the shiver. From stills, each beat lays patches down a new way. Whether that reads as redrawn or as blown is the call to make by eye |
| The deep comb, 0.3 m / 1.2 m | **park** | From stills, windswept patches that lean like gusts, and the path's edges start to wander. Worth trying again on a taller grass |
| The shiver's tempo | **watch** | At step 0.125 a beat is worth 0.5 s of sway, so the sway runs four times faster there, in snaps. It may read as the thorns' boil or as a dropped frame |

The keep ones are finished in this change: an opt-in field, tested, in the book and
in the Grass Debug panel (F3, *Wooze*), so no follow-up issue is needed for them.

## What to look at in each GIF

In `renders/woozy_grass/`, each is `<variant>_<place>.gif`.

- **`wall_*` against `clock_*`:** does the meadow now rest with the trees, and does
  the stop-start read as drawn or as a stutter?
- **`clock_*` against `comb_*`:** does each beat look like the same meadow redrawn?
  Watch the path's edges: a comb that's too strong makes the path wander.
- **`comb_hush`:** the meadow should nearly hold.
- **`comb_shiver`:** the fast-snap question above.
- **In `flint play`:** walk from the clearing into the shiver and back. The tempo
  bends as you cross and shouldn't jump. Grass far off in another zone keeps
  *your* beat, not its own: if that reads wrong from the clearing, the grass needs
  a clock per patch, which this didn't build.

## How it differs from the trees

- A tree's parts each keep a phase offset, so a tree never holds all at once. The
  grass has none, like shallow wooze, so the whole meadow holds and moves
  together. That's what makes it read as one drawing, and it's also the likeliest
  thing to feel mechanical.
- A tree's drift is its own regrown shape. The comb's is noise. It's the jelly risk
  #251 named for the noise pose, here at blade scale, where it reads more like wind.
- The grass has one clock for the whole field, read at the camera. The trees read
  the field where each one stands.
