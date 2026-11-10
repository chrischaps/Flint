//! Hot reload's reads and compiles, off the frame thread (#633).
//!
//! A changed script is read and compiled here, with an engine that parses as
//! the game's does ([`crate::engine::compiler`]), and the frame thread only
//! swaps the finished AST in. A changed `scripts/lib` is read and compiled
//! here too, every module of it, so the imports after it evaluate ASTs
//! instead of reading files in a frame. Native only: on the web nothing
//! reloads.

use flint_core::file_watch;
use flint_core::EntityId;
use rhai::AST;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::mpsc::{channel, Receiver, Sender};
use std::time::SystemTime;

/// What to do.
pub(crate) enum Job {
    /// Read and compile an entity's script.
    Script { entity: EntityId, path: PathBuf },
    /// Read and compile these lib modules (every `.rhai` under the folder).
    Lib { files: Vec<PathBuf> },
}

/// What came of it.
pub(crate) enum Done {
    Script {
        entity: EntityId,
        path: PathBuf,
        /// The file's modified time as it was before it was read: a write
        /// after the read is newer, so it is noticed again.
        modified: Option<SystemTime>,
        ast: Result<AST, String>,
    },
    Lib {
        /// Each module, compiled or why not (reported where it's imported;
        /// fixed, it changes again and is compiled again).
        compiled: HashMap<PathBuf, Result<AST, String>>,
    },
}

/// The worker thread and its two channels.
pub(crate) struct ReloadWorker {
    jobs: Sender<Job>,
    done: Receiver<Done>,
}

impl ReloadWorker {
    /// A worker on its own thread, or `None` if one couldn't start (hot
    /// reload then compiles on the caller's thread, as it used to).
    pub(crate) fn spawn() -> Option<Self> {
        let (jobs, job_rx) = channel::<Job>();
        let (done_tx, done) = channel::<Done>();
        std::thread::Builder::new()
            .name("flint-script-reload".into())
            .spawn(move || {
                let engine = crate::engine::compiler();
                for job in job_rx {
                    let done = match job {
                        Job::Script { entity, path } => {
                            // Read once it holds still, so a file caught
                            // mid-write isn't compiled as a shorter script
                            let (ast, modified) = match file_watch::read_settled(&path) {
                                Ok((src, at)) => (
                                    engine.compile(&src).map_err(|e| e.to_string()),
                                    at.map(|(t, _)| t),
                                ),
                                Err(e) => (Err(e), file_watch::modified(&path)),
                            };
                            Done::Script {
                                entity,
                                path,
                                modified,
                                ast,
                            }
                        }
                        Job::Lib { files } => {
                            let compiled = files
                                .into_iter()
                                .map(|file| {
                                    let name = file.to_string_lossy().into_owned();
                                    let ast =
                                        file_watch::read_settled(&file).and_then(|(src, _)| {
                                            crate::module_resolver::compile_module_source(
                                                &engine,
                                                src,
                                                &name,
                                                rhai::Position::NONE,
                                            )
                                            .map_err(|e| e.to_string())
                                        });
                                    (file, ast)
                                })
                                .collect();
                            Done::Lib { compiled }
                        }
                    };
                    if done_tx.send(done).is_err() {
                        return; // its ScriptSync is gone
                    }
                }
            })
            .map_err(|e| eprintln!("[script] couldn't start the reload worker: {e}"))
            .ok()?;
        Some(Self { jobs, done })
    }

    /// Hand a job over; false if the worker has gone.
    pub(crate) fn send(&self, job: Job) -> bool {
        self.jobs.send(job).is_ok()
    }

    /// Everything finished since the last call, without waiting.
    pub(crate) fn finished(&self) -> Vec<Done> {
        self.done.try_iter().collect()
    }
}
