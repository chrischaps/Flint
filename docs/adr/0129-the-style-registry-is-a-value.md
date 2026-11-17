# 0129: The style registry is a value a renderer or host owns

Date: 2026-11-17. Issue: #742 (epic #674). Supersedes the registry half of
[0112](0112-user-authored-styles.md) (its decision 3, "an append-only
snapshot", and its #426 addendum's "the registry stays one for the process").

## Context

Until #742 the style registry was process-wide state. `stylize::registry()`
read a `static SNAPSHOT: RwLock<&'static [&'static StyleDescriptor]>`;
`register_runtime` and `replace_runtime` copied it and `Box::leak`ed the copy
on every call. `custom.rs` leaked every user style's descriptor, its
parameter and option slices, every string in them and its bind-cache names
(with a generation in each, so unique per install) on each load and on each
hot reload of a `.shader.toml`, and kept the loaded styles in `static
LOADED`, their load failures in `static FAILED`, and a game's Rust styles in
`static GAME_STYLES`.

What followed from that:

- Two scenes in one process shared one namespace: a transition, `flint
  goldens`' cases, an A/B split, the viewer and a player, and every test.
  #426's answer (the nearest `styles/` wins a name) had to replace a style in
  place and then hide a style an earlier scene loaded (`drop_unseen`).
- Memory grew with each edit in a long `flint edit` session.
- `StyleSet` told a replaced style by its descriptor's address, and the
  ownership pass's owner table held indices into the process's registry, so
  a result depended on what else the process had loaded.
- The registry's tests raced (#489) and the reload tests took a lock
  (`RELOAD_POLL`) and filtered each poll to their own folder (`under`).

## Decision

1. **`StyleRegistry` is a value** (`stylize/registry.rs`): the built-ins (a
   compiled-in `static` slice of the style modules' `static STYLE`s, as
   before), then each game style in registration order, then each user style
   in load order, plus the user styles' load failures. `Clone` is cheap. It
   has the lookups the free functions had (`find`, `style`, `index_of`,
   `names`, `is_game_style`, `is_native`, `is_user_style`) and the
   frame-wide questions (`any_uses_mask`, `any_uses_albedo`, `owner_table`,
   `foreign_ids`, `stage_chain`). The only free lookups left are the
   built-ins' (`builtins()`, `builtin(name)`, `is_builtin(name)`).
2. **Whoever draws owns one.** `SceneRenderer` holds the registry it draws
   with (`styles()`, `styles_mut()`, `set_styles`). `SceneBinding::new(scope)`
   / `SceneBinding::over(scope, &base)` make a scene's (`base.for_scene`) and
   `bind_scene` hands it to the renderer. The player keeps its own in
   `GameCore::styles` (it exists headless too), registers its extensions'
   styles there, and hands the renderer a copy at each bind and after each
   hot reload. The viewer's renderer owns its. `flint render`, `flint check`
   and `flint style list` make one for the scene in hand.
3. **Every reader is handed the registry.** A scene's `[post_process]` is
   read against one (`post_process_config_from_def(def, &styles)`,
   `StyleSettings::from_tables(tables, &styles)`), and so are `--style`
   words, the script commands (`post_style::*`, `entity_style::*`), the
   style mask's ids, look fades, the catalog and F4 (`StylesEditor`
   mirrors the renderer's). `StyleFrame::styles` and `StyleDevice::style`
   carry it into a pass.
4. **A user style is an `Arc`.** `Style` is a handle: a native style's
   `&'static StyleDescriptor`, or a user style's `Arc<Loaded>`, which holds
   its own descriptor, description and errors. A descriptor's parameter and
   option lists are `List<T>`: a built-in's `'static` slice, or a user
   style's own. A reload replaces the `Arc` in the registry; the old one is
   freed when the last pass built from it is built again.
5. **A style's identity is its name and `generation`**: 0 for a built-in or
   game style, a new number for each user install (a process-wide counter:
   it numbers installs, it holds none). `StyleSet` keys its built passes by
   name and rebuilds one whose generation changed; `StyleFrame::bind` and
   `depth_bind` mix the generation into each bind-cache key, so an install's
   names need no generation of their own; the pipeline cache's scope is
   `(name, generation, format)`.
6. **Names are interned** (`stylize::intern`): a user style's name, label,
   parameter and option names, help lines, choices, bind names, pass labels
   and target names are `&'static str` wherever a built-in's are (bind cache
   keys, GPU timer labels, F4), from a pool keyed by content. Loading or
   reloading a style whose text didn't change adds nothing; descriptors are
   never held there.
7. **A scene holds only the user styles it sees.** `for_scene` keeps the
   natives and loads the scene's `styles/`, sharing an `Arc` the base loaded
   from the same unchanged file (so a transition within a folder keeps its
   built pass). A style only another scene could see isn't in the registry,
   so its table is an unknown style, as when the scene loads alone:
   `drop_unseen` is gone.
8. **A look carries no registry.** A look's table for a built-in style is
   checked when the look is read; one naming any other style is read
   against the registry of the scene it is laid on, which warns of a style
   it doesn't hold.

## Alternatives considered

- **A registry per thread, or an "installed" registry set around each
  call.** Cheaper to write (no signatures change), but still ambient state:
  a call made outside the window, or on another thread, silently sees the
  built-ins alone.
- **Owned text in every descriptor (`Cow`/`Arc<str>` fields).** Frees every
  string with its descriptor, but `StyleParam`, `StyleOption` and `StyleUi`
  would stop being `Copy`, and `&'static str` names flow into the bind
  cache, the GPU timer, `StyleTargets`, F4's caches and every crate that
  reads a descriptor. Interning keeps those `'static` and bounds the pool by
  what was ever typed, which an edit loop doesn't grow.
- **Keep the descriptor's address as identity, now that an `Arc` keeps it
  alive.** Correct while the slot holds the `Arc`, but an address freed and
  reused names a different install to anything that kept the number (the
  pipeline cache's scope); a generation can't be reused.

## Measured

Live heap bytes while one user style (one parameter, one option of three
choices) is saved and hot-reloaded 500 times, counted by a global allocator
(`crates/flint-render/tests/reload_memory.rs`; main at f92f10a0 run with
the same loop through its free functions; debug build, 2026-11-17):

| Reloads | Before (main) | After |
|---|---|---|
| 25 | 24,290 B | 3,080 B |
| 100 | 96,960 B | 12,296 B |
| 250 | 224,460 B | 12,296 B |
| 500 | 436,960 B | 12,296 B |

Before, each reload kept about 850 bytes for good. After, what grows is
`bind_cache::retire`'s log (four names a reload, capped at 256), full by the
64th reload; from there a reload keeps nothing. The test fails if reloads 100
to 500 leave a kilobyte more.

## Consequences

- Two renderers in one process draw two user styles of one name from two
  folders, each its own (`tests/style_registry.rs`).
- A hot reload frees the old install (`custom::tests::a_reload_frees_the_old_install`:
  a hundred reloads, every replaced install freed;
  `a_replaced_descriptor_is_built_again`: held by its pass until the pass is
  rebuilt).
- The registry's tests need no lock: each owns its registry. `RELOAD_POLL`,
  `under` and #489's "the registry may grow" ranges are gone, and the
  catalog and help tests check exact counts.
- The API changed for games: `stylize::register(&D)` is
  `registry.register(&D)` on the registry a host draws with (a game that
  embeds `PlayerApp` still returns its styles from `PlayerExtension::styles`);
  the `custom::load_*` / `reload_changed*` / `errors` free functions are
  `StyleRegistry` methods; readers take `&StyleRegistry`. The changelog
  fragment `changelog.d/742-style-registry-value.md` lists them.
- Still the process's: the interned names, the install counter,
  `bind_cache::retire`'s log of names to drop, `custom::poll_files_now`'s
  hint and `dev_shader`'s folder. None holds a descriptor.
