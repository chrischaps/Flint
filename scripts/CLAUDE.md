# scripts: what a PR must pass, the budgets, build output (moved out of the root CLAUDE.md by #992)

The root file keeps a short form of each section. The claim tool (`issue.sh`) is the root's "Claiming issues"; the book's checks (`docs-check.py`) are `docs/CLAUDE.md`.

## The root CLAUDE.md's budget (#992)

`scripts/claude-md-check.py` (in `scripts/ci.sh`; `--report` prints each section's size) fails when the root `CLAUDE.md` is over `BUDGET`, 15,000 bytes as git stores it (LF), or when a tracked `CLAUDE.md` is missing from its "Where the detail lives" table, or the table names one that isn't there.

- **Why a budget:** every session reads the whole root file, whatever it is about; a scoped file is read only by a session working under its directory. #755 cut the root from 96 KB to 18 KB by moving feature notes out; with nothing holding the total, it was 35 KB two weeks later, one bullet a ticket, each right for its ticket.
- **Why 15,000:** what every session needs, measured at #992: the claims section (vendored from setup-claims, kept byte for byte) 3.5 KB, the crate map 2.8 KB (`docs-check.py` holds it to name every crate), the index 1.8 KB and the overview 0.4 KB make 8.5 KB (8,502 bytes), and the build commands, the edit -> validate -> play loop and the cross-cutting rules and gotchas fit in the rest at one line each. #992 left it at 14.8 KB.
- **Over it:** move the detail, verbatim, into the scoped file for the code it describes, and leave the root a line (or nothing, if it named one crate). A topic with no scoped file gets one in the directory it describes, and a row in the table. Raising `BUDGET` is a decision to argue in its own PR, not a line in a feature's.

## `scripts/ci.sh`, CI and the build commands

The root's build commands, with every note they carried:

```bash
cargo build                    # Debug build
cargo build --release          # Release build
cargo build -p flint-cli --no-default-features  # Headless CLI: no sound card, gamepads or file dialogs,
                               #   so no ALSA/libudev headers on Linux (#221; features audio/gamepad/dialogs;
                               #   `bash scripts/headless-check.sh [--build]` keeps device crates out of its tree),
                               #   and no package either (#1333: living/music/rhythm/procgen/ai, or `packages`
                               #   for all five; CI's headless job checks the book with `--features packages`)
cargo run --bin flint -- <cmd> # Run CLI
cargo run --bin flint -- play flint-examples/phase4_runtime.scene.toml  # Play a scene
cargo run --bin flint-player -- flint-examples/phase4_runtime.scene.toml  # Standalone player (schemas: the root flint.toml)
cargo run --bin flint -- export web flint-examples/  # any game as a folder a static server plays (#106):
                               #   flint-examples/dist/web = player + games/<id>/ bundles + games.json;
                               #   --out, --player <trunk dist> (else trunk builds it), --meta k=v, --clean
cargo test                     # Run all tests
cargo test -p flint-core       # Test a single crate
cargo clippy                   # Lint
cargo fmt --all                # Format (rustfmt.toml pins the edition; `rustfmt <file>` for one file)
bash scripts/ci.sh             # What a PR must pass: formatting, a record for every cited ADR
                               #   (scripts/adr-check.sh), every clippy.toml made from the root's
                               #   (scripts/clippy-conf.sh --check), the tree's size budget
                               #   (scripts/size-check.py, ADR 0122) and the book's crate table
                               #   (flint-arch-analyzer --check-book: one small build, 23 s cold),
                               #   in seconds. `issue.sh land` runs it.
bash scripts/ci.sh --full      #   ...plus what CI's clippy and wasm jobs run (two builds)
```

- `flint-android` is the one crate excluded from `default-members` (a new crate goes in both lists, #758) -- use `cargo ndk -p flint-android`; min API 26. `cargo test --workspace` fails on `ndk-sys` on Windows; use plain `cargo test`

- CI (`.github/workflows/ci.yml`, #757) runs `rustfmt` (the tree is formatted; an unformatted file fails the PR and `issue.sh land`), `clippy (native)` (`cargo clippy --all-targets`: warnings pass, a deny-level lint or code that doesn't compile fails), the wasm and headless checks, and the GPU tests on WARP (`cargo test -p flint-render --test stylize`, the render goldens and, since #1268, `cargo test -p flint-cli --test json_cli`, the global `--json` contract, and since #1285 `--test schema_cli`, which needs no GPU but has flint built there), which fail the run like any other job since #1138 (they had `continue-on-error` from #757, and a kept-bind-group regression went unseen under it). No unit tests run in CI yet, and no other flint-cli integration test
- **The package matrix** (#1336, CI's `package matrix` job, `scripts/ci.sh --full`): `scripts/package-check.sh [--build] [config...]` reads `packages/*/package.toml` (a manifest whose `[build] features` the binary has is in its matrix, so a new package needs no edit there) and, for flint-player and flint-cli with `--no-default-features`, takes `none`, `standard` (the standard tier: procgen, debug-hud; the epic's "a game that lists none of the feature packages"), each package alone (with its `[requires]`) and `all`. Each: `cargo tree -e normal` must hold no crate of a package left out (fails naming who pulls it), then with `--build` `cargo check --all-targets`; it goes on past a failure, names each configuration (a `::error::` line on Actions) and ends with a table. The one exception, both ways (a stale one fails too): flint-cli links flint-procgen always and flint-procgen links flint-wooze, until #1516. Measured warm here: 6-52 s a configuration, 15 configurations. A planted `use flint_rhythm` outside its cfg failed `none` and `music` in both binaries and passed `rhythm`, `all` and the default build, which is all clippy's job builds
- CI's `wasm32 check` (`.github/workflows/ci.yml`) builds `flint-player` and every crate it links for `wasm32-unknown-unknown` on each PR. Run it locally with `bash scripts/wasm-check.sh` before pushing runtime-crate changes. It also denies `Instant::now`/`SystemTime::now` and their `elapsed()`, which compile on wasm but panic there: runtime code uses the `web-time` crate (a workspace dep: `web-time = { workspace = true }`), which #100 moved every older site to. Desktop-only deps in a runtime crate go under `[target.'cfg(not(target_arch = "wasm32"))'.dependencies]`, as in flint-player's Cargo.toml (docs/design/web.md)

## The clippy rules (root `clippy.toml`)

The vfs rule in full; the root keeps its short form, and `crates/flint-core/CLAUDE.md` says what the vfs is.

- **Every runtime read goes through `flint_core::vfs`** (#97) -- `vfs::read` / `read_to_string` / `read_dir` / `exists` / `is_file` / `is_dir`, never `std::fs` or `Path::exists` in a runtime crate. Decode images with `flint_render::open_image` and glTF through `flint_import::import_gltf`. Tests and the CLI keep `std::fs`. Plain `cargo clippy` fails on a read past the vfs and on `as_float` in every crate the player links (the root `clippy.toml`, #761; `disallowed_methods = "deny"` for every member since #979, each with `[lints] workspace = true`, which `scripts/ci.sh` holds): a write path or test that means it says why, `#[allow(clippy::disallowed_methods)] // <reason>`, a test file once at its top (`#![allow(..)]`), a crate's unit tests in one `#![cfg_attr(test, allow(..))]` in its lib.rs. Change a rule in the root `clippy.toml`, then run `bash scripts/clippy-conf.sh`: it writes the tool crates' own copies (`TOOL_CRATES`: the CLI, viewer, asset-gen, procgen-ai, ai-core and arch-analyzer hold only the wgpu seams) and `scripts/ci.sh` fails a stale one. The clock rules can't be native (there `web_time::Instant` is `std::time::Instant`): `scripts/wasm-clippy/wasm-only.toml`, added for the wasm check alone. flint-render's own rules (#1091: the bare `create_*` / `write_*` calls, which go through `gpu_counts`) are `scripts/clippy-render.toml`; the script writes `crates/flint-render/clippy.toml` as the root list plus those, and `--check` holds it too

## Build output across worktrees

Every agent worktree builds its own `target/`, and parallel worktrees are how this repo is worked, so disk is what runs out (#762):

- **Keep `target/` per worktree; never point several at one `CARGO_TARGET_DIR`.** Cargo names a workspace crate's output by its path inside the workspace, so two worktrees' `flint-render` share one output and a branch can build against the other's (2026-10-30: a branch failed on a method its own source defines). One target dir also has one build lock, so parallel agents queue, and tests that write into it collide (#421). An A/B against another revision gets its own dir.
- **The dev profile is lean** (`Cargo.toml`): line tables for workspace crates, no debug info for dependencies. A cold `cargo build` plus `cargo test --no-run` measures 6.6 GB and 150 s. Locals in a debugger: `CARGO_PROFILE_DEV_DEBUG=full`. What loads a texture is built at `opt-level = 3` even in debug (#326): the image decoders, glTF, SHA-256 and `flint-pixels`, where the texel loops live so `flint-render` stays unoptimized. A debug render of a scene with a 32 MB model takes about 2.6 s; a new per-texel loop on load goes in `flint-pixels`. So is egui's text stack (epaint, harfrust, skrifa, vello_cpu, #1129): unoptimized, the first F2 press cost 110 ms.
- **sccache is optional, per machine, not in `.cargo/config.toml`** (a wrapper that isn't installed fails every build). With `RUSTC_WRAPPER=sccache` a cold-cache build measured no slower (96 s); what a warm cache saves the next worktree is unmeasured (the run was stopped for low memory). It can cache only dependencies, since the workspace crates build incrementally.
- **Disk is guarded** by `scripts/build-space.sh`. A hook refuses `issue.sh claim` under 30 GB free (`FLINT_DISK_FLOOR_GB`), and the SessionStart hook deletes the `target/` of worktrees whose issue is closed (not the worktree, not one built in the last hour). `report` lists every worktree's `target/` by size. A release build adds several GB: before one, check `df -h .`, and delete your own worktree's `target/release` when you're done with it. So does `--profile profiling` (#1101, release plus line tables for a sampling profiler): `-p flint-player` cold measured 2.7 GB and 3 min 46 s in its own `target/profiling` (a 187 MB PDB), shared with nothing; delete it when done (book: guides/profiling.md, "When scopes aren't enough").

## The tree's size budget (ADR 0122) and its shape (#760)

- **Every byte here is in every game** (ADR 0122, #759): a subtree is the whole tree. `scripts/size-check.py` (in `scripts/ci.sh`) fails a PR on a binary tracked twice, a texture over 2048 texels a side or 3 MB (`tools/assets/cap_textures.py` shrinks one with the renderer's own mip filter; colour maps as JPEG), a file over 8 MB, a book PNG with an alpha channel (`tools/assets/pack_book_images.py` drops it, same pixels; #1276), or the tree or one of its parts over budget; `--report` prints the sizes
- The root holds only manifests and docs, and nothing is a symlink (a Windows checkout without the privilege gets a text file): `scripts/ci.sh` refuses either. A default `flint render` writes `render.png` to the working directory, which `.gitignore` covers, as it does coverage runs' `*.profraw`

## The changelog (#765, #788)

The changelog is for games that carry the engine as a subtree (#765): a change a game can notice (a scene or schema key, a script function, a CLI flag, a format, a Cargo feature, a change in look or behaviour) gets an entry saying what changed and what to do. Each PR adds its own file, `bash scripts/changelog.sh new <issue> <slug>` (`changelog.d/<issue>-<slug>.md`, about 150 words), never an edit to `CHANGELOG.md` or `changelog/` (#788): after the merge the `Changelog` workflow moves it into `changelog/YYYY-MM.md` and `CHANGELOG.md` stays a short page linking the months. `changelog.sh since <date>` prints what is new. CI's `changelog` job (`scripts/changelog-check.sh`) fails a PR that touches `schemas/`, the script API or the scene format with no fragment; a commit-message line `Changelog: none (reason)` waives it for a change no game can notice.
