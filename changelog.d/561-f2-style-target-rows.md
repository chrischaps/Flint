### The F2 overlay lists each style target's memory (#561)
**What changed.** Under its `Style targets: N (X MB, K full-size)` total, the player's F2 overlay now has a row for each texture the style chain holds: its name, size and MB, as `flint render --gpu-time` lists them. A user style's own targets show as `<style>.<target>` (and `.a`/`.b` for a history pair) beside the built-ins', so a `[[passes]]` style's cost can be read while it runs. Past 16 textures, the rest share one `+N more` row. Nothing is read while the overlay is closed or showing the graph alone.
**What to do.** Nothing.
