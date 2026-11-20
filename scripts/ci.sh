#!/usr/bin/env bash
# What a change must pass before it lands (#757).
#
#   scripts/ci.sh          formatting, every cited ADR has a record
#                          (scripts/adr-check.sh), every clippy.toml is the
#                          root list's copy (scripts/clippy-conf.sh --check, #761),
#                          no FLINT_* read past flint_core::env (#715),
#                          the tree's shape (#760), no examples folder or
#                          path to one (scripts/examples-check.sh, #984),
#                          its size budget (scripts/size-check.py, ADR 0122),
#                          the root CLAUDE.md's (scripts/claude-md-check.py,
#                          #992) and the book's crate table (tools/arch-analyzer
#                          --check-book): seconds, and one small build (the
#                          analyzer: 23 s and 126 MB cold, once a worktree). `scripts/issue.sh
#                          land` runs this (verify= in scripts/issue.conf), and
#                          so does CI's rustfmt job.
#   scripts/ci.sh --full   also what CI's clippy and wasm jobs run: native
#                          `cargo clippy --all-targets`, the book against
#                          the script API (`flint script api --check-book`, CI's
#                          headless job), the book's environment page (`flint env
#                          --check-book`, the same job), the book's flags, styles and images
#                          against that `flint` (scripts/docs-check.py, the
#                          same job) and scripts/wasm-check.sh. Minutes, and
#                          three builds (the third is the debug `flint` most
#                          worktrees have already).
#
# Formatting is rustfmt's defaults at edition 2021 (rustfmt.toml). Fix a failure
# with `cargo fmt --all`; one file alone is `rustfmt <file>`.
set -euo pipefail

cd "$(dirname "$0")/.."

echo "ci: cargo fmt --all --check"
if ! cargo fmt --all -- --check >/dev/null 2>&1; then
    echo "ci: these files are not formatted:" >&2
    cargo fmt --all -- --check -l 2>/dev/null | sort -u >&2 || true
    echo "ci: run \`cargo fmt --all\`, commit, and try again" >&2
    exit 1
fi

echo "ci: scripts/adr-check.sh"
bash scripts/adr-check.sh

echo "ci: scripts/clippy-conf.sh --check"
bash scripts/clippy-conf.sh --check

# The root clippy.toml's rules are denied in every member (#979): each takes the
# workspace's lints, and none overrides them with a `[lints.*]` of its own, so a
# new crate can't leave a read past the vfs at a warning or unchecked.
echo "ci: every member takes [lints] workspace = true"
members=$(awk '/^members *= *\[/{on=1; next} on && /^\]/{exit} on' Cargo.toml | tr -d '\r' | sed -n 's/^ *"\([^"]*\)".*/\1/p')
bad_lints=""
for m in $members; do
    manifest="$m/Cargo.toml"
    if ! tr -d '\r' < "$manifest" | awk '/^\[lints\]/{l=1; next} /^\[/{l=0} l && /^workspace *= *true/{ok=1} END{exit !ok}'; then
        bad_lints="$bad_lints  $manifest: no \`[lints] workspace = true\`"$'\n'
    fi
    if tr -d '\r' < "$manifest" | grep -qE '^\[lints\.'; then
        bad_lints="$bad_lints  $manifest: a \`[lints.*]\` table of its own"$'\n'
    fi
done
if [ -n "$bad_lints" ]; then
    echo "ci: every workspace member takes the workspace's lints (root Cargo.toml, #979):" >&2
    printf '%s' "$bad_lints" >&2
    echo "ci: add \`[lints]\` / \`workspace = true\` to the manifest; a call that means it carries #[allow(clippy::disallowed_methods)] // <reason>" >&2
    exit 1
fi

# Every FLINT_* switch is read through its row in flint_core::env (#715), so
# `flint env` and the book list it and a bad value warns. The table's test
# (crates/flint-core/tests/env_table.rs) also holds every "FLINT_..." name to it.
echo "ci: FLINT_* read only through flint_core::env"
reads=$(git grep -nE '(^|[^_[:alnum:]])var(_os)?\(\s*"FLINT_' -- '*.rs' ':!crates/flint-core/src/env.rs' || true)
if [ -n "$reads" ]; then
    echo "ci: these read a FLINT_* variable past flint_core::env; use the switch's getter (add a row for a new one):" >&2
    echo "$reads" >&2
    exit 1
fi

# The tree (#760): a symlink checks out as a text file on Windows without the
# symlink privilege, and the root is for manifests and docs (a render, spec
# or texture there is a stray: specs/ and tests/fixtures/ hold them)
echo "ci: no symlinks, a root of manifests and docs"
links=$(git ls-files -s | awk '$1 == "120000" { print $4 }')
if [ -n "$links" ]; then
    echo "ci: these are symlinks; commit the files, or name the path they point at:" >&2
    echo "$links" >&2
    exit 1
fi
strays=$(git ls-files | grep -v / | grep -vxE '\.git(ignore|attributes)|[A-Z]+\.md|Cargo\.(toml|lock)|(clippy|rustfmt|flint)\.toml' || true)
if [ -n "$strays" ]; then
    echo "ci: these files are at the root, which holds only manifests and docs:" >&2
    echo "$strays" >&2
    exit 1
fi

echo "ci: scripts/examples-check.sh"
bash scripts/examples-check.sh

echo "ci: scripts/size-check.py"
# python3 where it runs (Windows' may be a stub); standard library only
py=python3
"$py" --version >/dev/null 2>&1 || py=python
"$py" scripts/size-check.py

# Every session reads the root CLAUDE.md whole (#992): it went from 18 KB
# after #755 to 35 KB in two weeks with nothing holding it
echo "ci: scripts/claude-md-check.py"
"$py" scripts/claude-md-check.py

# The book's crate table (#764), from every Cargo.toml: a dependency added
# without `cargo run -p flint-arch-analyzer -- --book` kept CI's crate table
# job red on main from #853 to #969, failing every PR in between
echo "ci: flint-arch-analyzer --check-book"
cargo run --locked -q -p flint-arch-analyzer -- --check-book

if [ "${1:-}" = "--full" ]; then
    echo "ci: cargo clippy --all-targets"
    cargo clippy --all-targets --keep-going
    echo "ci: flint script api --check-book"
    cargo run --locked -q -p flint-cli --bin flint -- script api --check-book
    echo "ci: flint env --check-book"
    cargo run --locked -q -p flint-cli --bin flint -- env --check-book
    echo "ci: scripts/docs-check.py"
    # The flint just built; python3 where it runs (Windows' may be a stub)
    py=python3
    "$py" --version >/dev/null 2>&1 || py=python
    "$py" scripts/docs-check.py
    echo "ci: scripts/wasm-check.sh"
    bash scripts/wasm-check.sh
fi

echo "ci: ok"
