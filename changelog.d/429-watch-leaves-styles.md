### `flint edit --watch` no longer reloads the scene for a style's WGSL (#429)
**What changed.** Saving a user style's `.wgsl` or `.shader.toml` in a
`styles/` folder the scene searches no longer re-parses and rebuilds the
whole scene in the viewer. The shader rebuilds in place, as before, and the
world (selection, camera, skinned meshes, unsaved entity edits) is left
alone. A changed `.shader.toml` still reloads the scene once, a few times a
second at most, because the scene's `[post_process.styles.<name>]` has to be
read again against its new parameters. Any other file under the scene's
folder, a `.wgsl` outside `styles/` included, reloads the scene as it did.
**What to do.** Nothing.
