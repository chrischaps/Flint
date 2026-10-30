//! Flint in a browser (#94; async startup #99, docs/design/web.md).
//!
//! The web twin of `main.rs`. Arguments come from the page's query string
//! instead of clap, the scene arrives by `fetch`, and the player starts with
//! winit's `spawn_app` instead of the blocking `run_app`. `PlayerApp` then
//! asks for its GPU device without blocking (`player_app/web_startup.rs`)
//! and finishes starting up when it lands.
//!
//! Query string (all optional):
//!   scene=<url>     scene TOML, relative to the page (default: the demo scene)
//!   msaa=1|4        scene-pass MSAA (ADR 0058)
//!   width=, height= fix the canvas to this many CSS pixels instead of the page
//!
//! The engine schemas are compiled in (`build.rs`). Files the scene points
//! at (models, textures, scripts, audio) still go through `std::fs`, which
//! has no files on the web; they load once the vfs (#97) and game bundles
//! (#98) land. Until then the web player draws scenes made of the engine's
//! own geometry.
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
    use flint_player::{PlayerApp, WEB_CANVAS_ID};
    use flint_schema::SchemaRegistry;
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
        scene: String,
        msaa: u32,
        size: Option<(u32, u32)>,
    }

    impl WebArgs {
        fn from_query() -> Self {
            let params = web_sys::window()
                .and_then(|w| w.location().search().ok())
                .and_then(|q| web_sys::UrlSearchParams::new_with_str(&q).ok());
            let get = |key: &str| params.as_ref().and_then(|p| p.get(key));
            let num = |key: &str| get(key).and_then(|v| v.parse::<u32>().ok());
            Self {
                scene: get("scene").unwrap_or_else(|| DEFAULT_SCENE.to_string()),
                msaa: num("msaa").filter(|&m| m == 1 || m == 4).unwrap_or(1),
                size: num("width").zip(num("height")),
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

    async fn start() -> Result<(), String> {
        let args = WebArgs::from_query();
        if let Some((w, h)) = args.size {
            fix_canvas_size(w, h);
        }

        let registry = engine_registry()?;
        status(&format!("loading {}", args.scene));
        let text = fetch_text(&args.scene).await?;
        let (world, scene_file) = flint_scene::load_scene_string(&text, &registry)
            .map_err(|e| format!("{}: {e}", args.scene))?;
        tracing::info!(
            "[flint-web] scene {:?}: {} entities",
            scene_file.scene.name,
            world.entity_count()
        );
        status(&scene_file.scene.name);

        let event_loop = EventLoop::new().map_err(|e| format!("event loop: {e}"))?;
        event_loop.set_control_flow(ControlFlow::Poll);
        let mut app = PlayerApp::new(
            world,
            args.scene,
            false,
            None,
            scene_file.scene.input_config.clone(),
        );
        app.msaa_sample_count = args.msaa;
        app.apply_scene_settings(&scene_file);
        app.set_event_proxy(event_loop.create_proxy());
        // Returns at once: the browser drives the loop from here on.
        event_loop.spawn_app(app);
        Ok(())
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

    async fn fetch_text(url: &str) -> Result<String, String> {
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
        JsFuture::from(response.text().map_err(js)?)
            .await
            .map_err(js)?
            .as_string()
            .ok_or_else(|| format!("{url}: not text"))
    }

    fn fix_canvas_size(width: u32, height: u32) {
        let canvas = web_sys::window()
            .and_then(|w| w.document())
            .and_then(|d| d.get_element_by_id(WEB_CANVAS_ID));
        if let Some(canvas) = canvas {
            let style = format!("width: {width}px !important; height: {height}px !important;");
            let _ = canvas.set_attribute("style", &style);
        }
    }

    /// The small line under the wordmark: what's loading, or what failed.
    fn status(text: &str) {
        let el = web_sys::window()
            .and_then(|w| w.document())
            .and_then(|d| d.get_element_by_id("status"));
        if let Some(el) = el {
            el.set_text_content(Some(text));
        }
    }
}
