//! The `chunk` module: scene chunks streamed in and out by a script (ADR
//! 0116). Loads and unloads are commands the host runs after the batch.

use super::builder::kind::{Bool, Num, Str};
use super::builder::ScriptApi;
use crate::context::ScriptCommand;

pub(crate) fn register(api: &mut ScriptApi) {
    let mut m = api.module("chunk");

    m.func(
        "load_chunk",
        "Load a chunk scene file, offset in x and y, under an id of the script's choosing.",
    )
    .arg("path", Str)
    .arg("offset_x", Num)
    .arg("offset_y", Num)
    .arg("chunk_id", Str)
    .run(|call, (path, offset_x, offset_y, chunk_id)| {
        call.ctx().commands.push(ScriptCommand::LoadChunk {
            path: path.to_string(),
            offset_x,
            offset_y,
            chunk_id: chunk_id.to_string(),
        });
    });

    m.func("unload_chunk", "Unload the chunk loaded under this id.")
        .arg("chunk_id", Str)
        .run(|call, (chunk_id,)| {
            call.ctx().commands.push(ScriptCommand::UnloadChunk {
                chunk_id: chunk_id.to_string(),
            });
        });

    m.func(
        "is_chunk_loaded",
        "Whether a chunk is loaded under this id.",
    )
    .arg("chunk_id", Str)
    .returns(Bool, "false too while its load is still queued")
    .run(|call, (chunk_id,)| call.ctx_ref().loaded_chunk_ids.contains(chunk_id.as_str()));
}
