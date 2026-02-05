# Headless Rendering

> This page is a stub. Content coming soon.

A guide to rendering scenes without a window, for CI pipelines and automated screenshots. This guide will cover:

- The `flint render` command and its options
- Setting camera position, angle, and field of view
- Output resolution and format
- Using headless rendering in CI/CD pipelines
- Visual regression testing with baseline comparisons
- Disabling the ground grid for clean screenshots

Quick example:

```bash
flint render levels/tavern.scene.toml \
    --output preview.png \
    --width 1920 --height 1080 \
    --distance 30 --yaw 45 --pitch 30
```
