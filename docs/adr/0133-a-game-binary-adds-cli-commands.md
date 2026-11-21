# ADR 0133: A game binary adds its own `flint` commands through `CliExtension`

Status: accepted, 2026-11-21 (#1140). Follows #709; beside
[ADR 0071](0071-player-extensions-in-a-game-binary.md).

## Context

`flint` was a binary with one fixed `Commands` enum. A game had no way to
add a command of its own. Its tools either went into the engine, where every
game's `flint` carries them, or into a separate binary with its own flags,
its own `--json` conventions and none of the engine's commands. #709 moved
the rhythm project's six harness commands under `flint music`. That made the
top level tidier, but one game's harness is still compiled into every game's
CLI. #709 suggested the longer-term fix: a game binary that embeds the CLI
and registers its own subcommands, the way `PlayerApp::add_extension` lets
it add hooks to the player.

Three constraints shaped the answer:

- **One parser.** The global `--json` and `--project` (#710, ADR 0114), the
  `--json` error document for a bad command line, the refusal of windows
  under `--json` and `output::finish`'s exit path must hold for a game's
  command as they do for the engine's. Otherwise a game's commands would be
  the one part of `flint` an agent can't read as JSON.
- **The debug parser's stack** (#708, #990). A derived `augment_args` costs
  about 8 KB of stack a flag in a debug build, and the parser builds every
  command at once. A game's commands add to that.
- **No game-specific types in the engine.** The extension must work from a
  game crate that depends on the engine by path, through its subtree.

## Decision

1. **flint-cli is a library as well as the binary.** `src/lib.rs` holds
   what `main.rs` held. `flint_cli::run_with(Vec<Box<dyn CliExtension>>) ->
   ExitCode` is the whole command line, from the process's arguments to its
   exit code. The `flint` binary is `run_with(Vec::new())`. Everything else
   in the crate stays private: the public surface is `run_with` and the
   `extension` module.
2. **`CliExtension`** (`flint_cli::extension`) has three methods:
   - `commands() -> Vec<clap::Command>`: the subcommands it adds, each with
     its own name, flags and help. Asked once a run, before parsing.
   - `run(&ArgMatches) -> anyhow::Result<()>`: run the chosen one. The
     matches are the whole line's, so `matches.subcommand()` is the
     command and its arguments, the global flags are there too, and a
     derived `Subcommand` reads them directly.
   - `opens_window(&ArgMatches) -> bool`, no by default. `--json` refuses a
     command that says yes, with the engine's wording.
3. **`extension::subcommands(|cmd: GameCommands| ...)`** makes an extension
   from a derived `clap::Subcommand` enum, with `.opens_window_when(|cmd| ..)`.
   That is the shape a game will usually want. The trait is there for
   anything else.
4. **One parser, the engine's.** `run_with` builds `Cli::command()`, adds
   each extension's commands with `Command::subcommands`, parses once, and
   then either hands the matches to the extension that owns the chosen name
   or builds `Cli` from them (`Cli::from_arg_matches`), as `try_parse_from`
   did. Before an extension runs, `--project` is applied, `--json` is set and
   the log is the one every windowless command writes. It leaves through
   `output::finish`. An extension answers with the engine's own `emit`,
   `Answer`, `Report`, `document`, `failed` and `json`, re-exported from
   `flint_cli::extension`, and writes human lines with `extension::say`.
5. **Names are checked, not overridden.** An extension's command or alias
   that is the engine's (a subcommand, hidden ones included, an alias,
   `help`) is refused before anything runs, and so is a name two
   extensions share. (The old music names were reserved too until #1141
   removed them from the engine.) The message
   names the command and whose it is. A player extension's function may
   replace the engine's (ADR 0071). A command that silently stopped being
   the engine's would break every script and doc that names it, and clap
   would panic on the duplicate anyway.
6. **`flint music` stays in the engine for now.** Moving it out is now
   possible: the rhythm project's game binary would add the six commands as
   an extension, and flint-cli would drop flint-music, flint-rhythm and
   flint-input-capture. Whether to do that is Chris's call. It takes the
   harness away from anyone using the engine's `flint` without that game,
   and the book's music reference with it. It is not decided here.

## Alternatives considered

- **External subcommands, as cargo does** (`flint foo` runs `flint-foo` on
  the PATH). There's no type sharing and no shared parser: `--json`,
  `--project`, the help listing and the error document would each have to
  be copied by convention. It also needs installation and PATH handling,
  which agents in fresh worktrees don't have.
- **A `#[command(flatten)]` of a game enum into `Commands`.** That needs a
  generic `Cli<G>` and puts the game's type into every engine command's
  path, for what is only a parse-time join.
- **Letting an extension replace an engine command.** That's the
  `PlayerExtension` rule for script functions, but a replaced command is
  invisible in `flint --help`, and nothing in the engine's tests would
  notice. A game that wants a different `render` names it `render-game`.

## Consequences

- A game's `flint` is its own binary: `[[bin]] name = "flint"` in the game
  crate, `fn main() { flint_cli::run_with(vec![...]) }`. Since `capture`
  and `goldens` spawn `current_exe()`, they spawn the game's binary with its
  commands too.
- The game's binary links its own 8 MB main-thread stack on Windows in its
  `build.rs`, as a player binary does (#990). flint-cli's `build.rs`
  reaches only flint-cli's own targets. The parser test
  (`stack_tests::the_parser_fits_in_windows_default_stack`) measures the
  engine's commands. A game's flags add about 8 KB each in debug, so a
  game's `Args` structs follow the same dozen-flag rule.
- Unit tests under `src/` are the crate's `--lib` tests now:
  `cargo test -p flint-cli --lib stack_use -- --ignored --nocapture`.
- A game's generators (ADR 0119) still reach only the player. The engine's
  `flint gen` in a game's binary resolves built-ins only. A later hook
  could hand `CliExtension` the generator registry, if a game asks for it.
- Test: `crates/flint-cli/tests/cli_extension.rs` is a game binary
  (`harness = false`, so its `main` is its own). It spawns itself as a game's
  `flint` with a `hello` and a `peek` command and checks the help listing,
  text and `--json` answers, `--project`, a failed answer, a clap mistake's
  error document, a window refused under `--json`, an engine command still
  running in the game's binary, and a clash with `render` refused. The
  unit tests in `extension.rs` cover the name checks and dispatch.

## Sources

- #1140, #709 (the music move and its note on the direction), #710 (the
  one exit path), #708 and #990 (the parser's stack).
- Code: `crates/flint-cli/src/lib.rs` (`run_with`),
  `crates/flint-cli/src/extension.rs`. Book:
  [Player Extensions, "Commands of the Game's Own"](../book/src/guides/player-extensions.md#commands-of-the-games-own).
