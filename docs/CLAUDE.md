# docs: building and publishing the book (moved out of the root CLAUDE.md by #755)

The root file keeps the rule: a change that alters what the book describes updates the book in the same PR.

```bash
mdbook build docs/book                                   # must be clean before any docs commit; output in docs/book/book/ (gitignored)
bash docs/publish.sh --skip-rustdoc --serve              # preview at http://localhost:3000 (drop --skip-rustdoc for the API docs)
bash docs/publish.sh --publish                           # mdBook + rustdoc into ~/dev/docs.chaps.dev/flint, committed and pushed
cargo run -p flint-arch-analyzer -- --book               # rewrite the crate table in architecture/crate-graph.md (CI checks it)
cargo run --bin flint -- script api --book               # rewrite the script API reference and the chapters' script-api tables (CI checks it)
python scripts/docs-check.py                             # flags, styles, images and the crate/command maps the book, README and CLAUDE.md files name, against target/debug/flint (CI checks it)
```

**What a docs change runs**, and what a docs ticket should tell its agent (#375): update the book by hand, checking against the code; then `mdbook build docs/book`, `python scripts/docs-check.py`, and `flint script api --check-book` if a page names script functions. Nothing else exists: no `tools/docs-check/check.py`, `docs/DOCS_SYNC.md`, `.synced-to` or docs-sync workflow. A 2026-09 Vault note described that pipeline as built; it never landed (#175).

`scripts/docs-check.py` asks the binary, not the source: `flint <cmd> --help` for each command a line runs (a hidden one like `flint serve` answers its own `--help`), and `flint render --list-styles --json` for the styles. It reads command lines in code blocks (a block tagged `toml`, `rhai`, `text` and the like holds none: tag program output `text`) and in code spans, and in `cli-reference/` the flag tables under a command's heading ("The `render` Command", "`flint music chart play`") or one of its "... Flags" headings. A table under another heading is not read (the standalone player's flags under `play`). `--flint <path>` names the binary; `--json` for a list. What isn't drift (a user style a page makes up, a command a plan names) goes in `docs/book/docs-check-allow.txt`, by page.

Its `maps` check (#756) holds the maps an agent reads first. A crate count ("N-crate", "N crates under `crates/`") must be the number of directories under `crates/` with a `Cargo.toml`, and a member count ("N-member") the length of the root `Cargo.toml`'s `members`; a count spelled in words is refused, since nothing can hold it. Every crate must be named in the root CLAUDE.md's `## Architecture` section, in the book overview's `## Workspace Structure` tree, and as a `### <crate>` heading under its `## Crate Details`: a new crate needs all three, and the crate table in `crate-graph.md` from `flint-arch-analyzer --book`. A list that starts `Commands: ` (the root CLAUDE.md's `flint-cli` line, the overview's `flint-cli` section) may name hidden commands, must name every command `flint --help` shows, and nothing that isn't one: in a code block it runs over lines ending in a comma; where it uses code spans, only the spans count. The README is read too.

Its `packages` check (#1337) holds every engine package a page names to `flint package list --json --dir packages`: "the `living` package", "package `music`", `living = "*"`, the first code span of a `**Package:**` line, a key of a `[packages]` table and a `packages = [..]` array in a TOML block (other blocks and code spans are not read). Every page directly under `concepts/` must state its package on a `> **Package:** `<name>` ...` line under its title, and a new chapter there needs one. The init templates' Markdown (`crates/flint-cli/templates/**/*.md`) is read for this check only. A placeholder that stands for any package goes in the allow file by page. A name that isn't a package reads as drift, so write a Cargo package as a crate.

Without `--publish` the script builds and copies into the docs repo but doesn't commit. `publish.sh` is the only implementation (#764): `docs/build.ps1` (`-SkipRustdoc`, `-Publish`, `-Serve`, `-DocsRepo`) and `docs/publish.bat` (`publish.sh --publish`) hand their switches to it through `scripts/sh.cmd`, which finds Git for Windows' bash rather than WSL's. Change the script, not the shims.

The table between the `crate-table` markers in `architecture/crate-graph.md` is generated from every `Cargo.toml`; CI's `crate table` job fails when a dependency changes and the page doesn't. The ASCII diagram above it is still drawn by hand.

The script API is generated too (#695): `reference/script-api.md` is written whole by `flint script api --book`, its modules grouped by the package whose manifest provides them (#1337: `## The `<name>` package`, then `### <module>`, so a module's anchor is still its name), and so is every table between `<!-- script-api:begin <selector> -->` and `<!-- script-api:end -->` (the scripting chapter's function tables; selector `module:<name>`, function names or `callbacks`). Edit a function's doc in its declaration under `crates/flint-script/src/api/`, never in the page; what a one-line doc can't say goes in the notes or prose under the table. CI's headless job runs `--check-book`, which also fails on a ghost: a call in any chapter's code spans or Rhai blocks to a function the engine doesn't declare at that arity. A name that isn't a script call (a Rust method, a WGSL helper, a roadmap proposal) goes in `docs/book/script-api-allow.txt`, by page. Details: `crates/flint-cli/CLAUDE.md`.

- Publishing pushes to the separate `docs.chaps.dev` repo (default `~/dev/docs.chaps.dev`; `-DocsRepo` / `--docs-repo` to change it). It is outward-facing: only on request.
- Renders for the book are made locally with `flint render` (no GPU in CI) and land in `docs/book/src/images/` in kebab-case.

## From the root (#992)

The book is `docs/book` (mdBook), published to docs.chaps.dev by hand from this machine. CI checks its generated parts, the script API (`flint script api --check-book`) and the crate table (`flint-arch-analyzer --check-book`), and `python scripts/docs-check.py` (#375) checks what the prose names against a built `flint`: every `--flag` on a `flint` command line (book and CLAUDE.md files), every `[post_process.styles.<name>]`, every image a page links, and the maps (#756): a crate or member count (in digits), every crate in the root CLAUDE.md's Architecture tree and the book's overview, and a `Commands:` list against `flint --help`. Nothing checks what the prose says a flag or key does. When a change alters a command, flag, key or component the book describes, update the book in the same PR, checking against the code rather than memory. There is no `tools/docs-check`, `DOCS_SYNC.md` or `.synced-to` (#175, #375): a ticket that names them is wrong.

`tools/arch-analyzer` is a syn-based dependency/metrics extractor feeding the `tools/arch-viewer` web graph and the book's crate table, checked in CI.

- `docs/book/` -- mdBook source; `docs/build.ps1` / `docs/publish.sh` -- build, preview and publish it
- `docs/design/` -- Living design notes (the web port, the showcase film, Living Props targets, spike and cost measurements); `docs/archive/` -- superseded plans and the March 2026 design doc, each with a banner naming the book page that is current (#756). A plan whose work has shipped moves to the archive
- `docs/adr/` -- Architecture decision records, one file per number cited anywhere in the tree (0111 on written here; earlier ones rebuilt from commits and the Starchild repo, #374); `README.md` says how to take a number
