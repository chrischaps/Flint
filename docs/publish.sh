#!/usr/bin/env bash
# ============================================================
#  Flint Documentation — Build & Publish
# ============================================================
#
#  Builds mdBook guide + rustdoc API reference, merges them,
#  copies the result to the docs.chaps.dev repo, and optionally
#  commits and pushes to publish.
#
#  Usage:
#    ./docs/publish.sh                  # Build and deploy locally
#    ./docs/publish.sh --publish        # Build, deploy, commit, push
#    ./docs/publish.sh --skip-rustdoc   # Skip cargo doc (faster)
#    ./docs/publish.sh --serve          # Build and serve locally
#
# ============================================================

set -euo pipefail

SKIP_RUSTDOC=false
PUBLISH=false
SERVE=false
DOCS_REPO="${HOME}/dev/docs.chaps.dev"

while [[ $# -gt 0 ]]; do
    case "$1" in
        --skip-rustdoc) SKIP_RUSTDOC=true; shift ;;
        --publish)      PUBLISH=true; shift ;;
        --serve)        SERVE=true; shift ;;
        --docs-repo)    DOCS_REPO="$2"; shift 2 ;;
        *)              echo "Unknown option: $1"; exit 1 ;;
    esac
done

PROJECT_ROOT="$(cd "$(dirname "$0")/.." && pwd)"
BOOK_DIR="${PROJECT_ROOT}/docs/book"
BOOK_OUTPUT="${BOOK_DIR}/book"
RUSTDOC_HEADER="${PROJECT_ROOT}/docs/rustdoc-header.html"
TARGET_DOC="${PROJECT_ROOT}/target/doc"
DEPLOY_DIR="${DOCS_REPO}/flint"

echo ""
echo "=== Flint Documentation Build ==="
echo ""

# ----------------------------------------
# Step 1: Build mdBook
# ----------------------------------------
echo "[1/4] Building mdBook..."

if ! command -v mdbook &>/dev/null; then
    echo "  mdbook not found. Install with: cargo install mdbook"
    exit 1
fi

(cd "$PROJECT_ROOT" && mdbook build docs/book)
echo "  mdBook built successfully."

# ----------------------------------------
# Step 2: Build rustdoc (optional)
# ----------------------------------------
if [[ "$SKIP_RUSTDOC" == false ]]; then
    echo "[2/4] Building rustdoc..."

    RUSTDOCFLAGS="--html-in-header ${RUSTDOC_HEADER}" \
        cargo doc --manifest-path "${PROJECT_ROOT}/Cargo.toml" \
        --workspace --no-deps --exclude flint-android 2>&1 | sed 's/^/  /'

    echo "  rustdoc built successfully."
else
    echo "[2/4] Skipping rustdoc (--skip-rustdoc)"
fi

# ----------------------------------------
# Step 3: Merge outputs
# ----------------------------------------
echo "[3/4] Merging outputs..."

if [[ "$SKIP_RUSTDOC" == false ]]; then
    API_DIR="${BOOK_OUTPUT}/api"
    rm -rf "$API_DIR"
    cp -r "$TARGET_DOC" "$API_DIR"
    echo "  Copied rustdoc to book/api/"
else
    echo "  Skipped rustdoc merge"
fi

# ----------------------------------------
# Step 4: Deploy or serve
# ----------------------------------------
if [[ "$SERVE" == true ]]; then
    echo "[4/4] Serving locally..."
    echo "  Preview at http://localhost:3000"
    (cd "$PROJECT_ROOT" && mdbook serve docs/book --open)
else
    echo "[4/4] Deploying to docs repo..."

    if [[ ! -d "$DOCS_REPO" ]]; then
        echo "  Docs repo not found at ${DOCS_REPO}"
        exit 1
    fi

    rm -rf "$DEPLOY_DIR"
    cp -r "$BOOK_OUTPUT" "$DEPLOY_DIR"
    echo "  Deployed to ${DEPLOY_DIR}"
fi

# ----------------------------------------
# Step 5: Publish (commit + push)
# ----------------------------------------
if [[ "$PUBLISH" == true && "$SERVE" == false ]]; then
    echo ""
    echo "=== Publishing to docs.chaps.dev ==="
    echo ""

    if [[ ! -d "${DOCS_REPO}/.git" ]]; then
        echo "  Docs repo not found at ${DOCS_REPO}"
        exit 1
    fi

    cd "$DOCS_REPO"
    git add -A

    if git diff --cached --quiet; then
        echo "  No changes to publish - docs are already up to date."
    else
        git commit -m "Update Flint docs"
        git push
        echo "  Docs published successfully."
    fi
elif [[ "$SERVE" == false ]]; then
    echo ""
    echo "=== Build complete ==="
    echo ""
    echo "  To publish, run again with --publish"
    echo "  Or manually: cd ${DOCS_REPO}, then git add/commit/push"
    echo ""
fi
