# Assets

> This page is a stub. Content coming soon.

Flint uses a content-addressed asset system with SHA-256 hashing. This page will cover:

- Content addressing: how files are identified by hash
- Storage layout: `.flint/assets/<prefix>/<hash>.<ext>`
- Asset sidecar files (`.asset.toml`) for metadata
- Importing assets: `flint asset import`
- Browsing the catalog: `flint asset list`, `flint asset info`
- Resolution strategies: `strict`, `placeholder`
- glTF/GLB import with mesh, material, and texture extraction
- Deduplication and change detection

See the [Importing Assets](../guides/importing-assets.md) guide for practical examples.

Quick start:

```bash
flint asset import models/chair.glb --name tavern_chair --tags furniture,medieval
flint asset list --type mesh
flint asset resolve levels/tavern.scene.toml --strategy strict
```
