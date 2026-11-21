### `[post_process] preload_looks` and `preload_styles`: a scene pays for its later looks at its entry (#1159)
**What changed.** A scene can name the looks and styles it will show later, and the player builds them when the scene loads, on worker threads, so the frame a script cuts or fades to one builds nothing:

```toml
[post_process]
preload_looks = ["graphite", "ink_wash", "watercolor"]   # for set_look
preload_styles = ["matrix", "tron"]                      # for set_post_style
```

`prepare_look` (#1121) built a look's styles; it now builds the blend, reveal and tear draws they ask for too, which a frame used to build the first time a style's `reveal` lay between 0 and 1 (92 ms in one film) or it tore in. A preloaded look gets every one of those draws, since a fade blends it in; a preloaded style gets every one too, whatever its table says at load. A scene's own styles get their draws built at entry with no key at all, so a sheet waiting at `reveal = 0` tears open without a stop. A preloaded look is read from its file once, at load, so the cut to it asks the disk nothing; and every `set_look` now uses the project found at the scene's entry instead of looking for it again, which on a busy disk had stalled cuts by 130-170 ms. So a look file edited while a scene plays shows at the scene's next load if the scene preloads it. A look the search can't find or a style nobody registered is warned about at load and skipped. Rust hosts: `SceneRenderer::prepare_styles_with(device, &config, &names)`.
**What to do.** Nothing, unless a scene switches looks or styles mid-play: list them under `preload_looks` and `preload_styles`.
