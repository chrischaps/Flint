# docs: building and publishing the book (moved out of the root CLAUDE.md by #755)

The root file keeps the rule: a change that alters what the book describes updates the book in the same PR.

```bash
mdbook build docs/book                                   # must be clean before any docs commit; output in docs/book/book/ (gitignored)
bash docs/publish.sh --skip-rustdoc --serve              # preview at http://localhost:3000 (drop --skip-rustdoc for the API docs)
bash docs/publish.sh --publish                           # mdBook + rustdoc into ~/dev/docs.chaps.dev/flint, committed and pushed
cargo run -p flint-arch-analyzer -- --book               # rewrite the crate table in architecture/crate-graph.md (CI checks it)
cargo run --bin flint -- script api --book               # rewrite the script API reference and the chapters' script-api tables (CI checks it)
python scripts/docs-check.py                             # flags, styles and images the book and CLAUDE.md files name, against target/debug/flint (CI checks it)
```

**What a docs change runs**, and what a docs ticket should tell its agent (#375): update the book by hand, checking against the code; then `mdbook build docs/book`, `python scripts/docs-check.py`, and `flint script api --check-book` if a page names script functions. Nothing else exists: no `tools/docs-check/check.py`, `docs/DOCS_SYNC.md`, `.synced-to` or docs-sync workflow. A 2026-09 Vault note described that pipeline as built; it never landed (#175).

`scripts/docs-check.py` asks the binary, not the source: `flint <cmd> --help` for each command a line runs (a hidden one like `flint serve` answers its own `--help`), and `flint render --list-styles --json` for the styles. It reads command lines in code blocks (a block tagged `toml`, `rhai`, `text` and the like holds none: tag program output `text`) and in code spans, and in `cli-reference/` the flag tables under a command's heading ("The `render` Command", "`flint play-chart`") or one of its "... Flags" headings. A table under another heading is not read (the standalone player's flags under `play`). `--flint <path>` names the binary; `--json` for a list. What isn't drift (a user style a page makes up, a command a plan names) goes in `docs/book/docs-check-allow.txt`, by page.

Without `--publish` the script builds and copies into the docs repo but doesn't commit. `publish.sh` is the only implementation (#764): `docs/build.ps1` (`-SkipRustdoc`, `-Publish`, `-Serve`, `-DocsRepo`) and `docs/publish.bat` (`publish.sh --publish`) hand their switches to it through `scripts/sh.cmd`, which finds Git for Windows' bash rather than WSL's. Change the script, not the shims.

The table between the `crate-table` markers in `architecture/crate-graph.md` is generated from every `Cargo.toml`; CI's `crate table` job fails when a dependency changes and the page doesn't. The ASCII diagram above it is still drawn by hand.

The script API is generated too (#695): `reference/script-api.md` is written whole by `flint script api --book`, and so is every table between `<!-- script-api:begin <selector> -->` and `<!-- script-api:end -->` (the scripting chapter's function tables; selector `module:<name>`, function names or `callbacks`). Edit a function's doc in its declaration under `crates/flint-script/src/api/`, never in the page; what a one-line doc can't say goes in the notes or prose under the table. CI's headless job runs `--check-book`, which also fails on a ghost: a call in any chapter's code spans or Rhai blocks to a function the engine doesn't declare at that arity. A name that isn't a script call (a Rust method, a WGSL helper, a roadmap proposal) goes in `docs/book/script-api-allow.txt`, by page. Details: `crates/flint-cli/CLAUDE.md`.

- Publishing pushes to the separate `docs.chaps.dev` repo (default `~/dev/docs.chaps.dev`; `-DocsRepo` / `--docs-repo` to change it). It is outward-facing: only on request.
- Renders for the book are made locally with `flint render` (no GPU in CI) and land in `docs/book/src/images/` in kebab-case.
