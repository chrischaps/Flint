# #934 spike scripts

Run from the repo root, in this order, on a tree with the version bumps in Cargo.toml:

1. `python -I spike-vendor/scripts/port1.py .` (wgpu renames, 339 -> 31 errors in flint-render)
2. `python -I spike-vendor/scripts/port_egui.py crates/flint-debug-ui crates/flint-viewer crates/flint-player crates/flint-cli` (not flint-script: its `Rounding` is Flint's own)
3. `python -I spike-vendor/scripts/stroke_kind.py crates/flint-player crates/flint-viewer crates/flint-cli crates/flint-debug-ui`
4. `python -I spike-vendor/scripts/port_egui2.py crates/flint-cli crates/flint-player crates/flint-viewer`
5. `python -I spike-vendor/scripts/port_panels.py <files with SidePanel/TopBottomPanel>`

Then the hand edits in the branch's commits. `cmp_renders.py A B` compares two `flint goldens --out-dir` folders; `gputime2.sh` is the GPU-time A/B; `wgslsweep` validates WGSL with naga 23 and 30; `pollrace` is the two-thread DX12 poll test.
