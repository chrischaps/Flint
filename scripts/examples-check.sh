#!/usr/bin/env bash
# The examples are another repository (#984, ADR 0131): flint-examples, with
# the scenes at its root. Nothing in this tree may hold them again or name
# the folder they left, since a game's engine/ would then carry it, or a
# path would lead nowhere. `scripts/ci.sh` runs this.
#
# An example is cited as `flint-examples/<path>`: the path in a clone of
# that repository inside this checkout (`.gitignore` keeps it out), which
# docs-check holds to a real file there (scripts/docs-check.py, "examples").
#
# Not refused, because they are other folders: `schemas/examples/` and
# `engine/schemas/examples/` (the room-builder layer), `specs/examples/`,
# and a crate's own `crates/<crate>/examples/` (cargo's, which a crate's
# Cargo.toml names as `path = "examples/..."`, so manifests aren't read).
# The changelog is history, and a pending fragment there says what moved;
# ADR 0131 is the record of the move and names the folder it left.
set -euo pipefail

cd "$(dirname "$0")/.."

status=0

if [ -n "$(git ls-files -- examples)" ]; then
    echo "examples-check: examples/ is tracked again; the examples live in flint-examples (#984)" >&2
    status=1
fi

# A path to the old folder: `examples/...`, `../examples/...`, a bare
# `examples/`, not after a letter, digit, `_`, `.`, `/` or `-` (so not
# `schemas/examples/`, `flint-examples/`, `crates/x/examples/`).
named=$(git grep -nIE '(^|[^A-Za-z0-9_./-])(\.\./)*examples/' -- . \
    ':!changelog/' ':!changelog.d/' ':!CHANGELOG.md' ':!scripts/examples-check.sh' \
    ':!crates/*/Cargo.toml' ':!docs/adr/0131-the-examples-are-a-project-of-their-own.md' || true)
if [ -n "$named" ]; then
    echo "examples-check: these name the examples folder, which moved to flint-examples (#984):" >&2
    echo "$named" >&2
    echo "examples-check: cite one as flint-examples/<path> (scenes at its root), or give a test its own copy under tests/fixtures/scenes/" >&2
    status=1
fi

exit $status
