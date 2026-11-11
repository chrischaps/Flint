### The book's script API reference is generated, and `flint script api --check-book` keeps a book honest (#695)
**What changed.** The book has a Script API Reference page (Reference,
`docs/book/src/reference/script-api.md`): every function, its parameters'
kinds, what it returns, the conventions and the callbacks, written from the
engine's declarations by `flint script api --book`. The Scripting chapter's
function tables are generated the same way, between `script-api:begin` and
`script-api:end` comments. `flint script api --check-book [DIR]` exits 1 when
that text is stale or when a page calls a function the engine doesn't have,
or at an arity it isn't declared at (CI runs it on the engine's book). Fixed
calls the book made to functions that never existed: `action_held` (use
`is_action_pressed`), `entity()` (use `self_entity()`), and an undefined
`turn_view` and `current_mix` in two examples.
**What to do.** Nothing. A game with its own mdBook can check its pages
against the engine's API with `flint script api --check-book <book dir>`;
names there that aren't script calls go in `<book dir>/script-api-allow.txt`.
