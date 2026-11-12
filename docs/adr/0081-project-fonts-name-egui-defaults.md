# ADR 0081: Project fonts name egui's defaults; extensions get a fonts hook

Status: accepted, 2026-10-17 ([#49](https://github.com/chrischaps/Flint/pull/49)).
Backfilled 2026-11-12 (#374) from the ADR text in PR #49's description and
commit `927f590`.

## Context

The player owns egui's fonts. It installs `<project>/fonts/` at start and on
a project-root change, and each install replaced all of egui's definitions.
Projects could name families for the script HUD but couldn't choose egui's
default faces, so a game's own egui (`PlayerExtension::ui`) drew in
Ubuntu-Light. A game binary's own `set_fonts` was undone by the next
install. The Capitol embedded its fonts a second time (about 1 MB), checked
every frame whether they were still in egui, reinstalled them if not, and
skipped its UI for a frame each time.

## Decision

1. `fonts.toml` may hold `[egui] proportional = "<family>"` and
   `monospace = "<family>"`. The named face is put first in egui's
   `Proportional`/`Monospace` list, with egui's defaults kept after it. An
   unknown name warns and leaves egui's family alone.
2. Every project-named family is `[its face] + egui's default Proportional
   list`, so missing glyphs fall back rather than draw as tofu.
3. `PlayerExtension::fonts(&mut FontDefinitions)` is called, in add order,
   on the definitions just before each install, including a reinstall
   triggered by a late `add_extension`. The families scripts may name are
   every `FontFamily::Name` in the result.
4. `project_font_definitions(scene_path)` is exported for headless tests.

## Alternatives considered

- Merging into egui's current definitions instead of replacing them.
  Rejected: a scene change into a font-less project would keep stale
  families, and there would be no defined order between the game's fonts
  and the project's.

## Consequences

- Data-only projects get themed egui with one TOML line, and game binaries
  never call `set_fonts`.
- Families exist from the first frame, since the install happens before the
  first egui pass.
- Nothing changes for existing projects except the glyph fallback (better
  coverage, same metrics for covered glyphs) and one `set_fonts` call at
  start for a project with no `fonts/`.

## Sources

- [#49](https://github.com/chrischaps/Flint/pull/49) / commit
  [`927f590`](https://github.com/chrischaps/Flint/commit/927f590bf97d7e11725b7e4e5366f56331ed7f54)
  "Project fonts can name egui's default faces; extensions get a fonts hook
  (ADR 0081)"
- Vault: "The Capitol - egui Default Fonts from Flint (H15) - 2026-10-17"
- Code now: `crates/flint-player/src/player_app/fonts.rs`,
  `crates/flint-player/src/extension.rs` (`PlayerExtension::fonts`); book:
  [Player Extensions](../book/src/guides/player-extensions.md), "Fonts",
  and [Scripting](../book/src/concepts/scripting.md) (project fonts)
