### `flint check`: one pre-flight for a project; a constraint with a bad query now fails, and so do a few other things that passed in silence (#687)
**What changed.** New command: `flint check [dir|scene] [--json] [--strict]`
lists everything wrong with a project in one run (schemas, entity references,
every asset and file a scene names with the paths tried, styles, constraints)
and exits 1 on an error. `flint validate` and `flint bundle` run the same
checks, and `flint init` prints `flint check` as its first next step.

What now exits non-zero that did not:

| Case | Was | Now |
|---|---|---|
| A constraint whose `query` (or `query_rule` `rule`) does not parse, or selects `components` | skipped: it passed every scene | an error naming the constraint, its file and the parse error, in `flint validate`, `flint check` and the viewer |
| A query with text after its one condition (`entities where a == 1 and b == 2`), in `flint query` too | read as `a == 1`, the rest dropped | a parse error: there is no `and`, `or` or `not` |
| `flint validate`: a `joint.parent` or `ik_two_bone.target` / `pole` / `tip` naming no entity, or a `tip` that is not a child | exit 0 | `error[dangling-entity]`, exit 1 |
| `flint validate --fix` with an error-level violation no fix covers; `--dry-run` on a scene with one | exit 0 | exit 1: the exit code says whether the file, as the run left it, has an error |
| `flint validate --format` other than `text` or `json` | printed text | refused (exit 2) |
| `flint bundle`: a model whose only source is a `<name>.procgen.toml` that is empty or does not parse | packed | `error[bad-spec]` or `error[missing-asset]` |

A scene with none of these passes `flint validate` as before. Also:
`--fix` and `--dry-run` honour `--format json` (a `fix` object in the same
document); each JSON diagnostic has a new `tried` array; `flint bundle`'s
refusal lists problems as `flint check` prints them.
**What to do.** Run `flint check`. Rewrite each `bad-constraint` query as one
condition (it was never being checked, so expect real violations once it
runs); fix or empty (`""`) each dangling `joint.parent` and IK field. A CI
step that reads the exit code of `flint validate --fix` or `--dry-run` now
gets "the scene still has an error". Use `flint check` as the gate before
playing or bundling.
