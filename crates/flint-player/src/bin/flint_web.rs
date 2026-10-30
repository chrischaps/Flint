//! Flint in a browser (#94; async startup #99, bundles #98, docs/design/web.md).
//!
//! The web twin of `main.rs`. Arguments come from the page's query string
//! instead of clap, and the player starts with winit's `spawn_app` instead of
//! the blocking `run_app`. `PlayerApp` then asks for its GPU device without
//! blocking (`player_app/web_startup.rs`) and finishes starting up when it
//! lands.
//!
//! A game arrives as a bundle (`flint bundle`, `flint_core::bundle`): the
//! page fetches it with a progress bar, installs its files as the vfs, and
//! loads the scene the manifest names, exactly as the desktop reads a game
//! folder. Its late bundles (audio) are fetched once the player is handed to
//! the browser; when one lands the vfs grows and the player loads the sounds
//! it couldn't find at start.
//!
//! Query string (all optional):
//!   bundle=<url>    a .flintpack, relative to the page
//!   scene=<path>    with bundle: a scene inside it instead of the manifest's;
//!                   without: a scene TOML fetched from <url> (default: the
//!                   first-light demo, which needs no other files)
//!   msaa=1|4        scene-pass MSAA (ADR 0058)
//!   width=, height= fix the canvas to this many CSS pixels instead of the page
//!   frame_step=<s>  step game time exactly s per frame (`--frame-step`)
//!   hold_at=<s>     stop game time at s seconds, for a screenshot of an
//!                   exact moment (with frame_step; `PlayerApp::hold_at`)
//!
//! Build and serve from `crates/flint-player`:
//!   trunk serve            # http://127.0.0.1:8080
//!   trunk build --release  # dist/, wasm-opt -Oz applied

#[cfg(not(target_arch = "wasm32"))]
fn main() {
    eprintln!("flint-web is the browser entry; run `trunk serve` in crates/flint-player");
}

#[cfg(target_arch = "wasm32")]
fn main() {
    web::run();
}

#[cfg(target_arch = "wasm32")]
mod web {
    use flint_core::bundle::{self, Bundle, LateBundle};
    use flint_core::vfs::{self, MemorySource};
    use flint_ecs::FlintWorld;
    use flint_player::{PlayerApp, WEB_CANVAS_ID};
    use flint_scene::SceneFile;
    use flint_schema::SchemaRegistry;
    use std::sync::Arc;
    use wasm_bindgen::JsCast;
    use wasm_bindgen_futures::JsFuture;
    use winit::event_loop::{ControlFlow, EventLoop};
    use winit::platform::web::EventLoopExtWebSys;

    mod engine_schemas {
        include!(concat!(env!("OUT_DIR"), "/engine_schemas.rs"));
    }

    /// The scene a bare page plays, copied next to it by trunk (index.html).
    const DEFAULT_SCENE: &str = "scenes/web_first_light.scene.toml";

    /// What `main.rs` takes from clap, read from the query string.
    struct WebArgs {
        bundle: Option<String>,
        scene: Option<String>,
        msaa: u32,
        size: Option<(u32, u32)>,
        frame_step: Option<f64>,
        hold_at: Option<f64>,
    }

    impl WebArgs {
        fn from_query() -> Self {
            let params = web_sys::window()
                .and_then(|w| w.location().search().ok())
                .and_then(|q| web_sys::UrlSearchParams::new_with_str(&q).ok());
            let get = |key: &str| params.as_ref().and_then(|p| p.get(key));
            let num = |key: &str| get(key).and_then(|v| v.parse::<u32>().ok());
            let secs = |key: &str| {
                get(key)
                    .and_then(|v| v.parse::<f64>().ok())
                    .filter(|s| s.is_finite() && *s >= 0.0)
            };
            Self {
                bundle: get("bundle").filter(|b| !b.is_empty()),
                scene: get("scene").filter(|s| !s.is_empty()),
                msaa: num("msaa").filter(|&m| m == 1 || m == 4).unwrap_or(1),
                size: num("width").zip(num("height")),
                frame_step: secs("frame_step").filter(|s| *s > 0.0),
                hold_at: secs("hold_at"),
            }
        }
    }

    pub fn run() {
        console_error_panic_hook::set_once();
        tracing_wasm::set_as_global_default_with_config(
            tracing_wasm::WASMLayerConfigBuilder::new()
                .set_max_level(tracing::Level::INFO)
                .build(),
        );
        wasm_bindgen_futures::spawn_local(async {
            if let Err(e) = start().await {
                status(&format!("couldn't start: {e}"));
                web_sys::console::error_1(&format!("[flint-web] {e}").into());
            }
        });
    }

    /// A scene ready to play: the world, its file, its path and the schema
    /// directories transitions reload from.
    struct Loaded {
        world: FlintWorld,
        scene_file: SceneFile,
        scene_path: String,
        schemas: Vec<String>,
    }

    async fn start() -> Result<(), String> {
        let args = WebArgs::from_query();
        if let Some((w, h)) = args.size {
            fix_canvas_size(w, h);
        }

        let mut late = Vec::new();
        let loaded = match &args.bundle {
            Some(url) => {
                let (loaded, source, parts) = load_bundle(url, args.scene.as_deref()).await?;
                late.push((source, parts, absolute_url(url)?));
                loaded
            }
            None => load_scene_url(args.scene.as_deref().unwrap_or(DEFAULT_SCENE)).await?,
        };
        tracing::info!(
            "[flint-web] scene {:?}: {} entities",
            loaded.scene_file.scene.name,
            loaded.world.entity_count()
        );
        let title = loaded.scene_file.scene.name.clone();
        let line = loaded.scene_file.scene.description.clone();
        status(&title);

        let event_loop = EventLoop::new().map_err(|e| format!("event loop: {e}"))?;
        event_loop.set_control_flow(ControlFlow::Poll);
        let mut app = PlayerApp::new(
            loaded.world,
            loaded.scene_path,
            false,
            None,
            loaded.scene_file.scene.input_config.clone(),
        );
        app.msaa_sample_count = args.msaa;
        app.clock.frame_step = args.frame_step;
        app.hold_at = args.hold_at;
        app.apply_scene_settings(&loaded.scene_file);
        app.set_schema_paths(loaded.schemas);
        app.set_event_proxy(event_loop.create_proxy());
        // Returns at once: the browser drives the loop from here on.
        event_loop.spawn_app(app);
        open_gate(&title, line.as_deref().unwrap_or(""));

        for (source, parts, base) in late {
            fetch_late_bundles(source, parts, base, title.clone());
        }
        Ok(())
    }

    /// Fetch the main bundle, make it the file system, and load its scene.
    /// Returns the scene, the installed files (late bundles add to them) and
    /// the late bundles still to fetch.
    async fn load_bundle(
        url: &str,
        scene_override: Option<&str>,
    ) -> Result<(Loaded, MemorySource, Vec<LateBundle>), String> {
        let bytes = fetch_bytes(url, |got, total| {
            progress(url_name(url), got, total);
        })
        .await?;
        let bundle = Bundle::parse(bytes).map_err(|e| format!("{url}: {e}"))?;
        let manifest = bundle.manifest.clone();
        tracing::info!(
            "[flint-web] bundle {}: {} files, {:.2} MB",
            manifest.name,
            manifest.files.len(),
            bundle.len() as f64 / 1e6
        );
        let source = bundle.to_memory_source();
        drop(bundle);
        vfs::install(Arc::new(source.clone()));

        let scene_path = scene_override
            .map(str::to_string)
            .or(manifest.scene.clone())
            .ok_or_else(|| format!("{url} names no scene; add ?scene="))?;

        // The engine's schemas are compiled in; the bundle's own (its engine
        // subtree's, then the game's) load over them, as `--schemas` does.
        let mut registry = engine_registry()?;
        let mut schemas = manifest.schemas.clone();
        for dir in flint_schema::discover_schema_dirs(&scene_path) {
            let dir = vfs::normalize(&dir);
            if !schemas.contains(&dir) {
                schemas.push(dir);
            }
        }
        for dir in &schemas {
            if vfs::is_dir(dir) {
                registry
                    .load_directory(dir)
                    .map_err(|e| format!("schemas {dir}: {e}"))?;
            }
        }

        status(&format!("loading {scene_path}"));
        let (world, scene_file) = flint_scene::load_scene(&scene_path, &registry)
            .map_err(|e| format!("{scene_path}: {e}"))?;
        Ok((
            Loaded {
                world,
                scene_file,
                scene_path,
                schemas,
            },
            source,
            manifest.late,
        ))
    }

    /// No bundle: one scene TOML by URL, made of the engine's own geometry.
    async fn load_scene_url(url: &str) -> Result<Loaded, String> {
        let registry = engine_registry()?;
        status(&format!("loading {url}"));
        let text = fetch_text(url).await?;
        let (world, scene_file) =
            flint_scene::load_scene_string(&text, &registry).map_err(|e| format!("{url}: {e}"))?;
        Ok(Loaded {
            world,
            scene_file,
            scene_path: url.to_string(),
            schemas: Vec::new(),
        })
    }

    /// Fetch the late bundles one after another, each growing the vfs when
    /// it lands. The player notices (`vfs::generation`) and loads the audio.
    fn fetch_late_bundles(
        mut source: MemorySource,
        parts: Vec<LateBundle>,
        base: String,
        title: String,
    ) {
        if parts.is_empty() {
            hide_progress();
            return;
        }
        wasm_bindgen_futures::spawn_local(async move {
            for part in parts {
                let url = match web_sys::Url::new_with_base(&part.url, &base) {
                    Ok(u) => u.href(),
                    Err(_) => part.url.clone(),
                };
                let label = part.part.clone();
                let result = fetch_bytes(&url, |got, total| {
                    progress(&label, got, total.or(Some(part.size as usize)));
                })
                .await
                .and_then(|bytes| Bundle::parse(bytes).map_err(|e| e.to_string()));
                match result {
                    Ok(bundle) => {
                        tracing::info!(
                            "[flint-web] late bundle {}: {} files, {:.2} MB",
                            bundle.manifest.part,
                            bundle.manifest.files.len(),
                            bundle.len() as f64 / 1e6
                        );
                        bundle.insert_into(&mut source);
                        vfs::install(Arc::new(source.clone()));
                    }
                    Err(e) => {
                        web_sys::console::warn_1(&format!("[flint-web] {url}: {e}").into());
                    }
                }
            }
            status(&title);
            hide_progress();
        });
    }

    fn engine_registry() -> Result<SchemaRegistry, String> {
        let mut registry = SchemaRegistry::new();
        for toml in engine_schemas::COMPONENTS {
            registry
                .load_component_string(toml)
                .map_err(|e| format!("engine component schema: {e}"))?;
        }
        for toml in engine_schemas::ARCHETYPES {
            registry
                .load_archetype_string(toml)
                .map_err(|e| format!("engine archetype schema: {e}"))?;
        }
        Ok(registry)
    }

    async fn fetch_response(url: &str) -> Result<web_sys::Response, String> {
        let js = |e: wasm_bindgen::JsValue| format!("{url}: {e:?}");
        let window = web_sys::window().ok_or("no window")?;
        let response: web_sys::Response = JsFuture::from(window.fetch_with_str(url))
            .await
            .map_err(js)?
            .dyn_into()
            .map_err(js)?;
        if !response.ok() {
            return Err(format!("{url}: HTTP {}", response.status()));
        }
        Ok(response)
    }

    async fn fetch_text(url: &str) -> Result<String, String> {
        let js = |e: wasm_bindgen::JsValue| format!("{url}: {e:?}");
        let response = fetch_response(url).await?;
        JsFuture::from(response.text().map_err(js)?)
            .await
            .map_err(js)?
            .as_string()
            .ok_or_else(|| format!("{url}: not text"))
    }

    /// Fetch a bundle chunk by chunk, reporting `(bytes so far, whole size)`.
    /// The size comes from the bundle's own header once it has arrived, not
    /// Content-Length, which is the compressed size when the host gzips.
    async fn fetch_bytes(
        url: &str,
        mut report: impl FnMut(usize, Option<usize>),
    ) -> Result<Vec<u8>, String> {
        let js = |e: wasm_bindgen::JsValue| format!("{url}: {e:?}");
        let response = fetch_response(url).await?;
        let body = response.body().ok_or_else(|| format!("{url}: no body"))?;
        let reader: web_sys::ReadableStreamDefaultReader = body.get_reader().unchecked_into();
        let mut bytes = Vec::new();
        let mut total = None;
        report(0, None);
        loop {
            let chunk = JsFuture::from(reader.read()).await.map_err(js)?;
            let done = js_sys::Reflect::get(&chunk, &"done".into())
                .map(|v| v.is_truthy())
                .unwrap_or(true);
            if done {
                break;
            }
            let value: js_sys::Uint8Array = js_sys::Reflect::get(&chunk, &"value".into())
                .map_err(js)?
                .unchecked_into();
            let start = bytes.len();
            bytes.resize(start + value.length() as usize, 0);
            value.copy_to(&mut bytes[start..]);
            if total.is_none() {
                total = bundle::total_len(&bytes).map_err(|e| format!("{url}: {e}"))?;
                if let Some(t) = total {
                    bytes.reserve(t.saturating_sub(bytes.len()));
                }
            }
            report(bytes.len(), total);
        }
        Ok(bytes)
    }

    /// The bundle's URL made absolute, so late bundles resolve beside it.
    fn absolute_url(url: &str) -> Result<String, String> {
        let page = web_sys::window()
            .and_then(|w| w.location().href().ok())
            .ok_or("no page URL")?;
        web_sys::Url::new_with_base(url, &page)
            .map(|u| u.href())
            .map_err(|e| format!("{url}: {e:?}"))
    }

    fn url_name(url: &str) -> &str {
        url.rsplit('/').next().unwrap_or(url)
    }

    fn element(id: &str) -> Option<web_sys::Element> {
        web_sys::window()?.document()?.get_element_by_id(id)
    }

    fn fix_canvas_size(width: u32, height: u32) {
        if let Some(canvas) = element(WEB_CANVAS_ID) {
            let style = format!("width: {width}px !important; height: {height}px !important;");
            let _ = canvas.set_attribute("style", &style);
        }
    }

    /// The small line under the wordmark: what's loading, or what failed.
    fn status(text: &str) {
        if let Some(el) = element("status") {
            el.set_text_content(Some(text));
        }
    }

    /// The bar under the wordmark and its line: "float.flintpack 1.8 / 3.7 MB".
    fn progress(label: &str, got: usize, total: Option<usize>) {
        let mb = |b: usize| b as f64 / 1e6;
        let text = match total {
            Some(t) => format!("{label}  {:.1} / {:.1} MB", mb(got), mb(t)),
            None => format!("{label}  {:.1} MB", mb(got)),
        };
        status(&text);
        if let Some(bar) = element("progress") {
            let _ = bar.set_attribute("data-active", "");
            let fraction = total.map(|t| got as f64 / t.max(1) as f64).unwrap_or(0.0);
            let _ = bar.set_attribute("style", &format!("--p: {:.4}", fraction.min(1.0)));
        }
    }

    /// The Begin gate (#102). The world is already running behind it,
    /// silent. Its click is the gesture a browser asks for: it locks the
    /// pointer and starts the sound (`flint_player::web_begin`), and the
    /// veil lifts. Afterwards a line under the world says how to take the
    /// pointer back whenever Escape has freed it.
    fn open_gate(title: &str, line: &str) {
        use std::cell::Cell;
        use std::rc::Rc;
        use wasm_bindgen::closure::Closure;

        let (Some(gate), Some(button)) = (element("gate"), element("begin")) else {
            flint_player::web_begin();
            return;
        };
        if let Some(el) = element("gate-title") {
            el.set_text_content(Some(title));
        }
        if let Some(el) = element("gate-line") {
            el.set_text_content(Some(line));
        }
        let _ = gate.set_attribute("data-ready", "");
        if let Ok(button) = button.clone().dyn_into::<web_sys::HtmlElement>() {
            let _ = button.focus();
        }

        let begun = Rc::new(Cell::new(false));
        let on_begin = {
            let begun = begun.clone();
            Closure::<dyn FnMut()>::new(move || {
                if begun.replace(true) {
                    return;
                }
                flint_player::web_begin();
                let _ = gate.set_attribute("data-gone", "");
                if let Some(canvas) = element(flint_player::WEB_CANVAS_ID)
                    .and_then(|c| c.dyn_into::<web_sys::HtmlElement>().ok())
                {
                    let _ = canvas.focus();
                }
            })
        };
        let _ = button.add_event_listener_with_callback("click", on_begin.as_ref().unchecked_ref());
        on_begin.forget();

        let Some(document) = web_sys::window().and_then(|w| w.document()) else {
            return;
        };
        let on_lock = {
            let document = document.clone();
            Closure::<dyn FnMut()>::new(move || {
                let Some(hint) = element("relook") else {
                    return;
                };
                if document.pointer_lock_element().is_some() || !begun.get() {
                    let _ = hint.remove_attribute("data-on");
                } else {
                    let _ = hint.set_attribute("data-on", "");
                }
            })
        };
        let _ = document.add_event_listener_with_callback(
            "pointerlockchange",
            on_lock.as_ref().unchecked_ref(),
        );
        on_lock.forget();
    }

    fn hide_progress() {
        if let Some(bar) = element("progress") {
            let _ = bar.remove_attribute("data-active");
        }
    }
}
