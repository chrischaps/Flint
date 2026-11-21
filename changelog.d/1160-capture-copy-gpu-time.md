### A frame capture says what its copies cost on the GPU: `capture.gpu_copy_ms` (#1160)
**What changed.** A capture's `manifest.json` has `capture.gpu_copy_ms` (every copy together) and `capture.gpu_copies_timed`, and each image `gpu_copy_ms`, timed on the capture's own timestamp queries, so the frame's GPU timer still times every pass. `flint render --frame-capture` and the player's `wrote ...` line end with `copied in N ms on the GPU`. On an adapter without timestamp queries the fields are `null` and the line says nothing more. Older captures read as before.
**What to do.** Nothing.
