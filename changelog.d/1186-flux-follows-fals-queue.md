### `flint asset generate texture` follows fal's queue (#1186)
**What changed.** Flux's default endpoint, `queue.fal.run`, answers a POST
with a receipt, not an image. Flint read the image straight from that answer,
so every generate failed with `Unexpected Flux response format` after fal had
queued the request (and billed it). Now Flint polls the request's
`status_url`, fetches the output from its `response_url` and then downloads
the image. A queued texture is a job in `.flint/jobs/`, as a Meshy model is,
so `flint asset job resume <id>` finishes one whose run stopped without
paying again. An `api_url` pointing at the synchronous `fal.run` still works,
and makes no job.
**What to do.** Nothing. Jobs from earlier runs read as before.
