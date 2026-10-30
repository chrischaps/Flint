# Flint on the Web

Status: M0 spike (#95), async startup (#99), the vfs (#97), platform shims
(#100), sound (#101), game bundles (#98), first light (#102),
[play.chaps.dev](#playchapsdev-103) (#103), [Flint Kart](#flint-kart-104)
(#104) and Firefox's frame rate ([#121](#firefoxs-cadence-121)) done, epic #94. The browser entry runs the real `PlayerApp`: it fetches a game bundle
with a progress bar, serves every read from it through the vfs, opens the
WebGPU device without blocking and draws to a page canvas that follows the
window's size. A Begin gate starts the sound and locks the pointer with one
click. SET + DRIFT (Float) plays whole in Chrome
([First light: Float](#first-light-float-102)), and so does a whole Flint
Kart race against five AI racers, with keys or a pad
([Flint Kart](#flint-kart-104)).
A bare page with no bundle still plays the First Light scene, made of the
engine's own geometry.

## Try it

```bash
rustup target add wasm32-unknown-unknown
cargo check --target wasm32-unknown-unknown -p flint-player   # the whole player
cd crates/flint-player
trunk serve                 # http://127.0.0.1:8080, the player on the First Light scene
trunk serve shims.html      # the shim check (#100): sky clear, time, pad, rebinds;
                            #   sound behind a Begin click (#101)
trunk build --release       # dist/, with wasm-opt -Oz (trunk fetches binaryen)
trunk build --release --cargo-profile web-release   # what the site ships (#123, Size)
```

CI keeps this compiling (#96). The `wasm32 check` job in
`.github/workflows/ci.yml` runs `scripts/wasm-check.sh` on every PR: a wasm32
clippy pass over `flint-player` and the workspace crates it links, with every
lint off but `disallowed_methods`, configured in
`scripts/wasm-clippy/clippy.toml` to deny `Instant::now`, `SystemTime::now`
and their `elapsed()`. Those are the calls that compile but panic. #100
moved the 32 sites that predated the check to `web-time` and removed
their allows, so a std time call in a runtime crate now fails the check.

The page is `crates/flint-player/index.html` and builds the `flint-web` bin
(`src/bin/flint_web.rs`), the web twin of `main.rs`. Its arguments come from
the query string instead of clap:

| Query | Desktop flag | Meaning |
|---|---|---|
| `game=<id>` | | a game play.chaps.dev packed: the bundle at `games/<id>/<id>.flintpack` ([play.chaps.dev](#playchapsdev-103)). `bundle` wins over it |
| `embed=1` | | the iframe view: the world and its Begin button, no wordmark, title or hints; the full-screen button only where the iframe allows it ([Full screen](#full-screen-135)) |
| `webgpu=0` | | show the no-WebGPU film in any browser, to check it |
| `bundle=<url>` | `--bundle` | a `.flintpack` from `flint bundle`, relative to the page. Its manifest names the scene and schemas; its late bundles are fetched beside it |
| `scene=<url>` | `<scene>` | with `bundle`: a scene inside it instead of the manifest's. Without: a scene TOML fetched relative to the page. Default `scenes/web_first_light.scene.toml`, copied from `web/scenes/` by trunk |
| `msaa=1\|4` | `--msaa` | scene-pass MSAA |
| `scale=<f>\|auto` | `--render-scale` | the 3D scene's render scale ([Render scale](#render-scale-122)). By default a page draws the scene at no more than DPR 1.5 and the HUD at the full DPR; a number from 0.25 to 1 draws at that scale whatever the DPR (`scale=1` is native); `auto` keeps the cap and steps under it to hold 60 fps |
| `width=&height=` | `--width --height` | fix the canvas to that many CSS pixels; otherwise it fills the page |
| `frame_step=<s>` | `--frame-step` | step game time exactly s seconds a frame |
| `hold_at=<s>` | (`--screenshot-at`) | stop game time at s seconds (`PlayerApp::hold_at`), so a page holds an exact moment for a screenshot |
| `perf=1\|cpu` | `--perf` | the perf readout in a corner (#120): frame times, each phase's CPU time, the slowest scripts and each pass's GPU time ([The perf readout](#the-perf-readout-120)) |
| `loop=poll` | | winit's flat-out `ControlFlow::Poll`, as before #121, to compare against the default of one frame per `requestAnimationFrame` ([Firefox's cadence](#firefoxs-cadence-121)) |

The engine schemas are compiled in (`build.rs` embeds `schemas/components`
and `schemas/archetypes`), since a browser can't list a directory. A
bundle's own schema directories load over them.

## Game bundles (#98)

A browser has no files, so a game ships as one or two fetched blobs. `flint
bundle <game>` packs the game folder; `flint_core::bundle` is the format.

```bash
flint bundle ../Float                     # dist/float.flintpack, dist/float.audio.flintpack
flint bundle ../Float --list --dry-run    # every file, its size and its bundle; writes nothing
flint-player --bundle dist/float.flintpack   # play it on the desktop, every read from the bundle
# web: put both files next to index.html and open ?bundle=float.flintpack
```

**Format.** `FLINTPAK`, a `u32` format version, a `u32` manifest length, the
manifest as TOML, then every file's bytes back to back, uncompressed. The
manifest names the game, the start scene, the schema directories in load
order, the late bundles and each file's path, offset and size. Most of a
game is PNG, GLB and Ogg, compressed already; a host can still gzip the
whole file on the wire. `bundle::total_len` reads the real size from the
first bytes, so the progress bar is right even when Content-Length is the
gzipped size.

**What goes in.** Everything under the game root but the default excludes:
dot-folders, `target/`, `dist/`, `docs/`, `screenshots/`, `snapshots/`,
`renders/`, `reference/`, `scratch/`, `tmp/`, `logs/`, `tools/`, `src/`,
`engine/` (except `engine/schemas/`), Markdown, shell and Python scripts,
source art (`.blend`, `.psd`, `.kra`, `.xcf`) and Cargo files. A
`bundle.toml` in the game root adds to them:

```toml
name = "float"
scene = "scenes/float.scene.toml"
schemas = ["engine/schemas", "schemas"]   # the default
exclude = ["scenes/mixamo_spike.scene.toml", "scenes/snap_*.scene.toml"]
include = []          # wins over any exclude
[late]                # replaces the default; fetched in this order
ambience = ["audio/ocean_loop.ogg", "audio/wind_light.ogg"]
audio = ["**/*.ogg", "**/*.wav", "**/*.mp3", "**/*.flac"]
```

**Audio comes late.** By default every audio file goes to a second bundle the
player fetches once it is running. A browser won't start sound before the
first click anyway (#101), so nothing is lost by playing the first frames
silent. When a late bundle lands the web entry installs the grown source
(`vfs::install`), `vfs::generation()` moves, and the player runs
`load_audio_from_world` again: sounds already loaded are skipped. An
autoplay `audio_source` whose sound isn't loaded yet now waits unsynced
(`flint-audio/src/sync.rs`) instead of being marked done, so the ocean loop
starts when its file arrives. One-shot sounds a script fires before then are
lost, the same as with a missing file. `flint-player --bundle` adds the late
bundles three seconds in, so the desktop takes the same path.

**Ambience first, then decoded a little a frame (#124).** Late parts are
fetched one after another in the order `[late]` lists them, and a file goes to
the first part that matches, so a game puts what it plays from the first
second in a small part of its own: Float's ocean and wind are 0.33 MB before
6.78 MB of songs and one-shots. Nothing is decoded in the frame it lands.
`load_audio_from_world` with `queue` (every web load, and the late-bundle
path everywhere) hands each sound to `AudioEngine::queue_sound`, and
`tick_av_systems` calls `decode_queued` with 2 ms a frame
(`AUDIO_DECODE_BUDGET`). `flint_audio::decode` does what kira's
`StaticSoundData::from_cursor` does, one symphonia packet at a time, into an
`Arc<[Frame]>` sized from the stream's frame count, so finishing a song costs
no copy (the copy alone was a 67 ms frame). The queue takes audible autoplay
sources first, then the rest smallest first, so one-shots come before songs.
A script that plays a queued sound decodes it on the spot if its file is
64 KB or less (`DECODE_NOW_BYTES`); a bigger one goes to the front and that
play is skipped, with a warning that says so. kira's `StreamingSoundData`
was the other way. It decodes on a thread of its own
(`std::thread::spawn` in kira's `decode_scheduler.rs`), and this wasm build
has none.

Measured in headless Chrome 156, the line throttled to 1.5 MB/s from the main
bundle on (job tooling `arrive.mjs`, the `?perf=1` readout):

| | before (play.chaps.dev, main) | after |
|---|---|---|
| sea heard | with everything, after a 900�983 ms frame (+5.7 s and +27 s after Begin in two runs) | decoded and started +0.81 to +0.85 s after Begin (four runs, one unthrottled) |
| worst frame while the audio lands and decodes | 900 and 983 ms (1017/1600/800 ms unthrottled) | 16.8�16.9 ms (three runs). A first cut, 4 ms a frame and a `Vec` copied into the `Arc` at the end, still had 50�67 ms frames as each song finished |
| late audio buffers | 35�41 | 9�15, all in the first second's startup frames, which both builds share |
| decoding cost | one frame | 49 sounds in 0.7�0.8 s of frame time, spread over about 6 s; each song over 64�71 frames, longest step 2.6 ms |

The first second after Begin still has 250�930 ms frames in both builds:
startup, not sound.

**Checked before it's written.** A missing file should fail in the packer,
not as a pink texture deep in play. Before writing, the packer:

- loads every packed scene from the packed files, with the bundled schemas,
  expanding prefabs through the vfs, and fails on an archetype no schema
  defines;
- reads every string in scenes, prefabs, particle effects and chunks that
  looks like a file (a known extension after a stem), plus each
  `model.asset`, and fails if none of the places the runtime looks (beside
  the file, one level up, and their `models/`, `scripts/`, `audio/`,
  `textures/`, `sprites/` ... folders) holds it. A model may also be a
  `<name>.procgen.toml` the player builds;
- does the same for string literals in Rhai scripts. A name a script builds
  at run time (`"splash_" + n + ".ogg"`) can't be checked; the literal
  `".ogg"` has no stem and is skipped.

Packing Float on the first try failed this way: `scenes/mixamo_spike.scene.toml`
names `mixamo_body.glb`, which isn't in the repo. Float's `bundle.toml`
leaves that spike scene out, with the `snap_` scenes kept for reference
renders.

**Float**, packed with the `bundle.toml` above (Float at `7b37e1e`, its
engine subtree's schemas):

| Bundle | Files | Size | gzip -9 |
|---|---|---|---|
| `float.flintpack` (scenes, schemas, scripts, models, textures, sprites) | 92 | 3.66 MB | 1.59 MB |
| `float.audio.flintpack` (49 Ogg files, 5.3 MB of it the four music beds) | 49 | 7.11 MB | 6.96 MB |

`models/body.glb` is 2.45 MB of the main bundle on its own.

**In Chrome** (release build, `?bundle=float.flintpack`, network throttled to
1.5 MB/s over CDP): the 14.2 MB wasm took about 10 s, then the status line
counted `float.flintpack 0.3 / 3.7 MB` up to 3.7 over 2.3 s. The scene loaded
from the bundle (44 entities) and drew Float's raft, body and ocean while
`audio 0.2 / 7.1 MB` filled over the next 4.8 s, and the bar faded on
`late bundle audio: 49 files`. The frame then stopped on a `std::time`
panic right after startup ("time not implemented on this platform"): this
was measured before #100 moved those sites to `web-time`. With every engine read now live, the release
wasm is 14.2 MB after `wasm-opt -Oz`, 5.1 MB gzipped (it was 9.4 / 3.5 MB
while the vfs left file-reading code dead).

## play.chaps.dev (#103)

**One address for every game.** `play.chaps.dev/?game=float` plays SET +
DRIFT, and the next game is one more `?game=`. A subdomain per game was the
other choice. One site wins because the player is the same 7 MB of wasm for
every game, so a visitor who has played one has the engine cached for the
next. It also needs one DNS record and one Pages site, and a new game is a
few lines in `tools/play-site/games.toml`, not a new repo.

**What the site is.** `.github/workflows/play.yml` builds it on every push
to `main` that touches the player, and on a manual run:

1. `trunk build --release --cargo-profile web-release` in
   `crates/flint-player`, the player page (the profile is under Size);
2. `cargo build -p flint-cli`, for `flint bundle`;
3. `python tools/play-site/build.py`, which fetches each game in
   `games.toml` at its pinned commit (a GitHub tarball; nothing to clone),
   packs it with `flint bundle --name <id>` into `games/<id>/`, and writes
   `games.json` (what the page shows) and `built-from.txt` (the Flint
   commit and each game's);
4. `actions/upload-pages-artifact` and `actions/deploy-pages`. This repo's
   Pages site serves the result, with `play.chaps.dev` as its custom domain.

A pull request that touches the page, the bundler or the site builds it the
same way, as a trial, and publishes nothing; the site it would have deployed
is the run's `github-pages` artifact. Soba deploys by pushing to a Pages
repo with a deploy key. This deploy uses Pages from Actions instead: there
is no new secret, and no repo's history grows by 25 MB of wasm and bundles
on every push.

A game is pinned to a **commit**, not a branch, so a push to a game's repo
can't change or break the site between Flint deploys. Moving a game forward
means changing its `ref`. Float is pinned to its `engine-catchup` branch
(#102): the engine subtree at Flint `main`, plus its `bundle.toml`.

**The doorway.** Before any wasm is fetched, an inline script in
`index.html` decides whether this visit can play:

- no `?game=` (or `bundle`/`scene`) and a `games.json` beside the page:
  the chooser, a card per game with its poster;
- a `?game=` the site doesn't have: the chooser, saying so;
- no `navigator.gpu`, or no adapter from `requestAdapter()`: the game's
  film from chaps.dev, full-bleed and muted, with the reason and the
  browsers that play it. A missing adapter gets its own reason (hardware
  acceleration off, or a blocklisted GPU);
- otherwise the gate takes the game's title and line from `games.json`
  (not the scene file's name), and the player loads.

`Trunk.toml` makes this hold. Its `pattern_script` loader awaits
`window.flintDoorway`, a promise the doorway resolves only when the game
can play. Its `pattern_preload` preloads only the 90 KB JS glue. Trunk's
default loader also preloads the wasm, so even a browser that can't play
would have downloaded the whole wasm (7 MB). Under `trunk serve` there is no `games.json`
and the page plays what the query names, as before. `shims.html` has no
doorway, so it awaits `undefined` and starts at once.

**Embed.** `?game=float&embed=1` is for iframes. It shows the world and its
Begin button, with the progress hairline centred low, and no wordmark,
title or hints. Without WebGPU it shows the film with the reason, without
the caption or the browser list. To embed it:

```html
<iframe src="https://play.chaps.dev/?game=float&embed=1"
        allow="autoplay; fullscreen; gamepad" allowfullscreen
        style="width: 100%; aspect-ratio: 16 / 9; border: 0"></iframe>
```

A click on Begin inside the iframe is the iframe's own user activation, and
it starts the sound and locks the pointer. `allow="gamepad"` lets the pad
through to a cross-origin frame.

**Checked locally** (the site assembled by `build.py` from Float at
`c06e1c9`, served by `python -m http.server`):

| Visit | Browser | Result |
|---|---|---|
| `/` | headless Chrome 156 | the chooser; 0.45 MB fetched (the poster), no wasm |
| `/?game=float` | headless Chrome, WebGPU | the gate reads SET + DRIFT; the world runs behind it from the bundle |
| `/?game=float` | Firefox 159, `dom.webgpu.enabled` off | the film and "SET + DRIFT plays in the browser through WebGPU, and this browser doesn't offer it". The server log shows the page, the JS glue and `games.json`, and no wasm or bundle |
| `/?game=float&embed=1` in an 800x450 iframe on another origin | headless Chrome | Begin only; a real mouse click on it lifted the gate onto Float's title screen |

**Setting it up** (once, by hand; none of it is in the repo):

1. Porkbun DNS for chaps.dev: a `CNAME` record, `play` → `chrischaps.github.io`.
2. This repo's Pages, built by Actions, on that domain:
   `gh api -X POST repos/chrischaps/Flint/pages -f build_type=workflow`, then
   `gh api -X PUT repos/chrischaps/Flint/pages -f cname=play.chaps.dev`, and
   **Enforce HTTPS** once GitHub has issued the certificate. WebGPU needs a
   secure context.
3. Run the **Play site** workflow (or push to `main`).

**Not done here.** Safari can't be run from Windows. Firefox on macOS was
stood in for by Firefox on Windows with WebGPU turned off, which takes the
same path. The threaded AudioWorklet build would need cross-origin isolation
headers, which GitHub Pages can't set (the issue's second risk). Nothing
needs them yet: #101 renders sound on the main thread.

## Flint Kart (#104)

Flint Kart (chrischaps/FlintKart) is the second game on the site, and the
first with a race loop: a gamepad-first menu, a countdown, six karts with
items and drift boosts, five of them Rhai AI racers, laps, standings and a
results screen. `play.chaps.dev/?game=kart`.

**The engine catch-up.** The newest game was on `feature/trike-backports`
(2026-09-03, FlintKart PR #1, which carries items-and-powerups too), with
its subtree at Flint `81e54fc`. Unlike Float's, its last squash's split is
still in Flint, so one `subtree pull --prefix=engine engine main --squash`
brought 166 commits in with no conflicts, and `HEAD:engine` is
byte-identical to Flint `main`'s tree. FlintKart's `engine-catchup` branch
is that pull plus `bundle.toml` and `web/`. Every scene ran natively on the
new engine with no warning from the content, and the bundle checker passed
on the first pack. Nothing in the game needed changing.

**The bundle.** It starts on `scenes/main_menu.scene.toml` and leaves out
the render captures and `logo.png` at the repo root (the menu draws
`sprites/logo.png`), the personal `mountain_switchback_chris` scene and the
2.2 MB `Panorama_Lush` skybox only that scene names, the source music,
`tracks/` (the input to `build_branching_track.py`) and `web/`:

| Bundle | Files | Size |
|---|---|---|
| `kart.flintpack` | 108 | 3.31 MB |
| `kart.audio.flintpack` (24 Ogg files) | 24 | 1.20 MB |

The first pack, without those excludes, was 6.61 MB.

**A film of its own.** chaps.dev has no film of Flint Kart, so the game
ships one: `web/kart-race.mp4` (36 s, 5.2 MB) and `web/kart-poster.jpg`. A
`video` or `poster` in `games.toml` may now be a path in the game's repo,
not only a URL, and `build.py` copies it beside the bundle
(`games/kart/kart-race.mp4`). A game with a poster and no video gets the
poster as a still on the film door. A game can also give the gate its own
`controls` line, since "mouse to look" is Float's and a kart has no mouse.

**Persist on the web.** The menu remembers the chosen track with
`persist_set`. The player saved the store to `<project>/save/persist.toml`
with `std::fs`, which on wasm failed and warned once a second for the rest
of the session. On wasm the store is now a `user_files` entry named for
the input config's `game_id`, `flint:/config/flint/save_flint_kart.toml`
in `localStorage`, so every game on the one origin keeps its own. A
failed write marks the store clean instead of retrying every second.

**Racing it in Chrome.** A person can't be scripted, so the test gives one
a coach. A test-only copy of the game adds `coach_brain`, the game's own
`ai_driver.rhai` aimed at `player_kart` but writing to its own `ai_input`,
and a reporter that logs the coach's throttle, steering, brake, drift and
item once a frame. A CDP driver reads those lines and plays them the way a
person would. With **keys** it holds W and S, pulses A and D (sigma-delta,
so the share of time a key is down follows the analog steer) and presses
Shift and E. With a **pad** (`navigator.getGamepads` faked with a standard
mapping) it moves the left stick and pulls RT and LT, with LB to drift and
X for items. The menu is driven the same way: Enter or A twice. The kart
only ever sees the keyboard or the gamepad.

| Run (release, 1280x720, DPR 1) | Player | Mean frame | p99 | Late audio buffers |
|---|---|---|---|---|
| keys, headless Chrome 156 | 4th, 1:24.12 | 16.79 ms | 16.8 ms | 5 of 2,161 |
| pad, headed Chrome 156 | 2nd, 1:17.12 | 16.73 ms | 16.8 ms | 5 of 1,946 |
| pad, headless Chrome 156 | 3rd, 1:20.45 | 19.65 ms | 33.3 ms | |
| native, Vulkan, Fifo (a coach-driven copy) | 3rd, 1:20.41 | 17.62 / 19.21 ms (two runs) | | |

Every late buffer came while the track scene loaded. In the headless pad
run, `requestAnimationFrame` stopped for 13 s in three gaps of 3, 3 and
7 s while the audio clock kept mixing on time, so the main thread was
idle. That is headless Chrome not producing frames. The same race headed
had no gap after loading. Native vsync on this machine paces unevenly
(present-to-present p99 44 to 148 ms, with stalls to 1 to 2 s). Virtual
Desktop and Meta virtual monitors are attached, the Fifo case CLAUDE.md
describes. So the mean frame time in Chrome is **0.87 to 0.95x** native's,
not 1.5x.

**Where the time goes.** These were measured with a per-phase timer this
change first carried, which summed each phase's CPU time over the race and
counted fixed steps. #120's perf readout (`?perf=1`, `flint-player --perf`)
landed first and measures the same phases, so the timer was dropped in
favour of it; to repeat this table, read the readout's phase line over a
race. Mean ms per frame over the race, release builds, same machine
(native uncapped with `FLINT_VSYNC=0`):

| Phase | Native | Chrome, headed | Chrome, headless |
|---|---|---|---|
| scripts (`on_update`: 6 karts, 5 AI brains, the coach, race manager) | 3.15 | 2.41 | 3.21 |
| physics (rapier, ~1.06 fixed steps a frame) | 0.01 | 0.02 | 0.03 |
| ...per fixed step | 0.017 | 0.024 | 0.024 |
| script ui (`on_draw_ui`: the HUD) | 1.86 | 1.76 | 2.41 |
| audio/anim/fx | 0.48 | 0.58 | 0.85 |
| sync | 1.17 | 1.31 | 1.99 |
| render (encode and submit) | 2.49 | 1.80 | 2.90 |
| **CPU per frame** | **9.18** | **7.96** | **11.45** |

**rapier3d is not the hazard the issue feared.** FlintKart's kart physics
live in `kart_controller.rhai` (the karts ride the track spline), and the
scene declares no colliders, so rapier steps a near-empty world: about
0.02 ms a step on both targets. It is 1.4x slower in
the browser, which matters for a game that leans on rapier, and is noise
here. The AI racers are Rhai, and the scripts cost about the same in wasm
as natively. Chrome's `performance.now()` is coarsened, so one frame's
phases are noise. These are sums over 4,800 to 5,100 frames.

The cost the desktop doesn't pay is **sound on the main thread**. Six
karts' engine, tyre and wind loops take a mean 4.9 ms to mix per 46 ms
buffer, about 1.8 ms of every 16.7 ms frame, on top of the frame's own
work. In headless Chrome that leaves about 3.5 ms of headroom at 60 Hz. A
slower machine would drop frames or sound first. Soba's threaded
AudioWorklet build (it needs cross-origin isolation headers) is the
answer if it does.

**Not run:** Safari, Firefox (#102's open performance problem applies),
and a person with a real pad. The pad here is `navigator.getGamepads`
faked with the standard mapping, which gilrs' web backend reads like a
real one. Flint Kart's own quirk, seen in the race: when a shield ends,
`kart_visuals.rhai` restores the player's kart to white (`racer_info`'s
default), not its red body.

## First light: Float (#102)

SET + DRIFT (chrischaps/Float) is the first whole game on the web: the
ocean pipeline, the sky and its day, weather and rain, the birds, the
whale, reality tears, music and sea, look and scoot.

**The engine catch-up.** Float's `engine/` subtree was byte-identical to
Flint `1595dfa` (2026-07-28), but its squash recorded a split commit
(`6ce46b9`) that Flint no longer has, so `git subtree pull` failed. Float's
`engine-catchup` branch drops the subtree and re-adds it at Flint `main`
with `--squash`, so later pulls work again. Two pieces of content broke
or had always been broken, and are fixed there:

- `time_of_day.day_length_ramp` is `type = "array"` with no `element`,
  which Flint now reads as strings (it warned on load). It says
  `element = "float"` now.
- `water_ambience.rhai` played `dunk_in_N`, `dunk_out_N` and `drip_N` by
  bare name. The cache holds `audio/<name>.ogg`, so the eye going under
  was silent on every platform. The web day run's console caught it.

Float's `bundle.toml` is committed there too.

**The page.** One click does what a browser allows only from a gesture.
`index.html`'s Begin gate shows the scene's `name` and `description` over
the running, blurred world. Its click calls `flint_player::web_begin()`,
which locks the pointer to the canvas there and then and asks the player
to `start_audio()` next frame (`player_app/web_page.rs`).

**Pointer lock.** A script's `set_cursor_captured(true)` is a wish on the
web. Each frame `sync_pointer_lock` reads `document.pointerLockElement`:

- while a script wants the cursor and the page holds a gesture's
  activation (`navigator.userActivation.isActive`), it asks for the lock,
  at most once a second, and catches the refusal so it never reaches the
  console;
- mouse motion turns the view only while the lock is held, so a pointer
  crossing the page before Begin turns nothing;
- when the browser lets go (Escape is the browser's, and winit never hears
  of it), the player releases the cursor as Escape does on a desktop. Float's
  click-to-recapture then asks again, and the page shows "click to look
  around" until it holds.

Escape with the cursor free no longer exits on the web: a page has
nowhere to exit to.

**Comparing at a fixed time.** `?frame_step=&hold_at=` stops game time at
an exact second (`PlayerApp::hold_at`). Float's title day runs 24 h in
60 s from 10:30, so 06:00 is game time 48.75 s:

```bash
flint-player scenes/float.scene.toml --schemas engine/schemas --schemas schemas \
  --width 1280 --height 720 --frame-step 0.0166666667 \
  --screenshot native.png --screenshot-at 48.75 --exit-after 49
# web: ?bundle=float.flintpack&width=1280&height=720&frame_step=0.0166666667&hold_at=48.75
```

Chrome 156's frame against the desktop's (Vulkan, RTX 4070), 1280x720:
mean absolute difference 1.1/255; the sky band matches to 0.01/255. The
2.9% of pixels more than 16/255 apart sit on the edges of cloud shadows
and halftone dots, a sub-pixel shift of the same pattern.

**A day in Chrome** (headless Chrome 156, `--enable-unsafe-webgpu`, DPR 1,
1280x720 canvas, release build, Begin clicked and the title dismissed, 14
minutes: days 1, 2 and 3 whole, into day 4): mean frame time **16.72 ms**
from the page's own `requestAnimationFrame` (50,557 frames). p99 was
16.8 ms through minute 12 and 33.3 ms in minutes 13 and 14, at night. No
console errors. 18,145 audio buffers, **7 late**, all after 12:50, when
the main thread went 84 to 128 ms between buffers. That this was night
turned out to be chance: night costs nothing (#120, [below](#the-perf-readout-120)).
Keyboard scoot, a gamepad's left stick (scoot) and right stick (look)
all moved the seat, with the pad faked through `navigator.getGamepads`.

**Pointer lock in Chrome** (headed: headless Chrome never grants the
lock): Begin locked the canvas; `exitPointerLock` showed the hint; a click
locked it again and hid the hint. The window must have focus.

**A day in Firefox 159** (headed, driven over WebDriver BiDi, DPR 1.5,
same canvas and steps, 14 minutes): it draws everything Chrome does, from
the same bundle, with no console errors. But it is slow, and that is the
open problem. Mean frame time **103.8 ms** (8,190 frames). p50 sits at
83.3 ms, five 60 Hz refreshes, and a stall of 1.2 to 2.9 s comes every few
seconds. The game clock clamps those stalls to 250 ms, so game time fell
behind wall time: day 3 began at 507 s, not 395 s. Sound breaks up: 9,842
of 18,442 buffers were late. The flat 83 ms cadence looks like
presentation, not work. Firefox doesn't show a WebGPU canvas the way
Chrome does. Profiling it is follow-up work, not part of this change.
(It wasn't presentation: [Firefox's cadence (#121)](#firefoxs-cadence-121)
found the loop and the skinned draws, and Firefox now holds 16.7 ms.)

**Not run:** Safari (no way to run it from Windows), and a person
listening on speakers and looking with a real mouse and pad.

## The perf readout (#120)

`?perf=1` puts the numbers in a corner, twice a second
(`player_app/perf.rs` keeps them, `player_app/web_perf.rs` draws them into
the page's `#perf` panel):

- **frame**: p50, p99 and max over the last 10 s, from a
  `requestAnimationFrame` loop of the page's own, timed by rAF's
  timestamps. Timing the player's callbacks instead showed jitter that no
  one sees (p99 23 ms where rAF said 16.8).
- **worst**: the longest gap between the player's frames, and how much of
  it was the player's own work. A long frame with little work in it is the
  browser's, not ours.
- canvas pixels and DPR, the adapter as the browser names it (wgpu 23's
  WebGPU backend leaves `AdapterInfo` empty, so the page asks
  `navigator.gpu` itself), draw calls and triangles, and late audio buffers
  (`AudioEngine::buffer_stats`).
- **cpu ms**, mean and max per phase of the frame: input, scripts,
  physics, script ui, anim/fx, sync (procgen, renderer sync, late audio),
  render (draw lists, encoding, submission), hud, present.
- **scripts**: the four with the worst single frame, by file, and the
  script system's own steps in brackets (`[hot reload]`, `[load, on_init]`,
  `[events]`); `ScriptSystem::set_update_timing`.
- **gpu ms** per pass from `timestamp-query` (ADR 0108), where the adapter
  has it. The post chain is now a scope per effect (`SSAO`, `Bloom`,
  `Volumetric`, `Kuwahara`), not one `Post-process`. Without timestamps the
  panel says so and the CPU phases are all it has. `?perf=cpu` leaves GPU
  timing off; on Chrome it made no difference to the frame times.

**Copy line** puts the whole readout on one line for an issue:

```
flint perf · float · t=34s | Chrome 156 · nvidia lovelace | 1280x720 DPR 1 | frame p50 16.7 p99 16.8 max 16.8 ms (600 frames/10s) | cpu 9.07 ms: … | gpu 2.71 ms: Shadows 1.40, Scene 0.29, SSAO 0.69, Bloom 0.30, … | 71 draws 50.4K tris | audio 0/948 late
```

`flint-player --perf` prints the same line every 10 s, so a desktop can
be set beside a browser.

### What night costs: nothing

All in headless Chrome 156 on the RTX 4070 laptop, DPR 1, 1280x720 canvas,
release build, the bundle play.chaps.dev serves (Float `c06e1c9`).

**Held at one moment.** `?frame_step=&hold_at=` stops Float's title day at
noon (3.75 s) or midnight (33.75 s), stars and all. 60 s each, alternated:

| | rAF p99 | CPU ms/frame | GPU ms/frame | Shadows | Scene | SSAO | Bloom |
|---|---|---|---|---|---|---|---|
| noon (two runs) | 16.8 | 9.0 to 9.6 | 2.6 to 3.6 | 1.1 to 1.8 | 0.2 to 0.4 | 0.7 to 1.3 | 0.2 to 0.5 |
| midnight (two runs) | 16.8 | 8.8 to 9.6 | 2.4 to 3.2 | 1.1 to 1.6 | 0.2 to 0.6 | 0.5 to 1.2 | 0.2 to 0.5 |
| noon, `perf=cpu` | 16.8 | 8.9 to 10.0 | off | | | | |
| midnight, `perf=cpu` | 16.8 | 8.7 to 9.1 | off | | | | |

The stars, the night sky and the night sea cost nothing measurable, on
the CPU or the GPU.

**Fifteen minutes from the title** (into day 3's night), a perf line
every 10 s: the player's work never moved. CPU 9.6 to 11.3 ms a frame,
GPU 2.3 to 4.9 ms, sound 0 late of 19,656 buffers, the main thread never
more than 80 ms between audio buffers. But from 250 s to 670 s the frames
came unevenly: rAF p50 33 ms in some windows, p99 up to 133 ms, single
gaps of 780 to 987 ms. Those windows were day as often as night (the sky
was at full daylight at 240, 300, 540 and 600 s), and the darkest stretch,
780 to 900 s, ran at p99 16.8 ms. The worst frames held 9 to 18 ms of the
player's work against 250 to 987 ms of wall time, and audio kept being
fed through them, so the main thread wasn't blocked: the browser held
frames back. Nothing in the player's phases or passes explains them; this
laptop was shared with other work while it ran, and #102's 33 ms minutes
falling at night were the same thing.

**Where the web's CPU goes** (not night, but the largest number on the
panel): of ~9.5 ms a frame, **sync** is ~5.9 ms (renderer sync with the
world, procgen, post overrides), render ~2.0 and scripts ~1.4. On the
desktop sync is ~1.8 ms of ~5.3. It is the first place to look when a
phone or a slow laptop falls behind (#121, #122). #121 found it: the
skinned draws were rebuilt every frame, and sync is now ~0.3 ms.

**On the desktop** (`--perf`, Vulkan): the scripts phase spiked to 30 to
80 ms every few seconds, once 1.5 s, and the readout named `[hot reload]`,
the check of each script's modified time every frame. Windows file stats
stall now and then. The web has no disk to check, so it never pays it.

## Render scale (#122)

The surface is the canvas's CSS size times `devicePixelRatio`, so a DPR 2
laptop pays four times DPR 1's pixels in every full-screen pass. Now the
3D scene can be drawn below the surface's size and the HUD at full DPR:

- `SceneRenderer::set_render_scale(device, s)` sizes the HDR chain (HDR
  buffer, bloom mips, SSAO, volumetric, Kuwahara, MSAA targets, the ocean
  grab) at `s` of the surface on each side, `0.25..=1`. The composite
  already samples the HDR buffer by UV, so drawing it into the full-size
  surface is the upscale (bilinear).
- Below 1 the scene draws against a depth buffer of its own size
  (`render_scale.rs`, `ScaledDepth`), and a fullscreen pass copies it,
  nearest texel, into the caller's surface-sized depth view after the
  composite (`depth_upscale.wgsl`). World lines, world text and the
  viewer's overlays keep testing against the scene there, unchanged.
- At scale 1 none of it exists and the frame is drawn as before. Desktop
  captures of `showcase_tavern` and `shoreline` (1600x900, fixed step,
  held at 3 s) match main's pixel for pixel.

**The policy** is the player's (`player_app/render_scale.rs`): a ceiling,
which is the fixed scale capped further by pixel ratio, and an optional
dynamic step under it. A page caps at DPR 1.5 (`WEB_DPR_CAP`), so DPR 2
draws at 0.75 a side, 56% of the pixels, and DPR 3 at 0.5. `?scale=<f>`
(or `--render-scale`) draws at that scale whatever the DPR; `?scale=1` is
native. `auto` keeps the cap and steps down by eighths of it, to half,
after two 2 s windows whose median frame is over 1.2x the refresh period
(or 60 Hz's, whichever is longer), so a 30 Hz display isn't over budget.
It tries a step back up after 10 s calm; if that step is over budget in
its first two windows it comes back down at once, and the next try waits
twice as long, up to 160 s, so a scene on the edge doesn't pump. Stalls
over 250 ms are ignored, and `frame_step` turns it off. The desktop
default is scale 1 with no cap.

The perf readout says `scene 1440x810 px, scale 0.75 (56% of the pixels)`
under the canvas line, and F2's stats show `1600x900, scene 800x450`.

**Measured** (Chrome 156 over CDP, DPR 2 emulated, RTX 4070 laptop,
release build, Float held at 06:00 with `?frame_step=&hold_at=48.75`):

| | scene | GPU ms/frame | Shadows | Scene | SSAO | Bloom | Composite |
|---|---|---|---|---|---|---|---|
| `?scale=1` | 1920x1080 | 3.32 | 1.42 | 0.50 | 1.16 | 0.19 | 0.04 |
| default (cap 1.5) | 1440x810 | 2.80 | 1.23 | 0.33 | 0.95 | 0.16 | 0.11 |

On this GPU the frame was never short of time (rAF p50 16.7 ms either
way), and the shadow cascades don't depend on the surface, so the saving
is small here. It grows with fill rate: on the desktop tavern
(1600x900, Kuwahara + SSAO), GPU time went 6.39 → 4.18 → 2.42 ms at
scales 1, 0.75 and 0.5, SSAO alone 2.56 → 0.46 ms at half. Composite
costs a little more upscaling than copying one to one. The depth copy
isn't a timed scope of its own.

The capped frame against native, both at DPR 2: mean absolute difference
0.86/255, 0.94% of pixels more than 16/255 apart (less than #102's browser
against desktop). The HUD's text ("SET + DRIFT", "PRESS ANY KEY") is the
same pixels give or take the scene showing through behind it. Float's
halftone dots are a world-space grid, so they stay where they are; their
antialiased edges are a scene pixel wide, so they're a little softer.

## Firefox's cadence (#121)

Firefox 159 on Windows drew Float at p50 83.3 ms, five refreshes, with a
stall of 1 to 3 s every few seconds and half the audio buffers late.
There were two causes, one on top of the other, and neither was Firefox's
presentation: the sky-only shim page held 16.7 ms in the same browser
(99% of 5,373 frames).

All of these runs are headed Firefox 159 over WebDriver BiDi, Float's
title from `?bundle=float.flintpack&perf=cpu`, DPR 1.5 on a 2100x1223
canvas, timed by a `requestAnimationFrame` loop of the driver's own. They
were made before the [render scale](#render-scale-122) landed, so the
scene drew at the full DPR; a quarter of the pixels changed nothing here.

| build | frames | p50 ms | p90 ms | mean ms | over 250 ms | audio late |
|---|---|---|---|---|---|---|
| before: `ControlFlow::Poll` (90 s) | 893 | 83.3 | 83.3 | 101.8 | 13 | 933 of 2,086 |
| the loop on rAF (5 min) | 10,858 | 16.7 | 33.3 | 23.7 | 5 | 9 of 6,481 |
| and skinned draws cached (5 min) | 17,947 | 16.7 | 16.7 | 16.7 | 0 | 0 of 6,480 |

The last run's worst frame was 66.7 ms; 17,912 of its frames were one
refresh. The player's CPU went from 10.2 ms a frame to 3.2 to 4.7, and
`sync` from 6.1 to 0.3.

**The loop.** `flint_web.rs` ran winit with `ControlFlow::Poll`, which on
the web posts the next turn of the loop with `scheduler.postTask` as soon
as the last one ends, so the main thread never went idle. Chrome still
ran its frames at 16.7 ms; Firefox's refresh driver got one turn in five.
Now the loop waits (`ControlFlow::Wait`), `about_to_wait` asks for a
redraw, and winit draws in the browser's `requestAnimationFrame`.
`?loop=poll` brings the old loop back to compare. Chrome is unchanged
(p50 16.7, p99 16.8 either way), and its worst `sync` frame went from
15 to 18 ms to 7 to 8.

**The garbage.** That left a third of the frames at two refreshes and a
hitch of about 250 ms every 12 s. Neither changed at a quarter of the
pixels (`width=640&height=360`), so it wasn't fill. The Firefox Profiler
(`MOZ_PROFILER_STARTUP`, read from its JSON) put each hitch at the end of
a major GC, `ALLOC_TRIGGER` after 44.7 MB of JS allocation, with the main
thread nearly idle through it; the cycle collector was clearing 130,000
refcounted objects every 6 s. Wrapping the page's `GPUDevice` and
friends counted, per frame:

| per frame | before | after |
|---|---|---|
| `createBuffer` | 386 | 0.5 |
| `createBindGroup` | 327 | 36 |
| `writeBuffer` | 435 (314 KB) | 243 (292 KB) |
| `submit` | 23 | 22 |

A build without `wasm-opt` keeps the wasm's names, so the JS stack of
each call named its caller: `update_from_world`'s skinned path. Every
frame, each skinned sub-mesh got a transform buffer and bind group, a
material buffer and bind group, a bone bind group, and copies of its
vertex and index buffers, so Float's hands, body and birds made hundreds
of WebGPU objects a frame. On the desktop that costs CPU; in Firefox each
is also a DOM object for the GC and the cycle collector. Skinned draws now
go through `draw_bind_cache` like static ones (`SKINNED_DRAW_KEY` keeps
their keys apart), the bone bind group is cached per draw in
`skinned_bone_binds` and rebuilt when the entity's own bone buffer
arrives, and the mesh's buffers are shared through `Arc`.

**Not done.** Bloom still submits twelve times a frame, one per step,
because its steps share one uniform buffer (`PostProcessPipeline::run_bloom`);
it makes 24 of the 36 bind groups left. Firefox no longer shows the cost,
but it's the next to go. Safari wasn't run.

## Full screen (#135)

On a phone the address bar and toolbars take a large slice of the screen,
and on a desktop the page can't reach the edges without F11. The page now
asks the browser for the whole screen, and where the browser can't give it
(iPhone Safari), it says how to get it from the home screen.

- **A switch under Begin.** "full screen" goes in with the same click that
  starts the sound and locks the pointer. It starts on where the pointer is
  coarse (a phone or tablet) and off everywhere else, so a desktop player
  who just clicks Begin sees no change. The choice is kept in
  `localStorage` (`flint.fullscreen`). Full screen takes the document
  element, so the gate, the hints and the perf readout come along.
- **A corner button**, bottom right (`#fullscreen`), enters and leaves at
  any time and follows `fullscreenchange`. It hides while the pointer is
  locked, since a locked pointer can't click it. If Escape has freed a
  pointer the game wants (the "click to look around" hint is showing), the
  same click takes the lock back first.
- **On a touch screen**, once full screen is granted, the page asks
  `screen.orientation.lock("landscape")`. Chrome on Android honours it in
  full screen; elsewhere the refusal is ignored.
- **Where there is no Fullscreen API.** `document.fullscreenEnabled` is
  false on iPhone Safari, and in an iframe without `allowfullscreen`. In
  both cases the button and the switch stay hidden. On iPhone Safari
  (`"standalone" in navigator`), the gate says: tap Share, then Add to Home
  Screen. The page carries `apple-mobile-web-app-capable`, a black
  translucent status bar, `icons/flint-180.png` as its touch icon, and
  `manifest.webmanifest` (`display: fullscreen`, no `start_url`, so the icon
  opens the `?game=` it was added from). Opened from the home screen, the
  page has no address bar, and the hint and switch stay hidden.
- **Embed.** `?embed=1` keeps its look and shows the switch nowhere. The
  corner button appears only when the iframe may go full screen: a
  cross-origin frame needs `allow="fullscreen"` or `allowfullscreen`, which
  the snippet above has. (A same-origin frame may always: the policy's
  default is `self`.)
- **Canvas height** is `100dvh` (after a `100vh` fallback), the height
  actually visible on a phone as its bar comes and goes.

**Order matters in Chrome.** `requestFullscreen` uses up the click's
transient activation, and a pointer lock requested after it is refused
("A user gesture is required"). A lock taken first survives entering full
screen. The Begin button's own handler (`flint_web.rs`) asks for the lock,
and the page's full-screen handler listens on `document`, so it runs after
the button's handler in the bubble phase. Leaving full screen drops the
lock in Chrome; `sync_pointer_lock` then releases the cursor just as it
does for Escape, and the game's click-to-look takes it back. Nothing in the
player changed.

**Checked** (the page built with `trunk build --release`, served by
`python -m http.server`, driven over CDP):

| Visit | Browser | Result |
|---|---|---|
| a lock-then-fullscreen spike page | headed Chrome 156, Windows | lock first: both granted; fullscreen first: fullscreen granted, the lock refused; the lock requested on `fullscreenchange`: refused. `exitFullscreen` dropped the lock |
| `/`, Begin with the switch off | headed Chrome | the pointer locked, no full screen, canvas 2079x1125 at DPR 1.5 (as before) |
| `/?perf=1`, Begin with the switch on | headed Chrome | full screen and the lock together; the canvas went to 3840x2076 (2560x1384 CSS px at DPR 1.5), the scene too at scale 1, p99 16.9 ms after the resize |
| then `exitPointerLock`, the corner button twice | headed Chrome | the button showed with the hint; one click left full screen (2079x1125 again), the next entered it and took the lock back in the same click |
| `?embed=1` in two cross-origin iframes | headed Chrome | the button only in the frame with `allow="fullscreen" allowfullscreen`; no switch in either |
| `/`, emulated phone (412x915, DPR 2.625, touch) | headed Chrome | the switch on by default; a tap on Begin entered full screen; orientation reported `landscape-primary` |
| `/`, the same with Element fullscreen and `document.fullscreenEnabled` removed, and `navigator.standalone = false` | headed Chrome | no button and no switch; the Add to Home Screen line under Begin |

**Not done here.** Firefox on Windows refused both the lock and full screen
because its window wasn't focused while the desktop was in use ("The
document is not focused"), so Firefox's ordering is unconfirmed. No real
phone was tried: Chrome on Android, iPhone Safari and the installed
home-screen page are still to be checked on devices. The emulated phone
also takes the pointer lock on Begin; skipping the lock on touch is #132's.
With `?perf=1` on a narrow phone, the readout covers Begin.

## Async startup (#99)

A desktop makes the window and then blocks on `RenderContext::new`. A
browser can't block its main thread, so on wasm32:

1. `flint_web.rs` fetches and parses the scene, builds `PlayerApp`, gives it
   an `EventLoopProxy` (`set_event_proxy`) and starts winit with `spawn_app`,
   which returns at once.
2. `resumed` → `initialize` makes the window on the page's `<canvas id="flint">`
   (`web_startup::canvas_attributes`; a page without one gets a canvas
   appended) and starts `RenderContext::new` with `spawn_local`.
3. The future parks the context in a shared slot and wakes the loop with a
   user event, or a redraw if no proxy was given.
4. `user_event` → `take_ready_render_context` runs `finish_initialize`, the
   same code a desktop runs after `block_on`, then sizes the surface to the
   canvas as it is now.

Until step 4 the player is *not ready*: `render_context` is `None`, and
`window_event` drops every event, so nothing ticks, draws or reads input
against a missing device. `resumed` starts only when there is no window yet,
so a second `resumed` while the device is in flight doesn't start twice.
Desktop and Android keep `pollster::block_on` and `run_app`.

The canvas's CSS size decides the surface size: winit's `ResizeObserver`
sends `Resized` with the CSS size times `devicePixelRatio`, and the player
reconfigures the surface. Measured in Chrome at DPR 1.5: a 1266x650 page
gave a 1899x975 surface, 886x470 gave 1329x705, 1386x710 gave 2079x1065,
with no WebGPU errors.

## WGSL that only the browser rejects

naga, which compiles WGSL on the desktop, does not check WGSL's uniformity
rules. Chrome's Tint does, and an invalid pipeline invalidates the whole
frame's command buffer, so one bad shader leaves the canvas black. Fixed in
#99:

- `ocean_shader.wgsl`: `fwidth` inside the rain-ring cell loop. The
  derivative is taken once before the branch.
- `ssao_shader.wgsl`: `textureSample` inside the kernel loop. The pass reads
  unmipped targets, so it uses `textureSampleLevel(.., 0.0)`.
- `grass_render.wgsl`: `textureSampleCompare` inside the shadow-bounds
  branch, now `textureSampleCompareLevel`.
- WebGPU forbids depth bias on line topologies. The overlay, skeleton and
  skinned-wireframe line pipelines draw unbiased on the web
  (`pipeline::line_depth_bias`).

A WebGPU canvas offers only `Bgra8Unorm`/`Rgba8Unorm`, never an sRGB
format, so until #102 the web frame came out darker and more saturated than
the desktop's. Now `RenderContext` configures the plain format with its
sRGB twin as a view format, and the scene's last pass draws through that
view (`RenderContext::view_format`, `frame_view`), so the encode happens on
write as it does on the desktop. egui's HUD pass still draws through the
plain view (`hud_format`). Float's sunrise in Chrome now matches the
desktop's to a mean of 1.1/255 ([First light: Float](#first-light-float-102)).

## What failed to compile, crate by crate

Five errors in all, against the seven crates the issue was worried about:

| Crate | Result on wasm32 | Fix in this branch |
|---|---|---|
| `getrandom` 0.3 (via ahash: egui, hecs, rhai) | refuses to build: "wasm32-unknown-unknown targets are not supported by default" | `wasm_js` feature on a wasm-only dep in `flint-player`, plus `--cfg getrandom_backend="wasm_js"` in `.cargo/config.toml` |
| `egui-winit` → `arboard` | no web clipboard backend (8 errors) | `egui-winit` with `default-features = false` on wasm only. This also drops `links`/`webbrowser` |
| `kira` / `cpal` | builds. `StaticSoundData::from_file` is not there on wasm (flint-audio once, flint-music twice) | `static_sound_from_file` helper: `from_file` on native, `std::fs::read` + `from_cursor` on web. Since #97 both targets read through the vfs and decode with `from_cursor`. (The spike thought cpal had the null host here. It didn't: kira turns on cpal's `wasm-bindgen` feature for wasm32 itself. See [Sound](#sound).) |
| `rhai` | builds. `module_resolvers::FileModuleResolver` is not there | `ScriptEngine::set_module_base` skipped the resolver on wasm. Since #97 it installs `VfsModuleResolver` (flint-script) on every target, so `import` resolves through the vfs |
| `wgpu` 23 | builds (WebGPU backend by default, no naga or wgpu-core). `Instance::enumerate_adapters` is not there | `gpu_select` treats the adapter list as empty on wasm. `--gpu` / `WGPU_ADAPTER_NAME` fall back to `request_adapter` |
| `rapier3d` / `parry3d` | builds clean | none |
| `gilrs` (xinput-only features, ADR 0011) | builds clean. gilrs-core picks its web Gamepad backend by target | none needed to compile. See "Shims landed (#100)" for features |
| `gltf`, `image`, `toml`, `hecs`, `winit` 0.30 | build clean | none |

`cargo tree` lists 390 distinct crates in the wasm graph.

## Compiles, but will fail at run time

These are what M1 and M2 owe (`#97` vfs, `#98` async startup, the shim
tickets). Counts are for the runtime crates the player links:

- **`std::fs`** (done in #97: every runtime read now goes through `flint_core::vfs`; the web player installs a `MemorySource` filled from its bundle, which #98 fetches) returns `Unsupported` on wasm32-unknown-unknown. LLVM can
  *prove* that, so it deletes everything behind a file read. The real
  `flint-player` bin comes out at 2.4 MB with 4 imports because `load_scene`
  always fails and the whole engine after it is dead code. Any size number
  taken from the stock bin is wrong until the vfs lands.
- **`pollster::block_on`**: done in #99, see Async startup above.
- **Audio:** done in #101, see [Sound](#sound).

### Shims landed (#100)

- **Time.** `std::time::Instant::now` and `SystemTime::now` panic ("time not
  implemented on this platform"). Every runtime use now goes through
  `web-time` (already in the graph through egui and rhai): `GameClock`,
  input timestamps, the save debounce, frame pacing, the music clock bridge
  and session, procgen's cache, queue and random seed, input capture, and
  rhai's `random`. On native `web_time` re-exports std's types, so the
  signatures callers see are the same. The unused `instant` workspace dep is
  gone. Tests and native-only tools (flint-cli, the viewer) keep std.
- **Gamepads.** gilrs-core picks its backend by target: XInput on Windows
  through the `xinput` feature (ADR 0011, unchanged), the browser Gamepad API
  on wasm32. The feature has to stay on for every target, because
  gilrs-core 0.6.7's "needs xinput or wgi" `compile_error!` isn't gated to
  Windows. It does nothing elsewhere, since rusty-xinput is a Windows-only
  dep. The direct `gilrs-core` dependency (the rumble motor path, ADR 0025)
  is native only.
- **Threads.** `flint-input-capture`'s 1 kHz capture loop, `measure_granularity`
  and the rumble spike compile out on wasm. `spawn_with_rumble` returns `Err`
  there, which `attach_session_input` already treats as "play without
  input". A music session on the web has no high-rate capture yet. The
  player's ordinary once-a-frame gilrs poll still drives `InputState`.
- **Settings.** `flint_player::user_files` holds the files the player writes
  for the person playing: ordinary files under `dirs::config_dir()` on the
  desktop, and `localStorage` keys on the web (`flint:/config/flint/input_<game_id>.toml`).
  The input override's resolve, read and write go through it, so a rebind
  survives a reload. `dirs` is a native-only dependency now.
- **Desktop-only tools.** `notify`, `rfd` and `ureq` aren't in the wasm
  graph at all: `cargo tree --target wasm32-unknown-unknown -p flint-player
  -i <crate>` finds none of them. Only flint-cli and the viewer use them.
  The remaining `std::process::id` calls in player crates are all in tests.

The shim page (`shims.html`, bin `flint-web-shims`) exercises all of these: the day runs on
`web_time::Instant`, the left stick scrubs it, `pause_day` (Space or the
pad's South button) holds it through `InputState`, and R followed by a key
or pad button rebinds `pause_day` and saves the rebind through `user_files`.

Over its sky sits the Begin gate (#101). Float's sea and one of its songs
are fetched from its GitHub repo and decoded, and the click opens the sound
(query parameters are listed at the top of `flint_web_shims.rs`: `?music=`,
`?buffer=`, `?audio=`, `?begin=auto`, `?stall=`). The corner counts late
buffers, and the console prints a steadiness line every 10 s.

## Sound

#101. A browser gives no sound before a user gesture: an `AudioContext`
made earlier is kept suspended, and Chrome warns about it. So on wasm32
`AudioEngine::new()` stays silent, and the shim page's **Begin** gate opens the
stream from inside the click handler:

- `AudioEngine::silent()` / `start()`. On the desktop `new()` is `silent()`
  then `start()`, as before. In the browser, `start()` is called from the
  gesture. A silent engine still loads and caches sounds, so decoding
  happens behind the gate. The filter cutoff and bus volumes set while
  silent are applied when it starts. If the device fails, the engine stays
  silent and `start()` can be called again (the `Option` gotcha still
  holds).
- `AudioSystem::start(world)` picks up what `initialize` skipped while
  silent: the listener, the trigger rules and every `audio_source` already
  in the world. Looping sources therefore begin from the click.
  `PlayerApp::start_audio()` also starts a scene's `music_session` that was
  skipped for want of a device. The bundle page's Begin gate calls it
  (#102, `flint_player::web_begin`).
- `flint_audio::web::WebAudioBackend` replaces kira's `CpalBackend` on
  wasm32 (`engine::AudioBackend` picks the backend by target). It is the same
  cpal WebAudio stream, but it keeps cpal's timestamps. Each buffer's
  **slack** is how far ahead of the audio clock it was scheduled.
  `flint_audio::meter::BufferMeter` logs every late buffer (negative slack,
  heard as a gap), plus a summary every 10 s: buffers, late, slack min/mean,
  mix time and the longest wait between requests. It also logs the
  `AudioContext`'s state changes and device errors. `AudioEngine::buffer_stats()`
  returns the totals, which the page shows in its corner.

cpal's WebAudio host renders on the **main thread**: each buffer's
`onended` mixes the next one, with two in flight. So a frame that holds the
thread longer than about one buffer (2048 frames, 46 ms at 44.1 kHz) makes
sound drop. `?buffer=4096` doubles that headroom, at the cost of latency.

Measured in headless Chrome 156 on this laptop, with Float's `ocean_loop.ogg`
and `music_day.ogg` fetched from its repo. The `?stall=N` knob holds the main
thread for N ms every 2 s:

| Run | Buffer | Buffers | Late | Worst slack |
|---|---|---|---|---|
| no stall, **10 min** (`music_dusk`) | 46 ms | 12962 | 0 | +19.2 ms |
| no stall, 70 s | 46 ms | 1514 | 0 | +36.6 ms |
| 30 ms stall, 150 s | 46 ms | 3241 | 0 | +19.2 ms |
| 60 ms stall | 46 ms | 646 | 5 | −21.4 ms |
| 100 ms stall | 46 ms | 1080 | 45 | −62.1 ms |
| 100 ms stall | 93 ms | 326 | 1 | −4.0 ms |
| 200 ms stall | 93 ms | 324 | 19 | −102.7 ms |

Counts are as of each run's last 10 s summary. At 46 ms buffers, a 100 ms
stall loses about one buffer per stall. A stall is covered while it ends
before the playing buffer does, so the edge sits near one buffer length,
where the model puts it.

Mixing Float's sea and one song costs about 0.27 ms a buffer, so the mix
isn't the hazard. Heavy frames are. Decoding happens before Begin:
`ocean_loop.ogg` (156 KB) took 28 ms, `music_day.ogg` (1.26 MB) 241 ms, and
`music_dusk.ogg` (1.43 MB) 100 ms in another run.
A song decoded in one go mid-game stalls that long. Since #124 it doesn't
happen that way: sounds decode a packet at a time, 2 ms a frame (see "Audio
comes late" above). If heavy frames drop sound
once Float's ocean renders, follow Soba's threaded AudioWorklet build. It
needs cross-origin isolation headers.

**Firefox 147**, headless, with autoplay allowed by profile prefs: 865
buffers in 40 s, 0 late, worst slack +25.0 ms. Firefox's headless mode
offers no WebGPU, so the sky failed to create its surface while the sound
played on. Sound doesn't depend on the canvas.

What the headless runs can't show: whether it *sounds* clean on speakers,
and Safari (no way to run it from Windows). Those need a person at a browser
(the issue's ten-minute listen).

## Size

Release profile, wasm-bindgen 0.2.108, binaryen `wasm-opt` version 123 `-Oz`,
gzip -9:

| Build | wasm-bindgen output | after `wasm-opt -Oz` | gzip -9 |
|---|---|---|---|
| `flint-web` spike (wgpu + winit only) | 792 KB | 408 KB | 164 KB |
| `flint-web-shims`, the #100 shim check (+ `InputState`, toml, gilrs) | | 857 KB | 329 KB |
| Whole engine (size probe, below) | 14.9 MB | 10.7 MB | 4.1 MB |
| `flint-web` running PlayerApp (#99, measured before the vfs merge; file-reading code was still dead) | | 9.4 MB | 3.5 MB |
| `flint-web` reading game bundles (#98; every engine read live) | | 14.2 MB | 5.1 MB |

The **size probe** is a throwaway bin, not committed. It builds a
`PlayerApp` from a `black_box` world and passes opaque bytes to
`rhai::Engine::eval`, `image::load_from_memory`, `toml::from_str` and
`AudioEngine::load_sound_from_bytes`, then runs the event loop. That keeps the
code the vfs will open alive. The glTF mesh conversion still sits behind
`gltf::import(path)`, so the probe somewhat *undercounts*.

Code bytes by crate (9.5 MB code section, from the name section; generic std
glue is credited to the first crate in its type arguments):

| Crate | MB | | Crate | MB |
|---|---|---|---|---|
| rhai | 2.04 | | zune_jpeg | 0.21 |
| flint_script | 0.64 | | epaint | 0.18 |
| egui | 0.55 | | toml | 0.17 |
| hashbrown | 0.45 | | flint_player | 0.17 |
| flint_procgen | 0.45 | | winit | 0.17 |
| parry3d | 0.43 | | exr | 0.15 |
| flint_render | 0.35 | | image | 0.15 |
| rapier3d | 0.29 | | image_webp + tiff | 0.23 |

### The trims (#123)

Each row adds one lever to the row above it. `flint-web` as
play.yml builds it (trunk 0.21.14, its `wasm-opt -Oz`), gzip -9 of the
`.wasm`:

| Build | `.wasm` | gzip -9 | Saved (gzip) |
|---|---|---|---|
| `--release` (the site before #123) | 14.23 MB | 5.09 MB | |
| `web-release`: `opt-level = "s"`, fat LTO, 1 codegen unit, abort | 9.76 MB | 3.83 MB | 1.26 MB |
| `web-release` with `opt-level = "z"` instead | 8.25 MB | 3.43 MB | 1.66 MB |
| + `image` with PNG, JPEG and HDR only | 7.45 MB | 3.15 MB | 0.28 MB |
| + rhai `no_optimize` | 7.40 MB | 3.14 MB | 0.02 MB |
| + rhai `no_custom_syntax` | 7.39 MB | 3.13 MB | 0.00 MB |
| + rhai `only_i64` (**what the site ships**) | 7.28 MB | 3.11 MB | 0.02 MB |

**3.11 MB gzipped, 39% under the 5.09 MB the site served.** The profile does
most of it: fat LTO and one codegen unit let the linker drop what the
per-crate builds kept. Where it lives:

- **`[profile.web-release]`** in the workspace `Cargo.toml`. play.yml builds
  with `trunk build --release --cargo-profile web-release`. Desktop
  `--release` is untouched. The build is slower: about 5 minutes from cold
  on a desktop, against under 3.
- **`panic = "abort"` still shows the panic.** `console_error_panic_hook`
  runs before the abort. A forged bundle that panics in `Bundle::slice` logs
  `panicked at crates\flint-core\src\bundle.rs:292:20: range start index …`,
  then `RuntimeError: unreachable`.
- **`image`** is `default-features = false` with `png`, `jpeg` and `hdr` in
  the workspace (HDR because skybox panoramas can be `.hdr`).
  `flint-render` adds image's `default` back under
  `cfg(not(target_arch = "wasm32"))`, so every desktop binary decodes what it
  did before. The web drops exr, tiff, webp, avif (ravif/rav1e), gif, qoi and
  the rest. JPEG stays because glTF textures can be JPEG.
- **rhai's three features** are a wasm-only dependency of `flint-script`.
  None of them changes what a Flint script means. Flint registers no custom
  syntax, never sets an optimization level, and passes scripts `i64` and
  `f64` only. flint-script's 111 tests pass with all three on, and Float
  played a whole day with them. Together they save 0.15 MB of `.wasm` but
  only 0.04 MB gzipped, so drop one if a game ever needs it.

**What `z` costs at run time.** In headless Chrome, Float played a 14-minute
day on the shipped build at 16.67 ms a frame every minute (p99 16.8 ms), with
no console errors, the same as `--release`. The one CPU cost that shows is
kira's mix: 3.67 ms per 46 ms buffer on `--release`, 3.94 to 4.01 ms on
`web-release`, with no late buffers in any run. The first `web-release` day
fell to 37 to 65 ms a frame from minute 9 on. Two later runs (a full day, and
one to minute 10 with a CPU profile) held 60 fps throughout. The profile
showed no Rust function above 2.5% of the main thread, and that day's audio
was never late either. So it looks like load from elsewhere on the machine,
but #120's perf overlay is the place to watch for it. If decoding or mixing
ever needs the speed back, `[profile.web-release.package.<crate>]
opt-level = 3` raises one crate without giving back the rest.

Not trimmed:

- **flint_procgen is reachable on the web.** Scene loading resolves
  `procgen` components through `ProcGenResolver`
  (`player_app/scene_loading.rs`) into meshes and textures, so it stays.
  Float uses none, so a `procgen` cargo feature could drop it for games like
  Float. That needs a measurement first.
- **exr** was reachable only through image's default formats. It's gone now.

3.1 MB gzipped is in the range of other WebGPU game demos. Float's 10 MB of
data, most of it music, now costs three times what the engine does.
