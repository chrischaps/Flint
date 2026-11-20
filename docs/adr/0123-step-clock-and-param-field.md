# ADR 0123: One stepped clock, and the parameter field's general form

Status: accepted, 2026-11-15 (#747, epic #675). Builds on #213 (the wooze
field), #214 (the shading boil), #405 (motion on twos) and #754 (`flint-wooze`
out of `flint-core`).

## Context

Two general ideas had grown up inside one aesthetic and under its name.

- **A stepped clock.** `flint_wooze::warp(beats, glide)` and its `eased` are
  "hold a target, then ease into the next over the last `glide` of a step":
  animation on twos with an optional glide. `Cycle`, `BoilClock` and
  `ShallowClock` wrapped it. Motion on twos (#405) then needed "which tick is
  this time in" and wrote its own `floor(time * rate + TICK_EARLY)` in
  `flint_ecs::motion_hold`, which pencil's `boil` also used. Two clocks did the
  same thing without knowing about each other, and the 2026-11-10 refactoring
  audit found a fourth about to be written.
- **A parameter field.** `WoozeField` is global values, zones (sphere or box,
  soft falloff, laid over each other in order) and per-value ramps, so a
  sample is a pure function of position and time. Nothing in that machinery
  is about wooze except the four names (`amplitude`, `step`, `glide`,
  `jitter`), their ranges and the presets, but they were hard-coded through
  it (`WoozeSample`'s fields, `KNOB_NAMES`, `ZONE_KEYS`). "This part of the
  level is foggier" or "windier" could not reuse it.

## Decision

1. **`flint_core::step_clock` is the one stepped clock.** It holds `warp`,
   `eased`, `Cycle` (now taking the glide and the carried amount as numbers,
   not a `WoozeSample`), `STEP_MIN`, `TICK_EARLY`, `tick(time, rate)` and
   `StepClock`, a running count of beats advanced by `dt / step` with whole
   beats kept apart from the fraction.
   - `tick` is `warp` with no glide on `time * rate` beats. It counts in `f32`
     as the old floor did and widens exactly, so every tick lands where it did
     (a unit test compares the two at every 1/60 s step of a minute at 8, 12
     and 24 a second).
   - `flint_ecs::motion_hold` re-exports `tick` and `TICK_EARLY`, so a skinned
     entity held at `rate = 12`, a pencil `boil = 12` and a living prop's clock
     all step through the same function. `flint_wooze` re-exports `warp`,
     `Cycle`, `CycleNow` and `STEP_MIN`, so its users are unchanged.
   - `BoilClock` is a `StepClock` with the film-frame cadence on top.
     `ShallowClock` and the living props keep their single `f64` count: a
     split count rounds differently, and they must keep the beat they had.
2. **`flint_core::field::ParamField<N>` is the field's general form**: `N`
   named numbers over a `FieldKeys<N>` (the field's name for messages, the
   key names, a check per key, optional presets), with the zones, falloff,
   ramps, `sample`, `sample_key(name, ..)`, `zones_at`, `zones_in`,
   `set_global`, `add_zone`, `set_zone` and `remove_zone` written once. A zone
   table takes `SHAPE_KEYS` (`name shape center radius size falloff`) and the
   key names.
3. **`WoozeField` is a `ParamField<4>` over `flint_wooze::WOOZE_KEYS`**, a
   newtype that reads samples back as `WoozeSample`. The knobs' ranges, the
   presets, `ZONE_KEYS` and every message stay as they were, so `[wooze]`
   scenes, `flint validate` and the Rhai `set_wooze*` functions see no
   difference. `[wooze]` stays the authored name for the aesthetic.
4. **The key set stays closed for now.** `N` is fixed when a field is made, at
   compile time, so a sample stays an array on the stack and allocates
   nothing, which the grove's tens of thousands of reads a frame need. A field
   whose keys a scene declares (`[field.weather] keys = ["wind", "fog"]`) and
   that Rhai samples by name (`field_sample("wind", pos)`) is the next step;
   `sample_key` is the shape that call would take. It would need either a
   `ParamField` over a heap key list, sampled into a caller's buffer, or one
   `ParamField<1>` per key.

## Alternatives considered

- **Leave the clock in `flint-wooze` and have motion hold depend on it.** The
  smallest change, but motion hold lives in `flint-ecs`, which flint-particles
  reads and which has no business depending on an aesthetic's crate; and the
  next consumer (an animator's own `step` and `glide`, #505's and #529's
  script access) would have had to look for a clock under "wooze" again.
- **`StepClock` everywhere, the props included.** Cleaner, but the props'
  beat is one `f64` today, and swapping in a whole-plus-fraction count moves
  some phases by a rounding step, which would change living renders. The
  issue's done-when says the living goldens don't change.
- **An open key set at once** (keys as strings, samples as a map). It would
  allocate per sample, or need a caller-held buffer threaded through every
  consumer, for a need nobody has yet; and nothing authors such a field.
- **Put the field in `flint-wooze` under a general name.** #754 moved wooze
  out of `flint-core` so its edits rebuild less, but the general field and
  clock are small, have no dependencies beyond `toml`, and change rarely; a
  crate that wants fog by place should not link the wooze presets.

## Consequences

- The skinned animator held on twos, pencil's boil, the shading boil, shallow
  wooze and the living props read one clock. A change to how a tick lands is
  one edit, and `pencil::tests::the_boil_steps_on_the_living_props_clock`
  holds pencil to `warp`.
- Measured: `flint render --time` frames of the living props, the woozy grove,
  the shading boil, shallow wooze and motion hold are byte-identical to the
  same frames from main's binary (#747's PR lists the cases).
- A new field is a `static FieldKeys<N>` and a `ParamField::new`; it gets
  zones, ramps and their error messages with nothing more. It has no scene
  table or Rhai surface of its own until a follow-up gives it one.
- Book: concepts/procgen.md, "The wooze field".
- `Cycle::read` takes `(beats, glide, amount)`; a caller outside the tree that
  passed a `&WoozeSample` passes `s.glide, s.amplitude`.

## Decision 4, taken (2026-11-20, #1014)

A scene declares a field (`[field.<name>] keys = { wind = { min, max, default } }`
with `[[field.<name>.zone]]` entries), and scripts read it with
`field_sample("weather", "wind", pos)`. Of the two storages decision 4
named, the field is a `ParamField` over a heap key list (`SceneField`),
not one `ParamField<1>` per key: a `FieldKeys` is `'static` with a `fn`
check, so a key per field would have leaked a key set per declared key and
lost its range, and a script samples one key at a time anyway
(`SceneField::sample_index`), which reads the one list slot and allocates
nothing. The zone readers are one generic code path over both (`ZoneOf<S>`,
the private `KeySet`), so the wooze field's zones read and refuse as they did
and `ParamField<N>` samples the same bits; a test holds a `SceneField` to a
`ParamField<2>` with the same keys, bit for bit. The write functions are
`field_set`, `field_set_zone` and `field_remove_zone` rather than
`set_field*`, which would overload the component setter `set_field`.
