#!/usr/bin/env pwsh
# ============================================================
#  Flint Engine Showcase: The Rusty Flint Tavern
# ============================================================
#
#  This script builds a tavern scene from scratch using only
#  the Flint CLI, demonstrating that an AI agent (or script)
#  can programmatically create and populate a 3D scene.
#
#  Usage:
#    1. Open a terminal and run this script
#    2. In a second terminal: flint serve --watch demo/tavern/levels/tavern.scene.toml --schemas demo/tavern/schemas
#    3. Watch the scene build itself in the viewer
#
# ============================================================

$ErrorActionPreference = "Stop"

Write-Host ""
Write-Host "=== Flint Engine Showcase ===" -ForegroundColor Cyan
Write-Host "    The Rusty Flint Tavern" -ForegroundColor Cyan
Write-Host ""

# Clean up any previous run
if (Test-Path "demo/tavern") {
    Remove-Item -Recurse -Force "demo/tavern"
}

# Step 1: Initialize project
Write-Host "[1/8] Initializing project..." -ForegroundColor Yellow
flint init demo/tavern
Write-Host ""

$scene = "demo/tavern/levels/tavern.scene.toml"
$schemas = "demo/tavern/schemas"

# Create the scene file
flint scene create $scene --name "The Rusty Flint Tavern"

Write-Host ""
Write-Host ">>> TIP: In another terminal, run:" -ForegroundColor Magenta
Write-Host "    flint serve --watch $scene --schemas $schemas" -ForegroundColor White
Write-Host "    to watch the tavern build itself in real-time!" -ForegroundColor Magenta
Write-Host ""
Read-Host "Press Enter to start building"

# Step 2: Build the rooms
Write-Host "[2/8] Building rooms..." -ForegroundColor Yellow

flint entity create --archetype room --name "main_hall" --scene $scene --schemas $schemas `
    --props '{"transform":{"position":[0,0,0]},"bounds":{"min":[-7,0,-5],"max":[7,4,5]}}'
Write-Host "  + Main hall (14m x 4m x 10m)" -ForegroundColor Green
Start-Sleep -Seconds 1

flint entity create --archetype room --name "kitchen" --parent "main_hall" --scene $scene --schemas $schemas `
    --props '{"transform":{"position":[0,0,-9]},"bounds":{"min":[-4,0,-3],"max":[4,3.5,3]}}'
Write-Host "  + Kitchen (8m x 3.5m x 6m)" -ForegroundColor Green
Start-Sleep -Seconds 1

flint entity create --archetype room --name "storage" --parent "main_hall" --scene $scene --schemas $schemas `
    --props '{"transform":{"position":[11,0,0]},"bounds":{"min":[-3,0,-3],"max":[3,3,3]}}'
Write-Host "  + Storage room (6m x 3m x 6m)" -ForegroundColor Green
Start-Sleep -Seconds 1

flint entity create --archetype room --name "balcony" --parent "main_hall" --scene $scene --schemas $schemas `
    --props '{"transform":{"position":[0,4,0]},"bounds":{"min":[-7,0,-5],"max":[7,3,-2]}}'
Write-Host "  + Balcony overlooking main hall" -ForegroundColor Green
Start-Sleep -Seconds 1

Write-Host ""

# Step 3: Place doors
Write-Host "[3/8] Placing doors..." -ForegroundColor Yellow

flint entity create --archetype door --name "front_entrance" --parent "main_hall" --scene $scene --schemas $schemas `
    --props '{"transform":{"position":[0,0,5]},"door":{"style":"hinged","locked":false}}'
Write-Host "  + Front entrance (hinged, unlocked)" -ForegroundColor Green
Start-Sleep -Milliseconds 500

flint entity create --archetype door --name "kitchen_door" --parent "main_hall" --scene $scene --schemas $schemas `
    --props '{"transform":{"position":[0,0,-5]},"door":{"style":"sliding","locked":false}}'
Write-Host "  + Kitchen door (sliding, unlocked)" -ForegroundColor Green
Start-Sleep -Milliseconds 500

flint entity create --archetype door --name "storage_door" --parent "main_hall" --scene $scene --schemas $schemas `
    --props '{"transform":{"position":[7,0,0]},"door":{"style":"hinged","locked":true}}'
Write-Host "  + Storage door (hinged, LOCKED)" -ForegroundColor Green
Start-Sleep -Milliseconds 500

flint entity create --archetype door --name "back_exit" --parent "kitchen" --scene $scene --schemas $schemas `
    --props '{"transform":{"position":[0,0,-12]},"door":{"style":"hinged","locked":false}}'
Write-Host "  + Back exit from kitchen" -ForegroundColor Green
Start-Sleep -Milliseconds 500

Write-Host ""

# Step 4: Furnish the main hall
Write-Host "[4/8] Furnishing the main hall..." -ForegroundColor Yellow

flint entity create --archetype furniture --name "bar_counter" --parent "main_hall" --scene $scene --schemas $schemas `
    --props '{"transform":{"position":[-4,0,0]},"bounds":{"min":[-1.5,0,-3],"max":[0,1.2,3]}}'
Write-Host "  + Bar counter" -ForegroundColor Green
Start-Sleep -Milliseconds 300

$tablePositions = @(
    @{name="table_1"; x=2; z=2},
    @{name="table_2"; x=2; z=-2},
    @{name="table_3"; x=5; z=2},
    @{name="table_4"; x=5; z=-2}
)

foreach ($t in $tablePositions) {
    $props = '{"transform":{"position":[' + $t.x + ',0,' + $t.z + ']},"bounds":{"min":[-0.6,0,-0.6],"max":[0.6,0.8,0.6]}}'
    flint entity create --archetype furniture --name $t.name --parent "main_hall" --scene $scene --schemas $schemas --props $props
    Write-Host "  + $($t.name)" -ForegroundColor Green
    Start-Sleep -Milliseconds 300
}

Write-Host ""

# Step 5: Kitchen equipment
Write-Host "[5/8] Equipping the kitchen..." -ForegroundColor Yellow

flint entity create --archetype furniture --name "stove" --parent "kitchen" --scene $scene --schemas $schemas `
    --props '{"transform":{"position":[-2,0,-9]},"bounds":{"min":[-0.5,0,-0.5],"max":[0.5,1.0,0.5]}}'
Write-Host "  + Stove" -ForegroundColor Green
Start-Sleep -Milliseconds 300

flint entity create --archetype furniture --name "prep_table" --parent "kitchen" --scene $scene --schemas $schemas `
    --props '{"transform":{"position":[2,0,-9]},"bounds":{"min":[-1.0,0,-0.5],"max":[1.0,0.9,0.5]}}'
Write-Host "  + Prep table" -ForegroundColor Green
Start-Sleep -Milliseconds 300

Write-Host ""

# Step 6: Storage contents
Write-Host "[6/8] Stocking the storage room..." -ForegroundColor Yellow

flint entity create --archetype furniture --name "crate_stack_1" --parent "storage" --scene $scene --schemas $schemas `
    --props '{"transform":{"position":[10,0,-2]},"bounds":{"min":[-0.5,0,-0.5],"max":[0.5,1.5,0.5]}}'
Write-Host "  + Crate stack" -ForegroundColor Green

flint entity create --archetype furniture --name "crate_stack_2" --parent "storage" --scene $scene --schemas $schemas `
    --props '{"transform":{"position":[12,0,-2]},"bounds":{"min":[-0.5,0,-0.5],"max":[0.5,2.0,0.5]}}'
Write-Host "  + Tall crate stack" -ForegroundColor Green

flint entity create --archetype furniture --name "barrel" --parent "storage" --scene $scene --schemas $schemas `
    --props '{"transform":{"position":[11,0,2]},"bounds":{"min":[-0.4,0,-0.4],"max":[0.4,1.0,0.4]}}'
Write-Host "  + Barrel" -ForegroundColor Green

Write-Host ""

# Step 7: Balcony
Write-Host "[7/8] Adding balcony railing..." -ForegroundColor Yellow

flint entity create --archetype furniture --name "balcony_railing" --parent "balcony" --scene $scene --schemas $schemas `
    --props '{"transform":{"position":[0,4,-2]},"bounds":{"min":[-6.5,0,-0.1],"max":[6.5,1.0,0.1]}}'
Write-Host "  + Balcony railing" -ForegroundColor Green

Write-Host ""

# Step 8: Characters
Write-Host "[8/8] Populating with characters..." -ForegroundColor Yellow

flint entity create --archetype character --name "bartender" --parent "main_hall" --scene $scene --schemas $schemas `
    --props '{"transform":{"position":[-5,0,0]}}'
Write-Host "  + Bartender (behind the bar)" -ForegroundColor Green
Start-Sleep -Milliseconds 300

flint entity create --archetype character --name "patron_1" --parent "main_hall" --scene $scene --schemas $schemas `
    --props '{"transform":{"position":[2,0,2.8]}}'
Write-Host "  + Patron at table 1" -ForegroundColor Green
Start-Sleep -Milliseconds 300

flint entity create --archetype character --name "patron_2" --parent "main_hall" --scene $scene --schemas $schemas `
    --props '{"transform":{"position":[5,0,2.8]}}'
Write-Host "  + Patron at table 3" -ForegroundColor Green
Start-Sleep -Milliseconds 300

flint entity create --archetype character --name "patron_3" --parent "main_hall" --scene $scene --schemas $schemas `
    --props '{"transform":{"position":[5,0,-2.8]}}'
Write-Host "  + Patron at table 4" -ForegroundColor Green
Start-Sleep -Milliseconds 300

flint entity create --archetype character --name "cook" --parent "kitchen" --scene $scene --schemas $schemas `
    --props '{"transform":{"position":[-2,0,-8]}}'
Write-Host "  + Cook in the kitchen" -ForegroundColor Green
Start-Sleep -Milliseconds 300

flint entity create --archetype character --name "mysterious_stranger" --parent "main_hall" --scene $scene --schemas $schemas `
    --props '{"transform":{"position":[2,0,-2.8]}}'
Write-Host "  + A mysterious stranger at table 2..." -ForegroundColor Green
Start-Sleep -Milliseconds 300

Write-Host ""
Write-Host "============================================" -ForegroundColor Cyan
Write-Host "  Tavern complete! Let's explore it." -ForegroundColor Cyan
Write-Host "============================================" -ForegroundColor Cyan
Write-Host ""

# Now demonstrate queries
Write-Host "--- Query: All entities ---" -ForegroundColor Yellow
flint query "entities" --scene $scene
Write-Host ""

Write-Host "--- Query: Just the doors ---" -ForegroundColor Yellow
flint query "entities where archetype == 'door'" --scene $scene
Write-Host ""

Write-Host "--- Query: Locked doors ---" -ForegroundColor Yellow
flint query "entities where door.locked == true" --scene $scene
Write-Host ""

Write-Host "--- Query: Characters ---" -ForegroundColor Yellow
flint query "entities where archetype == 'character'" --scene $scene
Write-Host ""

Write-Host "--- Schema: door component ---" -ForegroundColor Yellow
flint schema door --schemas $schemas
Write-Host ""

Write-Host "--- Scene info ---" -ForegroundColor Yellow
flint scene info $scene
Write-Host ""

Write-Host "============================================" -ForegroundColor Cyan
Write-Host "  Showcase complete!" -ForegroundColor Cyan
Write-Host ""
Write-Host "  The scene file is human-readable TOML:" -ForegroundColor White
Write-Host "    $scene" -ForegroundColor White
Write-Host ""
Write-Host "  Camera controls in the viewer:" -ForegroundColor White
Write-Host "    Left-drag   = orbit" -ForegroundColor Gray
Write-Host "    Right-drag  = pan" -ForegroundColor Gray
Write-Host "    Scroll      = zoom" -ForegroundColor Gray
Write-Host "    Space       = reset camera" -ForegroundColor Gray
Write-Host "    R           = force reload" -ForegroundColor Gray
Write-Host "    Escape      = quit" -ForegroundColor Gray
Write-Host "============================================" -ForegroundColor Cyan
