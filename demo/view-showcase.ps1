#!/usr/bin/env pwsh
# ============================================================
#  Quick-view the pre-built showcase scene
#
#  This opens the pre-built tavern scene in the Flint viewer
#  without running the step-by-step build script.
#
#  Usage: .\demo\view-showcase.ps1
# ============================================================

Write-Host ""
Write-Host "=== Flint Scene Viewer ===" -ForegroundColor Cyan
Write-Host ""
Write-Host "Opening: The Rusty Flint Tavern" -ForegroundColor White
Write-Host ""
Write-Host "  Camera controls:" -ForegroundColor Gray
Write-Host "    Left-drag   = orbit" -ForegroundColor Gray
Write-Host "    Right-drag  = pan" -ForegroundColor Gray
Write-Host "    Scroll      = zoom" -ForegroundColor Gray
Write-Host "    Space       = reset camera" -ForegroundColor Gray
Write-Host "    R           = force reload" -ForegroundColor Gray
Write-Host "    Escape      = quit" -ForegroundColor Gray
Write-Host ""

# Show scene info first
Write-Host "--- Scene contents ---" -ForegroundColor Yellow
flint scene info demo/showcase.scene.toml
Write-Host ""

# Launch viewer
flint serve --watch demo/showcase.scene.toml --schemas schemas
