#!/usr/bin/env bash
# One list of disallowed methods, every reader made from it (#761).
#
# The root clippy.toml is the list. Clippy reads the clippy.toml nearest a
# crate's manifest, so a crate with a file of its own reads that instead of
# the root's; this script writes those files from the root's, and the wasm
# check's from the root's plus scripts/wasm-clippy/wasm-only.toml, so no rule
# is written twice.
#
#   scripts/clippy-conf.sh             write each tool crate's clippy.toml, and
#                                      flint-render's (the root list plus
#                                      scripts/clippy-render.toml, #1091)
#   scripts/clippy-conf.sh --check     exit 1 when one is stale (scripts/ci.sh runs it)
#   scripts/clippy-conf.sh --wasm DIR  write DIR/clippy.toml, for CLIPPY_CONF_DIR
#                                      (scripts/wasm-check.sh)
#
# A file is rewritten only when its text changes: clippy tracks its config
# file, and a new mtime would re-lint every crate.
set -euo pipefail
cd "$(dirname "$0")/.."

# Crates the player never links. They read the disk directly by design, so
# they hold the root list's every-crate group (the wgpu and egui seams) and
# not its runtime group (the vfs and as_float rules).
TOOL_CRATES=(
    crates/flint-cli
    crates/flint-viewer
    crates/flint-asset-gen
    crates/flint-procgen-ai
    crates/flint-ai-core
    tools/arch-analyzer
)

ROOT=clippy.toml
WASM_ONLY=scripts/wasm-clippy/wasm-only.toml
# The renderer's own rules (#1091): every GPU object it makes is counted, so
# flint-render reads the whole root list plus these, as its own clippy.toml.
RENDER_ONLY=scripts/clippy-render.toml
RENDER_CRATE=crates/flint-render

# The lines between `# >>> <group>` and `# <<< <group>` in a file.
group() {
    tr -d '\r' <"$1" | awk -v g="$2" '
        $0 ~ "# <<< " g "$" { on = 0 }
        on { print }
        $0 ~ "# >>> " g "$" { on = 1 }'
}

# The lines inside a file's `disallowed-methods = [ ... ]`.
entries() {
    tr -d '\r' <"$1" | awk '
        /^disallowed-methods = \[/ { on = 1; next }
        on && /^\]/ { on = 0 }
        on { print }'
}

tool_conf() {
    local body
    body="$(group "$ROOT" every-crate)"
    if [ -z "$body" ]; then
        echo "clippy-conf: no '# >>> every-crate' group in $ROOT" >&2
        exit 1
    fi
    printf '%s\n' \
        "# Written by scripts/clippy-conf.sh from the root clippy.toml (#761); edit" \
        "# that file and rerun the script, never this one. This crate never runs in" \
        "# the player and reads the disk directly, so it holds the root list's" \
        "# every-crate group (the wgpu and egui seams), not its runtime group." \
        "disallowed-methods = [" \
        "$body" \
        "]"
}

render_conf() {
    printf '%s\n' \
        "# Written by scripts/clippy-conf.sh (#761, #1091) from the root clippy.toml's" \
        "# list and scripts/clippy-render.toml's; edit those and rerun the script," \
        "# never this one. The renderer counts every GPU object it makes" \
        "# (flint_render::gpu_counts), so the bare calls are refused here alone." \
        "disallowed-methods = [" \
        "$(entries "$ROOT")" \
        "$(entries "$RENDER_ONLY")" \
        "]"
}

wasm_conf() {
    printf '%s\n' \
        "# Written by scripts/clippy-conf.sh --wasm (#761): the root clippy.toml's" \
        "# list, then scripts/wasm-clippy/wasm-only.toml's." \
        "disallowed-methods = [" \
        "$(entries "$ROOT")" \
        "$(entries "$WASM_ONLY")" \
        "]"
}

# write <file> <text>: replace the file only when the text differs.
write() {
    if [ -f "$1" ] && [ "$(tr -d '\r' <"$1")" = "$2" ]; then
        return
    fi
    mkdir -p "$(dirname "$1")"
    printf '%s\n' "$2" >"$1"
    echo "wrote $1"
}

case "${1:-}" in
"")
    for c in "${TOOL_CRATES[@]}"; do
        write "$c/clippy.toml" "$(tool_conf)"
    done
    write "$RENDER_CRATE/clippy.toml" "$(render_conf)"
    ;;
--check)
    want="$(tool_conf)"
    stale=0
    for c in "${TOOL_CRATES[@]}"; do
        if [ ! -f "$c/clippy.toml" ] || [ "$(tr -d '\r' <"$c/clippy.toml")" != "$want" ]; then
            echo "clippy-conf: $c/clippy.toml is not the root clippy.toml's every-crate group" >&2
            stale=1
        fi
    done
    if [ ! -f "$RENDER_CRATE/clippy.toml" ] || [ "$(tr -d '\r' <"$RENDER_CRATE/clippy.toml")" != "$(render_conf)" ]; then
        echo "clippy-conf: $RENDER_CRATE/clippy.toml is not the root list plus $RENDER_ONLY" >&2
        stale=1
    fi
    if [ "$stale" = 1 ]; then
        echo "clippy-conf: run 'bash scripts/clippy-conf.sh' and commit what it writes" >&2
        exit 1
    fi
    echo "clippy-conf: ${#TOOL_CRATES[@]} tool crates hold the root list's every-crate group, $RENDER_CRATE the root list and its own"
    ;;
--wasm)
    dir="${2:?--wasm needs a directory}"
    write "$dir/clippy.toml" "$(wasm_conf)"
    ;;
*)
    echo "usage: scripts/clippy-conf.sh [--check | --wasm DIR]" >&2
    exit 2
    ;;
esac
