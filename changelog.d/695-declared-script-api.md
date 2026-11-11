### `flint script api` lists the script API; Entity functions take integers (#695)
**What changed.** `flint script api` prints every function a script can call
and the callbacks; `--json` is the same as data, `--filter draw` narrows it.
The Entity API (`set_position`, `get_component`, `set_material_override`, ...)
is declared through the engine's new API builder (ADR 0116): an integer works
wherever it wants a number (`set_position(id, 0, 1, 0)`), a wrong argument is
an error naming the function, the parameter and the line, and every Entity
function given `-1` or a despawned id warns once per script, getters included
(they still return their empty value). A game's `PlayerExtension` can declare
functions the same way with `declare_script_api`.
**What to do.** Nothing. A new warning names a getter that was quietly reading
a missing entity: check it with `entity_exists(id)` first.
