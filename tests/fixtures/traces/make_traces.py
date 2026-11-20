"""Writes the two hand-made traces `flint profile`'s tests read (#1098).

    python tests/fixtures/traces/make_traces.py

Each is the shape `flint-player --trace` writes (ADR 0128, #1097): one
`"ph":"X"` event a scope with `frame` and `depth` args, written as each
scope closes (children before their parent), on a `main` thread and an
`io` worker, then the player's `frames`, `phases`, `gpu`, `hitches` and
`notes` tracks, and the header as `otherData`. Sixteen frames, 100..115,
about 4 ms each, every number fixed (no randomness), so a test can name the
answer:

- `stall.trace.json`: frame 105 waits 40 ms in `acquire` (no scope covers
  it), frame 109 loads `models/whale.glb` for 30 ms (a scope that runs in
  no other frame), frame 113 is a script stalling 100 ms
  (`stall.rhai:on_update`, inside `tick > run_scripts > script::call`).
  The hitches track has all three.
- `calm.trace.json`: the same game with no stalls, whose `encode_to` takes
  0.4 ms a frame where the stall trace's takes 0.9: what `diff` must find.
"""

import json
from pathlib import Path

HERE = Path(__file__).resolve().parent
FIRST = 100
FRAMES = 16


def us(ms):
    return round(ms * 1000.0, 3)


class Writer:
    def __init__(self):
        self.lines = []
        self.tracks = {}

    def track(self, name):
        tid = len(self.tracks) + 1
        self.tracks[name] = tid
        self.event({"ph": "M", "name": "thread_name", "pid": 1, "tid": tid, "args": {"name": name}})
        self.event(
            {"ph": "M", "name": "thread_sort_index", "pid": 1, "tid": tid, "args": {"sort_index": tid}}
        )
        return tid

    def event(self, e):
        self.lines.append(json.dumps(e, separators=(",", ":")))

    def x(self, tid, name, start, dur, **args):
        self.event(
            {"ph": "X", "name": name, "pid": 1, "tid": tid, "ts": us(start), "dur": us(dur), "args": args}
        )

    def text(self, header):
        return (
            '{"displayTimeUnit":"ms","traceEvents":[\n'
            + ",\n".join(self.lines)
            + '\n],"otherData":'
            + json.dumps(header, separators=(",", ":"))
            + "}\n"
        )


def trace(stalls, encode_ms):
    w = Writer()
    main, io = w.track("main"), w.track("io")
    frames, phases, gpu, hitches, notes = [], [], [], [], []
    t = 1000.0  # ms from the recorder's epoch
    for i in range(FRAMES):
        f = FIRST + i
        jitter = ((i * 37) % 7) * 0.01
        script_stall = 100.0 if stalls and f == 113 else 0.0
        load = 30.0 if stalls and f == 109 else 0.0
        wait = 40.0 if stalls and f == 105 else 0.0

        start = t
        # tick: input, physics, then the scripts
        tick_start = t
        t += 0.05  # input
        w.x(main, "physics::step", t, 0.03, frame=f, depth=1, detail=1)
        t += 0.05
        scripts_start = t
        rs_start = t
        t += 0.01
        call = t
        w.x(main, "metronome.rhai:on_update", t + 0.005, 0.2 + jitter, frame=f, depth=3)
        t += 0.21 + jitter
        w.x(main, "script::call", call, t - call, frame=f, depth=2, detail="scripts/metronome.rhai")
        call = t
        w.x(main, "stall.rhai:on_update", t + 0.005, 0.05 + script_stall, frame=f, depth=3)
        t += 0.06 + script_stall
        w.x(main, "script::call", call, t - call, frame=f, depth=2, detail="scripts/stall.rhai")
        t += 0.01
        w.x(main, "run_scripts", rs_start, t - rs_start, frame=f, depth=1)
        scripts_end = t
        w.x(main, "tick", tick_start, t - tick_start, frame=f, depth=0)
        # sync: the renderer's prepare, and a model load when there is one
        sync_start = t
        if load:
            w.x(main, "import_model", t + 0.01, load, frame=f, depth=1, detail="models/whale.glb")
        t += 0.1 + load
        w.x(main, "sync_renderer", sync_start, t - sync_start, frame=f, depth=0)
        # render: encode, submit
        render_start = t
        w.x(main, "encode_to", t + 0.02, encode_ms, frame=f, depth=1)
        w.x(main, "submit", t + 0.03 + encode_ms, 0.3, frame=f, depth=1)
        t += 0.35 + encode_ms + 0.05
        w.x(main, "render", render_start, t - render_start, frame=f, depth=0)
        render_end = t
        acquire = 0.5 + wait
        t += acquire
        present = 2.0 - jitter
        t += present
        if i % 4 == 0:
            w.x(io, "decode_image", start + 0.5, 0.8, frame=f, depth=0, detail="textures/moss.png")

        frames.append((f, start, t - start))
        phases += [
            ("input", f, tick_start, 0.05),
            ("scripts", f, scripts_start - 0.05, scripts_end - scripts_start + 0.05),
            ("sync", f, sync_start, render_start - sync_start),
            ("render", f, render_start, render_end - render_start),
            ("acquire", f, render_end, acquire),
            ("present", f, render_end + acquire, present),
        ]
        gpu += [("Scene", f, render_end, 0.6), ("UI", f, render_end + 0.6, 0.05)]
        if load:
            notes.append(("model load", start, "models/whale.glb", 30.0))
        for at, cause in [
            (105, "elsewhere, acquire 40.5 ms"),
            (109, "model load models/whale.glb 30.0 ms"),
            (113, "script stall.rhai:on_update 100.1 ms"),
        ]:
            if stalls and f == at:
                hitches.append((start, t - start, cause))

    tid = w.track("frames")
    for f, start, dur in frames:
        w.x(tid, "frame", start, dur, frame=f, ms=round(dur, 3))
    tid = w.track("phases")
    for name, f, start, dur in phases:
        w.x(tid, name, start, dur, frame=f)
    tid = w.track("gpu")
    for name, f, start, dur in gpu:
        w.x(tid, name, start, dur, frame=f, gpu_frame_ms=0.65)
    tid = w.track("hitches")
    for start, dur, cause in hitches:
        w.x(tid, "hitch", start, dur, judged_ms=round(dur, 3), median_ms=4.0, cause=cause)
    tid = w.track("notes")
    for kind, at, detail, ms in notes:
        w.event(
            {
                "ph": "i",
                "s": "t",
                "name": kind,
                "pid": 1,
                "tid": tid,
                "ts": us(at),
                "args": {"detail": detail, "depth": 0, "ms": ms},
            }
        )
    header = {
        "format": "flint-trace",
        "version": 1,
        "written_by": "tests/fixtures/traces/make_traces.py",
        "reason": "hitch at t=0.420 s, 104.2 ms: script stall.rhai:on_update 100.1 ms"
        if stalls
        else "exit",
        "scene": "flint-examples/hitch_metronome.scene.toml",
        "build": "debug",
        "adapter": "a hand-written trace",
        "scopes": sum(1 for line in w.lines if '"depth"' in line),
        "frames": FRAMES,
        "threads": 2,
    }
    return w.text(header)


(HERE / "stall.trace.json").write_text(trace(True, 0.9), newline="\n")
(HERE / "calm.trace.json").write_text(trace(False, 0.4), newline="\n")
