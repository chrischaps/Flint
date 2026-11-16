### An entity's components are held in name order (#1115)
**What changed.** `DynamicComponents::data` in `flint-ecs` is a
`BTreeMap<String, toml::Value>` (it was a `HashMap`). Walking it gives the same
order in every process, by name. The scene viewer's inspector and
`flint entity show` read it directly, so they listed an entity's components in
a new order each run. Now they list them in name order every time. Scenes,
scripts and saved files are unchanged.
**What to do.** Nothing, unless your game binary's Rust code names the field's
type (a `&HashMap<String, toml::Value>` taken from `components.data`). Change
that to `BTreeMap`, or read through `get`, `component_names` and `data.iter()`,
which work with either.
