//! The game's answer to an input script's `expect` and `wait` lines
//! (#714): its world through flint-query's conditions, its persistent
//! store, what scripts gave `test_log`, and the events of the run.

use super::input_checks::FrameCheck;
use super::input_checks::{
    compare_keyed, count_holds, json_value, toml_value, Probe, Reading, Subject,
};
use super::GameCore;
use flint_script::context::{DrawCommand, DrawItem};

/// How many matching entities a reading names before "and N more".
const NAMED: usize = 4;

/// The core as an input script's probe, with the last frame drawn.
pub(crate) struct CoreProbe<'a> {
    pub core: &'a GameCore,
    pub frame: FrameSeen<'a>,
}

/// What the last frame showed, for a `frame` line (#1191): the HUD's
/// draw list, and the renderer that drew it, when there is one.
pub(crate) struct FrameSeen<'a> {
    pub hud: &'a [DrawItem],
    pub renderer: Option<&'a flint_render::SceneRenderer>,
}

impl Probe for CoreProbe<'_> {
    fn read(&self, subject: &Subject, since: f64) -> Reading {
        let core = self.core;
        match subject {
            Subject::Frame(check) => self.frame.read(core, check),
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

/// What a `frame` line found with no renderer to ask.
const NO_RENDERER: &str =
    "no frame was drawn: this run has no renderer (`flint playtest --render` draws one)";

impl FrameSeen<'_> {
    fn read(&self, core: &GameCore, check: &FrameCheck) -> Reading {
        match check {
            FrameCheck::Hud { text, present } => {
                let texts: Vec<&str> = self
                    .hud
                    .iter()
                    .filter_map(|item| match &item.cmd {
                        DrawCommand::Text { text, .. } => Some(text.as_str()),
                        _ => None,
                    })
                    .collect();
                let hit = texts.iter().find(|t| t.contains(text.as_str()));
                let found = match (hit, texts.is_empty()) {
                    (Some(t), _) => format!("\"{t}\""),
                    (None, true) => "the HUD drew no text".to_string(),
                    (None, false) => {
                        let mut s = texts[..texts.len().min(NAMED)]
                            .iter()
                            .map(|t| format!("\"{t}\""))
                            .collect::<Vec<_>>()
                            .join(", ");
                        if texts.len() > NAMED {
                            s.push_str(&format!(" and {} more", texts.len() - NAMED));
                        }
                        s
                    }
                };
                Reading {
                    holds: hit.is_some() == *present,
                    found,
                }
            }
            FrameCheck::Style { name, on } => {
                let Some(renderer) = self.renderer else {
                    return Reading {
                        holds: false,
                        found: NO_RENDERER.into(),
                    };
                };
                match renderer.style_drawn(name) {
                    Some(drawn) => Reading {
                        holds: drawn == *on,
                        found: format!("{name} {}", if drawn { "on" } else { "off" }),
                    },
                    None => Reading {
                        holds: false,
                        found: format!("no style named `{name}`"),
                    },
                }
            }
            FrameCheck::Draws { entity, drawn } => {
                let Some(renderer) = self.renderer else {
                    return Reading {
                        holds: false,
                        found: NO_RENDERER.into(),
                    };
                };
                let Some(id) = core.world.get_id(entity) else {
                    return Reading {
                        holds: false,
                        found: format!("no entity named `{entity}`"),
                    };
                };
                // The entity and what it's made of: a model's nodes are
                // its children, each drawing its part
                let mut ids = vec![id];
                let mut i = 0;
                while i < ids.len() {
                    ids.extend_from_slice(core.world.children(ids[i]));
                    i += 1;
                }
                let seen = renderer.entities_drawn(&ids);
                Reading {
                    holds: seen.unwrap_or(false) == *drawn,
                    found: match seen {
                        Some(true) => "drawn".into(),
                        Some(false) => "culled".into(),
                        None => "nothing of it in the frame's draws".into(),
                    },
                }
            }
        }
    }
}
