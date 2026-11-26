#!/usr/bin/env bash
# The package matrix (#1336, epic #1328): every package builds alone, all on,
# and none, in the player and in the CLI.
#
# A package that compiles only when everything is on rots, as `Path::is_dir`
# in a linked crate once did for wasm. So this builds, for flint-player and
# flint-cli, with `--no-default-features` (no sound card or gamepad either,
# so a Linux box needs no device headers) and:
#
#   none       no package
#   standard   the standard tier only (procgen, debug-hud): what a game that
#              lists none of the feature packages gets, the epic's done-when
#   <name>     each package alone, with what its manifest [requires]
#   all        every package
#
# The packages are read from packages/*/package.toml: a manifest whose
# `[build] features` the binary has is in its matrix, so a new package joins
# it with no edit here. For each configuration it
#
#   1. reads `cargo tree` and fails if a left-out package's crates
#      (`[provides] crates`) are linked, naming who pulls each;
#   2. with --build, runs `cargo check` (tests and examples included), all
#      configurations in the one target dir, and fails naming each that
#      didn't compile.
#
# It goes on past a failure, so one run lists every broken configuration.
#
#   scripts/package-check.sh                 the trees only (seconds, no build)
#   scripts/package-check.sh --build         the trees, then cargo check each
#   scripts/package-check.sh --build music   only the configurations named
set -euo pipefail

cd "$(dirname "$0")/.."

build=0
only=()
for arg in "$@"; do
    case "$arg" in
        --build) build=1 ;;
        -h|--help) sed -n '2,29p' "$0" | sed 's/^# \{0,1\}//'; exit 0 ;;
        -*) echo "package-check: unknown flag $arg" >&2; exit 2 ;;
        *) only+=("$arg") ;;
    esac
done

# No exception: every package crate is out of a tree whose package is out.
# The last, flint-cli's flint-procgen, went optional with #1543 (its wooze
# field had gone behind its own `living` feature in #1516).

# --- the manifests --------------------------------------------------------

# A string or array value of `key` in `[section]` of a package.toml, its
# items space-separated. An array may run over several lines.
field() {
    tr -d '\r' < "$1" | awk -v want="$2" -v key="$3" '
        function finish() {
            gsub(/[][",]/, " ", value); gsub(/ +/, " ", value)
            sub(/^ /, "", value); sub(/ $/, "", value); print value; exit
        }
        !open && /^\[/ { section = $0; gsub(/[][ ]/, "", section); next }
        open { line = $0; sub(/#.*/, "", line); value = value " " line; if (line ~ /\]/) finish(); next }
        section == want && $0 ~ "^" key " *=" {
            line = $0; sub(/^[^=]*= */, "", line); sub(/#.*/, "", line); value = line
            # An array left open runs on to the line that closes it
            if (line ~ /^\[/ && line !~ /\]/) { open = 1; next }
            finish()
        }
    '
}

# The keys of `[section]`: a manifest's [requires].
keys() {
    tr -d '\r' < "$1" | awk -v want="$2" '
        /^\[/ { section = $0; gsub(/[][ ]/, "", section); next }
        section == want && /^[A-Za-z0-9_-]+ *=/ { sub(/ *=.*/, ""); print }
    ' | tr '\n' ' ' | sed 's/ $//'
}

# The feature names of a crate's Cargo.toml.
cargo_features() {
    tr -d '\r' < "crates/$1/Cargo.toml" | awk '
        /^\[/ { on = ($0 == "[features]"); next }
        on && /^[A-Za-z0-9_-]+ *=/ { sub(/ *=.*/, ""); print }
    '
}

declare -A TIER REQUIRES FEATURES CRATES
PACKAGES=()
for manifest in packages/*/package.toml; do
    name=$(field "$manifest" package name)
    features=$(field "$manifest" build features)
    # The kernel has no switch: it is in every build
    [ -n "$features" ] || continue
    PACKAGES+=("$name")
    TIER[$name]=$(field "$manifest" package tier)
    REQUIRES[$name]=$(keys "$manifest" requires)
    FEATURES[$name]=$features
    CRATES[$name]=$(field "$manifest" provides crates)
done
if [ ${#PACKAGES[@]} -eq 0 ]; then
    echo "package-check: no package with [build] features under packages/" >&2
    exit 1
fi

# A set of packages with everything they require, sorted.
closure() {
    local -A seen=()
    local queue=("$@") p r
    while [ ${#queue[@]} -gt 0 ]; do
        p=${queue[0]}; queue=("${queue[@]:1}")
        [ -n "$p" ] || continue
        [ -z "${seen[$p]:-}" ] || continue
        seen[$p]=1
        for r in ${REQUIRES[$p]:-}; do queue+=("$r"); done
    done
    printf '%s\n' "${!seen[@]}" | sort | tr '\n' ' ' | sed 's/ $//'
}

in_list() { # word list...
    local w=$1; shift
    for x in "$@"; do [ "$x" = "$w" ] && return 0; done
    return 1
}

annotate() { # GitHub's error line, so the run's page names the configuration
    if [ -n "${GITHUB_ACTIONS:-}" ]; then echo "::error title=package matrix::$1"; fi
}

# --- the matrix -----------------------------------------------------------

FAILURES=()
SUMMARY=()

for bin in flint-player flint-cli; do
    mapfile -t have < <(cargo_features "$bin")
    # The packages this binary can leave out: each of its build features is
    # one of the binary's own
    applicable=()
    for p in "${PACKAGES[@]}"; do
        ok=1
        for f in ${FEATURES[$p]}; do in_list "$f" "${have[@]}" || ok=0; done
        [ $ok -eq 1 ] && applicable+=("$p")
    done
    standard=()
    for p in "${applicable[@]}"; do [ "${TIER[$p]}" = standard ] && standard+=("$p"); done

    configs=("none=" "standard=${standard[*]}")
    for p in "${applicable[@]}"; do configs+=("$p=$p"); done
    configs+=("all=${applicable[*]}")

    echo "== $bin: packages ${applicable[*]}"
    declare -A done_sets=()
    for entry in "${configs[@]}"; do
        label=${entry%%=*}
        if [ ${#only[@]} -gt 0 ] && ! in_list "$label" "${only[@]}"; then continue; fi
        # shellcheck disable=SC2086
        on=$(closure ${entry#*=})
        features=""
        for p in $on; do
            in_list "$p" "${applicable[@]}" || continue
            for f in ${FEATURES[$p]}; do features="$features,$f"; done
        done
        features=${features#,}
        flags="--no-default-features${features:+ --features $features}"
        name="$bin [$label]"
        [ "$on" = "$label" ] || name="$bin [$label: ${on:-no package}]"
        if [ -n "${done_sets[_$features]:-}" ]; then
            echo "-- $name: the same build as [${done_sets[_$features]}]"
            continue
        fi
        done_sets[_$features]=$label

        # 1. The tree: no crate of a package that is out
        # shellcheck disable=SC2086
        tree=$(cargo tree -q -p "$bin" $flags -e normal --prefix none | cut -d' ' -f1 | sort -u)
        bad=()
        for p in "${applicable[@]}"; do
            in_list "$p" $on && continue
            for c in ${CRATES[$p]}; do
                printf '%s\n' "$tree" | grep -qx "$c" || continue
                bad+=("$c ($p)")
            done
        done

        tree_result="ok"
        if [ ${#bad[@]} -gt 0 ]; then
            tree_result="FAIL"
            echo "FAIL $name links a left-out package's crate ($flags):" >&2
            for b in "${bad[@]}"; do
                echo "  $b; who pulls it:" >&2
                # shellcheck disable=SC2086
                cargo tree -q -p "$bin" $flags -e normal,features -i "${b%% *}" 2>&1 | sed 's/^/    /' >&2 || true
            done
            FAILURES+=("$name: links ${bad[*]}")
            annotate "$name links ${bad[*]} (cargo tree -p $bin $flags)"
        fi
        crates=$(printf '%s\n' "$tree" | grep -c . || true)
        echo "-- $name: tree $tree_result, $crates crates ($flags)"

        # 2. The build
        check_result="-"
        if [ $build -eq 1 ]; then
            start=$(date +%s)
            # shellcheck disable=SC2086
            if cargo check -q -p "$bin" $flags --all-targets; then
                check_result="ok"
            else
                check_result="FAIL"
                echo "FAIL $name doesn't compile: cargo check -p $bin $flags --all-targets" >&2
                FAILURES+=("$name: cargo check -p $bin $flags --all-targets")
                annotate "$name doesn't compile (cargo check -p $bin $flags --all-targets)"
            fi
            echo "-- $name: check $check_result in $(( $(date +%s) - start )) s"
        fi
        SUMMARY+=("$(printf '%-12s %-9s %-38s %-5s %-5s %6s' "$bin" "$label" "${on:--}" "$tree_result" "$check_result" "$crates")")
    done
    unset done_sets
done

echo
printf '%-12s %-9s %-38s %-5s %-5s %6s\n' binary config packages tree check crates
printf '%s\n' "${SUMMARY[@]}"

if [ ${#FAILURES[@]} -gt 0 ]; then
    echo >&2
    echo "package-check: ${#FAILURES[@]} configuration(s) failed:" >&2
    printf '  %s\n' "${FAILURES[@]}" >&2
    exit 1
fi
echo "package-check: ok"
