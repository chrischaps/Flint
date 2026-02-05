#!/usr/bin/env pwsh
# ============================================================
#  Explore the showcase scene with queries
#
#  Demonstrates the query and introspection capabilities
#  of the Flint CLI.
# ============================================================

$scene = "demo/showcase.scene.toml"
$schemas = "schemas"

Write-Host ""
Write-Host "=== Exploring The Rusty Flint Tavern ===" -ForegroundColor Cyan
Write-Host ""

# Scene overview
Write-Host "--- Scene Overview ---" -ForegroundColor Yellow
flint scene info $scene
Write-Host ""

# All entities
Write-Host "--- All Entities ($(flint query 'entities' --scene $scene | ConvertFrom-Json | Measure-Object | Select-Object -ExpandProperty Count) total) ---" -ForegroundColor Yellow
flint query "entities" --scene $scene --format json
Write-Host ""

# By archetype
Write-Host "--- Rooms ---" -ForegroundColor Blue
flint query "entities where archetype == 'room'" --scene $scene
Write-Host ""

Write-Host "--- Doors ---" -ForegroundColor DarkYellow
flint query "entities where archetype == 'door'" --scene $scene
Write-Host ""

Write-Host "--- Furniture ---" -ForegroundColor Green
flint query "entities where archetype == 'furniture'" --scene $scene
Write-Host ""

Write-Host "--- Characters ---" -ForegroundColor Yellow
flint query "entities where archetype == 'character'" --scene $scene
Write-Host ""

# Property-based queries
Write-Host "============================================" -ForegroundColor Cyan
Write-Host "  Property-based queries" -ForegroundColor Cyan
Write-Host "============================================" -ForegroundColor Cyan
Write-Host ""

Write-Host "--- Locked doors (which ones need keys?) ---" -ForegroundColor Red
flint query "entities where door.locked == true" --scene $scene
Write-Host ""

Write-Host "--- Sliding doors ---" -ForegroundColor Magenta
flint query "entities where door.style == 'sliding'" --scene $scene
Write-Host ""

# Schema introspection
Write-Host "============================================" -ForegroundColor Cyan
Write-Host "  Schema introspection" -ForegroundColor Cyan
Write-Host "============================================" -ForegroundColor Cyan
Write-Host ""

Write-Host "--- Component: door ---" -ForegroundColor Yellow
flint schema door --schemas $schemas
Write-Host ""

Write-Host "--- Component: transform ---" -ForegroundColor Yellow
flint schema transform --schemas $schemas
Write-Host ""

Write-Host "--- Archetype: room ---" -ForegroundColor Yellow
flint schema room --schemas $schemas
Write-Host ""

Write-Host "--- Archetype: character ---" -ForegroundColor Yellow
flint schema character --schemas $schemas
Write-Host ""

# Entity detail
Write-Host "============================================" -ForegroundColor Cyan
Write-Host "  Entity detail view" -ForegroundColor Cyan
Write-Host "============================================" -ForegroundColor Cyan
Write-Host ""

Write-Host "--- Detail: mysterious_stranger ---" -ForegroundColor Yellow
flint entity show mysterious_stranger --scene $scene --schemas $schemas
Write-Host ""

Write-Host "--- Detail: storage_door (the locked one) ---" -ForegroundColor Yellow
flint entity show storage_door --scene $scene --schemas $schemas
Write-Host ""

Write-Host "============================================" -ForegroundColor Cyan
Write-Host "  Done! The scene file is at:" -ForegroundColor White
Write-Host "    $scene" -ForegroundColor Gray
Write-Host ""
Write-Host "  Every operation above used the CLI." -ForegroundColor White
Write-Host "  An AI agent can do all of this." -ForegroundColor White
Write-Host "============================================" -ForegroundColor Cyan
