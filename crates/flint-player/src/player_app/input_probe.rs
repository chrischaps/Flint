//! The game's answer to an input script's `expect` and `wait` lines
//! (#714): its world through flint-query's conditions, its persistent
//! store, what scripts gave `test_log`, and the events of the run.

use super::input_checks::{
    compare_keyed, count_holds, json_value, toml_value, Probe, Reading, Subject,
};
use super::GameCore;

/// How many matching entities a reading names before "and N more".
const NAMED: usize = 4;

/// The core as an input script's probe.
pub(crate) struct CoreProbe<'a>(pub &'a GameCore);

impl Probe for CoreProbe<'_> {
    fn read(&self, subject: &Subject, since: f64) -> Reading {
        let core = self.0;
        match subject {
            Subject::Entity { name, condition } => {
                let Some(id) = core.world.get_id(name) else {
                    return Reading {
                        holds: false,
                        found: format!("no entity named `{name}`"),
                    };
                };
                let Some(entity) = core.world.all_entities().into_iter().find(|e| e.id == id)
                else {
                    return Reading {
                        holds: false,
                        found: format!("no entity named `{name}`"),
                    };
                };
                let holds = flint_query::entity_matches(&core.world, &entity, condition);
                let found = flint_query::field_value(&core.world, &entity, &condition.field)
                    .map(|v| format!("{} = {v}", condition.field))
                    .unwrap_or_else(|| format!("`{name}` has no {}", condition.field));
                Reading { holds, found }
            }
            Subject::Entities { count, condition } => {
                let names: Vec<String> = core
                    .world
                    .all_entities()
                    .into_iter()
                    .filter(|e| flint_query::entity_matches(&core.world, e, condition))
                    .map(|e| e.name)
                    .collect();
                let mut found = format!(
                    "{} match{}",
                    names.len(),
                    if names.len() == 1 { "" } else { "es" }
                );
                if !names.is_empty() {
                    found.push_str(": ");
                    found.push_str(&names[..names.len().min(NAMED)].join(", "));
                    if names.len() > NAMED {
                        found.push_str(&format!(" and {} more", names.len() - NAMED));
                    }
                }
                Reading {
                    holds: count_holds(*count, names.len()),
                    found,
                }
            }
            Subject::Persist(condition) => compare_keyed(
                condition,
                core.store.get(&condition.field).and_then(toml_value),
                || format!("the store has no `{}`", condition.field),
            ),
            Subject::Logged(condition) => {
                let record = core.script.test_record();
                compare_keyed(
                    condition,
                    record.logged.get(&condition.field).and_then(json_value),
                    || format!("no test_log(\"{}\", ..) yet", condition.field),
                )
            }
            Subject::Event(name) => {
                let Some(log) = &core.event_log else {
                    return Reading {
                        holds: false,
                        found: "this run keeps no event log".into(),
                    };
                };
                let fired: Vec<f64> = log
                    .iter()
                    .filter(|(t, n)| n == name && *t + 1e-9 >= since)
                    .map(|(t, _)| *t)
                    .collect();
                Reading {
                    holds: !fired.is_empty(),
                    found: match fired.first() {
                        Some(t) => format!("fired at {t:.3} s"),
                        None => "not fired".into(),
                    },
                }
            }
        }
    }
}
