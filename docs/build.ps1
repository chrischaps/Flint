#!/usr/bin/env pwsh
# ============================================================
#  Flint Documentation — Build & Publish
# ============================================================
#
#  Builds mdBook guide + rustdoc API reference, merges them,
#  copies the result to the docs.chaps.dev repo, and optionally
#  commits and pushes to publish.
#
#  Usage:
#    ./docs/build.ps1                  # Build and deploy locally
#    ./docs/build.ps1 -Publish         # Build, deploy, commit, push
#    ./docs/build.ps1 -SkipRustdoc     # Skip cargo doc (faster)
#    ./docs/build.ps1 -Serve           # Build and serve locally
#
# ============================================================

param(
    [switch]$SkipRustdoc,
    [switch]$Publish,
    [switch]$Serve,
    [string]$DocsRepo = "$env:USERPROFILE\dev\docs.chaps.dev"
)

$ErrorActionPreference = "Stop"

$ProjectRoot = Split-Path -Parent (Split-Path -Parent $PSCommandPath)
$BookDir = Join-Path $ProjectRoot "docs\book"
$BookOutput = Join-Path $BookDir "book"
$RustdocHeader = Join-Path $ProjectRoot "docs\rustdoc-header.html"
$TargetDoc = Join-Path $ProjectRoot "target\doc"
$DeployDir = Join-Path $DocsRepo "flint"

Write-Host ""
Write-Host "=== Flint Documentation Build ===" -ForegroundColor Cyan
Write-Host ""

# ----------------------------------------
# Step 1: Build mdBook
# ----------------------------------------
Write-Host "[1/4] Building mdBook..." -ForegroundColor Yellow

if (-not (Get-Command mdbook -ErrorAction SilentlyContinue)) {
    Write-Host "  mdbook not found. Install with: cargo install mdbook" -ForegroundColor Red
    exit 1
}

Push-Location $ProjectRoot
try {
    mdbook build docs/book
    if ($LASTEXITCODE -ne 0) {
        Write-Host "  mdbook build failed!" -ForegroundColor Red
        exit 1
    }
    Write-Host "  mdBook built successfully." -ForegroundColor Green
} finally {
    Pop-Location
}

# ----------------------------------------
# Step 2: Build rustdoc (optional)
# ----------------------------------------
if (-not $SkipRustdoc) {
    Write-Host "[2/4] Building rustdoc..." -ForegroundColor Yellow

    $env:RUSTDOCFLAGS = "--html-in-header $RustdocHeader"
    Push-Location $ProjectRoot
    try {
        # Temporarily allow stderr (cargo writes progress to stderr)
        $ErrorActionPreference = "Continue"
        cargo doc --workspace --no-deps 2>&1 | ForEach-Object { Write-Host "  $($_.ToString())" }
        $ErrorActionPreference = "Stop"
        if ($LASTEXITCODE -ne 0) {
            Write-Host "  cargo doc failed!" -ForegroundColor Red
            exit 1
        }
        Write-Host "  rustdoc built successfully." -ForegroundColor Green
    } finally {
        $env:RUSTDOCFLAGS = $null
        Pop-Location
    }
} else {
    Write-Host "[2/4] Skipping rustdoc (-SkipRustdoc)" -ForegroundColor DarkGray
}

# ----------------------------------------
# Step 3: Merge outputs
# ----------------------------------------
Write-Host "[3/4] Merging outputs..." -ForegroundColor Yellow

if (-not $SkipRustdoc) {
    $ApiDir = Join-Path $BookOutput "api"
    if (Test-Path $ApiDir) {
        Remove-Item -Recurse -Force $ApiDir
    }
    Copy-Item -Recurse $TargetDoc $ApiDir
    Write-Host "  Copied rustdoc to book/api/" -ForegroundColor Green
} else {
    Write-Host "  Skipped rustdoc merge" -ForegroundColor DarkGray
}

# ----------------------------------------
# Step 4: Deploy or serve
# ----------------------------------------
if ($Serve) {
    Write-Host "[4/4] Serving locally..." -ForegroundColor Yellow
    Write-Host "  Preview at http://localhost:3000" -ForegroundColor Cyan
    Push-Location $ProjectRoot
    try {
        mdbook serve docs/book --open
    } finally {
        Pop-Location
    }
} else {
    Write-Host "[4/4] Deploying to docs repo..." -ForegroundColor Yellow

    if (-not (Test-Path $DocsRepo)) {
        Write-Host "  Docs repo not found at $DocsRepo" -ForegroundColor Red
        exit 1
    }

    # Clean and copy
    if (Test-Path $DeployDir) {
        Remove-Item -Recurse -Force $DeployDir
    }
    Copy-Item -Recurse $BookOutput $DeployDir
    Write-Host "  Deployed to $DeployDir" -ForegroundColor Green
}

# ----------------------------------------
# Step 5: Publish (commit + push)
# ----------------------------------------
if ($Publish -and -not $Serve) {
    Write-Host ""
    Write-Host "=== Publishing to docs.chaps.dev ===" -ForegroundColor Cyan
    Write-Host ""

    if (-not (Test-Path (Join-Path $DocsRepo ".git"))) {
        Write-Host "  Docs repo not found at $DocsRepo" -ForegroundColor Red
        exit 1
    }

    Push-Location $DocsRepo
    try {
        git add -A
        if ($LASTEXITCODE -ne 0) {
            Write-Host "  git add failed" -ForegroundColor Red
            exit 1
        }

        # Check if there are staged changes
        git diff --cached --quiet
        if ($LASTEXITCODE -eq 0) {
            Write-Host "  No changes to publish - docs are already up to date." -ForegroundColor Green
        } else {
            git commit -m "Update Flint docs"
            if ($LASTEXITCODE -ne 0) {
                Write-Host "  git commit failed" -ForegroundColor Red
                exit 1
            }

            git push
            if ($LASTEXITCODE -ne 0) {
                Write-Host "  git push failed" -ForegroundColor Red
                exit 1
            }

            Write-Host "  Docs published successfully." -ForegroundColor Green
        }
    } finally {
        Pop-Location
    }
} elseif (-not $Serve) {
    Write-Host ""
    Write-Host "=== Build complete ===" -ForegroundColor Cyan
    Write-Host ""
    Write-Host "  To publish, run again with -Publish" -ForegroundColor White
    Write-Host "  Or manually: cd $DocsRepo, then git add/commit/push" -ForegroundColor Gray
    Write-Host ""
}
