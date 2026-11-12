//! What the declared API registered (ADR 0116): every function the
//! builder declared, with its module, parameters, return and doc, and the
//! engine's callbacks. `flint script api` lists it.
//!
//! Native builds only: the web build keeps the registrations and drops the
//! doc strings, so there [`ApiRegistry`] is empty and holds nothing.

/// Every function declared through the builder, in declaration order.
#[derive(Debug, Default, Clone)]
pub struct ApiRegistry {
    #[cfg(not(target_arch = "wasm32"))]
    entries: Vec<ApiEntry>,
    /// The modules an optional feature owns, and when each is enabled
    /// (#698), in declaration order.
    #[cfg(not(target_arch = "wasm32"))]
    gates: Vec<crate::module::Gate>,
}

impl ApiRegistry {
    #[cfg_attr(target_arch = "wasm32", allow(unused_variables))]
    pub(crate) fn add_gate(&mut self, gate: crate::module::Gate) {
        #[cfg(not(target_arch = "wasm32"))]
        if !self.gates.contains(&gate) {
            self.gates.push(gate);
        }
    }
}

#[cfg(not(target_arch = "wasm32"))]
pub use native::*;

#[cfg(not(target_arch = "wasm32"))]
mod native {
    use super::ApiRegistry;

    /// One declared function (one overload: a name declared at two arities
    /// is two entries).
    #[derive(Debug, Clone, PartialEq)]
    pub struct ApiEntry {
        pub module: &'static str,
        pub name: &'static str,
        pub params: Vec<ApiParam>,
        /// The return's kind and its shape in words; `None` returns `()`.
        pub returns: Option<(&'static str, &'static str)>,
        /// One line.
        pub doc: &'static str,
        /// Registered on Rhai's typed path (`run_typed`): its kinds are
        /// exact types and convert nothing.
        pub typed: bool,
        /// Kept for a release and going (ADR 0116 decision 3): what to use
        /// instead. `flint script api` marks it and `flint script check`
        /// warns at each call (#694).
        pub deprecated: Option<&'static str>,
    }

    /// A parameter: its name and the kind it accepts (`num`, `entity`, ...).
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub struct ApiParam {
        pub name: &'static str,
        pub kind: &'static str,
    }

    impl ApiEntry {
        /// `set_position(id: entity, x: num, y: num, z: num)`, with ` -> map`
        /// when it returns something.
        pub fn signature(&self) -> String {
            let params: Vec<String> = self
                .params
                .iter()
                .map(|p| format!("{}: {}", p.name, p.kind))
                .collect();
            let ret = self
                .returns
                .map(|(kind, _)| format!(" -> {kind}"))
                .unwrap_or_default();
            format!("{}({}){ret}", self.name, params.join(", "))
        }
    }

    impl ApiRegistry {
        pub(crate) fn push(&mut self, entry: ApiEntry) {
            self.entries.push(entry);
        }

        /// Every entry, in declaration order.
        pub fn entries(&self) -> &[ApiEntry] {
            &self.entries
        }

        /// The modules an optional feature owns (#698): each one's name and
        /// when it is enabled. A module not here is core: always enabled.
        pub fn gates(&self) -> &[crate::module::Gate] {
            &self.gates
        }

        /// When `module` is enabled, if an optional feature owns it.
        pub fn enabled_when(&self, module: &str) -> Option<&'static str> {
            self.gates
                .iter()
                .find(|g| g.module == module)
                .map(|g| g.enabled_when)
        }

        /// Whether `name` is declared at `arity`.
        pub fn declares(&self, name: &str, arity: usize) -> bool {
            self.entries
                .iter()
                .any(|e| e.name == name && e.params.len() == arity)
        }
    }

    /// The rules every declared function follows, by name, as `flint
    /// script api` states them above the functions (#696, #701).
    pub const CONVENTIONS: &[(&str, &str)] = &[
        (
            "numbers",
            "A num parameter takes an int or a float; an int parameter takes an int or a \
             float with nothing after the point. A function marked [typed] takes exactly \
             the types listed.",
        ),
        (
            "missing_entity",
            "An entity parameter given -1 (what get_entity returns for a name it can't \
             find) or a despawned id skips the call and warns once per script and \
             function. A setter or command then does nothing and queues nothing; a getter \
             returns the empty value of what it returns ((), #{}, [], false, 0, \"\") \
             unless its returns line names another: -1 for an id, -1.0 for a time, the \
             largest float for a distance. entity_exists(id) never warns: ask it where \
             an entity may be missing.",
        ),
        (
            "nothing_found",
            "A query of a live entity or the world that finds nothing (no hit, no such \
             component or body, no physics) returns () where the answer would be a map \
             or a value, -1 where it would be an id and -1.0 where it would be a time; \
             each function's returns line says which. The entity module's transform \
             getters return #{} for an entity without a transform.",
        ),
        (
            "names",
            "A name puts its verb first, then what it acts on: play_sprite, \
             set_sprite_speed, shake_camera, stop_camera_shake. A value a script can \
             change is read with get_<noun> and changed with set_<noun>, the same noun \
             (get_lod_scale and set_lod_scale, get_current_scene); a yes-or-no question \
             starts is_ or has_ (is_sprite_playing). What no script sets is named for what \
             it returns, with no get_: this frame's input, the clocks and the screen \
             (mouse_x, touch_x, delta_time, screen_width) and a calculation \
             (distance, raycast, measure_text). A family one module owns keeps its prefix \
             before the verb (persist_, ui_, spline_, conducted_). One name a function: no \
             aliases and no _ex variants. A renamed function's old name works for one \
             release, listed as deprecated with its new name, and flint script check warns \
             at each call.",
        ),
        (
            "opts_maps",
            "A draw call takes what places it as positional numbers (a position and a \
             size, two points, a centre and a radius) and then either its colour as four \
             numbers (r, g, b, a, 0..1) or an opts map holding everything else, every key \
             optional: draw_rect(x, y, w, h, #{color: \"#ff8800\", rounding: 6, layer: 2}). \
             In a map a colour is [r, g, b], [r, g, b, a] or \"#rrggbb(aa)\", white when \
             left out; layer orders draws (higher on top, 0); thickness is in points (1). \
             Each function's description lists its keys; a key it doesn't take, or a \
             value of the wrong shape, is ignored with a warning once per function and \
             key. A map costs a call 0.15 to 0.22 us more than four colour numbers \
             (draw_rect: 315 ns positional, 534 ns with a three-key map), so a loop of \
             hundreds of draws a frame that needs only a colour keeps the positional form.",
        ),
        (
            "modules",
            "Most modules are core: every scene can call them. A module an optional \
             feature owns (living, wooze, debug, conducted) is declared by that feature \
             and enabled where its subsystem runs: living, wooze and debug in a scene with \
             living props, a [living] or [wooze] table, shallow wooze or woozy grass; \
             conducted in a scene whose music_session has a chart. Each such module's \
             heading says when it is enabled. Calling one of its functions anywhere else \
             is a script error saying the module is not enabled, at the call's line.",
        ),
    ];

    /// A callback the engine calls on a script that defines it.
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub struct CallbackDoc {
        pub name: &'static str,
        pub params: &'static [&'static str],
        pub doc: &'static str,
    }

    /// The callbacks, as the book's scripting page describes them.
    pub const CALLBACKS: &[CallbackDoc] = &[
        cb("on_init", &[], "Once, when the script is first loaded."),
        cb(
            "on_update",
            &[],
            "Every frame the game isn't paused; delta_time() is the frame's step.",
        ),
        cb(
            "on_draw_ui",
            &[],
            "Every frame after on_update, paused or not: the HUD's draw calls.",
        ),
        cb(
            "on_collision",
            &["other_id"],
            "This entity collided with another.",
        ),
        cb(
            "on_collision_exit",
            &["other_id"],
            "A contact with another entity ended.",
        ),
        cb(
            "on_trigger_enter",
            &["other_id"],
            "Another entity entered this trigger volume.",
        ),
        cb(
            "on_trigger_exit",
            &["other_id"],
            "Another entity left this trigger volume.",
        ),
        cb(
            "on_action",
            &["action_name"],
            "An input action fired (\"jump\", \"interact\").",
        ),
        cb(
            "on_interact",
            &[],
            "The player pressed Interact within this entity's interactable range.",
        ),
        cb("on_scene_exit", &[], "The scene is about to be left."),
        cb("on_scene_enter", &[], "A scene was entered."),
        cb(
            "on_animation_end",
            &["clip"],
            "A once sprite clip finished.",
        ),
        cb(
            "on_sequence_cue",
            &["sequence", "cue"],
            "An animation sequence passed a cue event.",
        ),
        cb(
            "on_event",
            &["name", "data"],
            "A game event fired by a script, the engine or a player extension.",
        ),
        cb(
            "on_expression",
            &["state"],
            "A new state arrived from outside the process (a timeline or a socket).",
        ),
        cb(
            "on_persist_migrate",
            &["from"],
            "Save data loaded at a version below save_version; runs before on_init.",
        ),
    ];

    const fn cb(
        name: &'static str,
        params: &'static [&'static str],
        doc: &'static str,
    ) -> CallbackDoc {
        CallbackDoc { name, params, doc }
    }

    /// A function Rhai has registered that the builder didn't declare: an
    /// extension's raw registration (every engine function is declared).
    /// From Rhai's metadata, so types but no parameter names or docs.
    #[derive(Debug, Clone, PartialEq, Eq)]
    pub struct UndeclaredFn {
        pub name: String,
        /// The parameters' Rust types, as Rhai names them.
        pub params: Vec<String>,
        pub returns: String,
        /// Rhai's signature line.
        pub signature: String,
    }

    /// The whole script API: what the builder declared, what it didn't,
    /// and the callbacks. `ScriptSystem::api_listing` makes one.
    #[derive(Debug, Clone)]
    pub struct ApiListing {
        pub declared: Vec<ApiEntry>,
        pub undeclared: Vec<UndeclaredFn>,
        /// Whether `undeclared` is from Rhai's metadata: false when
        /// flint-script was built without its `metadata` feature, and the
        /// undeclared part is then unknown, not empty.
        pub undeclared_listed: bool,
        pub callbacks: Vec<CallbackDoc>,
        /// The modules an optional feature owns and when each is enabled
        /// (#698); every other module is core.
        pub gates: Vec<crate::module::Gate>,
    }

    impl ApiListing {
        /// When `module` is enabled, if an optional feature owns it.
        pub fn enabled_when(&self, module: &str) -> Option<&'static str> {
            self.gates
                .iter()
                .find(|g| g.module == module)
                .map(|g| g.enabled_when)
        }
    }

    /// Parse one of Rhai's signature lines, `name(a: T, b: U) -> R`, or
    /// `name(T, U) -> R` for a closure registered without parameter names.
    pub fn parse_signature(sig: &str) -> Option<UndeclaredFn> {
        let open = sig.find('(')?;
        let name = sig[..open].trim();
        // The parameter list's own `)`, not one in a `-> ()` after it
        let mut depth = 0i32;
        let close = sig[open..].char_indices().find_map(|(i, ch)| {
            match ch {
                '(' => depth += 1,
                ')' => {
                    depth -= 1;
                    if depth == 0 {
                        return Some(open + i);
                    }
                }
                _ => {}
            }
            None
        })?;
        let inner = &sig[open + 1..close];
        let returns = sig[close + 1..]
            .trim()
            .strip_prefix("->")
            .map(|r| r.trim().to_string())
            .unwrap_or_else(|| "()".to_string());
        let mut params = Vec::new();
        let mut depth = 0i32;
        let mut start = 0;
        for (i, ch) in inner.char_indices() {
            match ch {
                '<' | '(' | '[' => depth += 1,
                '>' | ')' | ']' => depth -= 1,
                ',' if depth == 0 => {
                    params.push(inner[start..i].trim().to_string());
                    start = i + 1;
                }
                _ => {}
            }
        }
        if !inner.trim().is_empty() {
            params.push(inner[start..].trim().to_string());
        }
        let params = params
            .into_iter()
            .map(|p| match p.split_once(':') {
                Some((_, ty)) => ty.trim().to_string(),
                None => p,
            })
            .collect();
        Some(UndeclaredFn {
            name: name.to_string(),
            params,
            returns,
            signature: sig.to_string(),
        })
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn rhai_signatures_parse() {
            let f = parse_signature("draw_rect(x: f64, y: f64, w: f64) -> ()").unwrap();
            assert_eq!(f.name, "draw_rect");
            assert_eq!(f.params, ["f64", "f64", "f64"]);
            assert_eq!(f.returns, "()");
            let f = parse_signature("get_entity(_: &str) -> i64").unwrap();
            assert_eq!(f.params, ["&str"]);
            let f = parse_signature("mouse_x() -> f64").unwrap();
            assert!(f.params.is_empty());
            let f = parse_signature("f(a: Result<i64, Box<E>>, b: Map)").unwrap();
            assert_eq!(f.params.len(), 2);
            assert_eq!(f.returns, "()");
        }
    }
}
