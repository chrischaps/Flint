#!/usr/bin/env bash
# What a change must pass before it lands (#757).
#
#   scripts/ci.sh          formatting, every cited ADR has a record
#                          (scripts/adr-check.sh), the tree's shape (#760) and
#                          its size budget (scripts/size-check.py, ADR 0122):
#                          seconds, no build. `scripts/issue.sh
#                          land` runs this (verify= in scripts/issue.conf), and
#                          so does CI's rustfmt job.
#   scripts/ci.sh --full   also what CI's clippy and wasm jobs run: native
#                          `cargo clippy --all-targets`, the book's crate table
#                          (tools/arch-analyzer --check-book), the book against
#                          the script API (`flint script api --check-book`, CI's
#                          headless job), the book's flags, styles and images
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

# The tree (#760): a symlink checks out as a text file on Windows without the
# symlink privilege, and the root is for manifests and docs (a render, spec
# or texture there is a stray: specs/, examples/ and tests/fixtures/ hold them)
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

echo "ci: scripts/size-check.py"
# python3 where it runs (Windows' may be a stub); standard library only
py=python3
"$py" --version >/dev/null 2>&1 || py=python
"$py" scripts/size-check.py

if [ "${1:-}" = "--full" ]; then
    echo "ci: cargo clippy --all-targets"
    cargo clippy --all-targets --keep-going
    echo "ci: flint-arch-analyzer --check-book"
    cargo run --locked -q -p flint-arch-analyzer -- --check-book
    echo "ci: flint script api --check-book"
    cargo run --locked -q -p flint-cli --bin flint -- script api --check-book
    echo "ci: scripts/docs-check.py"
    # The flint just built; python3 where it runs (Windows' may be a stub)
    py=python3
    "$py" --version >/dev/null 2>&1 || py=python
    "$py" scripts/docs-check.py
    echo "ci: scripts/wasm-check.sh"
    bash scripts/wasm-check.sh
fi

echo "ci: ok"
