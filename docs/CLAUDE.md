# docs: building and publishing the book (moved out of the root CLAUDE.md by #755)

The root file keeps the rule: a change that alters what the book describes updates the book in the same PR.

```bash
mdbook build docs/book                                   # must be clean before any docs commit; output in docs/book/book/ (gitignored)
powershell -ExecutionPolicy Bypass -File docs/build.ps1 -SkipRustdoc -Serve  # preview at http://localhost:3000 (drop -SkipRustdoc for the API docs)
powershell -ExecutionPolicy Bypass -File docs/build.ps1 -Publish            # mdBook + rustdoc into ~/dev/docs.chaps.dev/flint, committed and pushed
bash docs/publish.sh --publish                           # the same in bash (--skip-rustdoc, --serve, --docs-repo <dir>)
```

Without `-Publish` / `--publish` the scripts build and copy into the docs repo but don't commit. `docs/publish.bat` is `build.ps1 -Publish`. Windows PowerShell 5.1 (`powershell`) runs `build.ps1`; `pwsh` may not be installed.

- Publishing pushes to the separate `docs.chaps.dev` repo (default `~/dev/docs.chaps.dev`; `-DocsRepo` / `--docs-repo` to change it). It is outward-facing: only on request.
- Renders for the book are made locally with `flint render` (no GPU in CI) and land in `docs/book/src/images/` in kebab-case.
