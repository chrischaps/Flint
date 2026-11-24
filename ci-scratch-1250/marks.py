"""#1250 scratch: does each --hitch-log hitch's mark sit on its own samples?

   python3 marks.py <profile.json.gz> <samply-url.txt> <hitches.jsonl> <out.json>

Finds flint-player's main thread, its hitch marks (samply's marker file on
Linux: an interval named "hitch ..."; an ETW event on Windows, imported
with --unknown-event-markers: an instant at the moment it was written,
carrying the hitch's wall_time and the wall_time it was written at), and
symbolicates the thread through samply's own symbol server. For each
hitch it compares the mark's span with the samples inside the Rhai stall
(stall.rhai's busy wait): the first and last stall sample against the
mark's start and end, in ms."""
import gzip
import json
import re
import sys
import urllib.parse
import urllib.request

prof_path, url_file, log_path, out_path = sys.argv[1:5]
p = json.load(gzip.open(prof_path))
url = open(url_file, encoding="utf-8").read().strip().split()[-1]
server = urllib.parse.unquote(url.split("symbolServer=")[1])
log = [json.loads(l) for l in open(log_path, encoding="utf-8") if l.strip()]

cands = [
    (sum(1 for s in t["samples"]["stack"] if s is not None), i)
    for i, t in enumerate(p["threads"])
    if t.get("isMainThread") and str(t.get("processName", "")).startswith("flint-player")
]
_, ti = max(cands)
t = p["threads"][ti]
strings = t.get("stringArray") or p.get("shared", {}).get("stringArray") or []

def text_of(v):
    if isinstance(v, str):
        return v
    if isinstance(v, int) and 0 <= v < len(strings):
        return strings[v]
    return json.dumps(v)

m = t["markers"]
marks = []
for i in range(m["length"]):
    name = text_of(m["name"][i])
    data = m["data"][i] or {}
    words = [name] + [text_of(v) for k, v in data.items() if k != "type"]
    marks.append({"name": name, "start": m["startTime"][i], "end": m["endTime"][i],
                  "text": " | ".join(words)})

HITCH = re.compile(r"hitch ([\d.]+) ms (\w+): .*?\(frame (\d+), wall_time ([\d.]+)\)")
def field(text, key):
    m = re.search(r"\b" + key + r"\b\W{0,3}(\d+)", text)
    return int(m[1]) if m else None

hitch_marks, zero = [], None
for mk in marks:
    text = mk["text"]
    h = HITCH.search(text)
    if mk["name"].startswith("Hitch"):  # Windows: an instant, written at now_us
        now, wall, dur = field(text, "now_us"), field(text, "wall_time_us"), field(text, "duration_us")
        start = mk["start"] - (now - wall) / 1000.0
        hitch_marks.append({"frame": field(text, "frame"), "class": h[2] if h else None,
                            "wall_time": wall / 1e6, "ms": dur / 1000.0,
                            "start": start, "end": start + dur / 1000.0,
                            "event_after_end_ms": round(mk["start"] - start - dur / 1000.0, 3),
                            "raw": text[:400]})
    elif mk["end"] and h:  # Linux: the marker file's own span
        ms, cls, frame, wall = float(h[1]), h[2], int(h[3]), float(h[4])
        hitch_marks.append({"frame": frame, "class": cls, "wall_time": wall, "ms": ms,
                            "start": mk["start"], "end": mk["end"], "raw": text[:300]})
    elif "wall_time 0" in text:
        zero = mk["start"]
    elif mk["name"].startswith("WallTimeZero"):
        zero = mk["start"] - field(text, "now_us") / 1000.0

# samples' times
s = t["samples"]
if "time" in s:
    times = s["time"]
else:
    times, acc = [], 0.0
    for d in s["timeDeltas"]:
        acc += d
        times.append(acc)

# symbolicate (as #1249's symstats.py): every frame -> lib, address
ft, fn, rt, st = t["frameTable"], t["funcTable"], t["resourceTable"], t["stackTable"]
libs = p["libs"]
frame_lib = []
for f in range(ft["length"]):
    res = fn["resource"][ft["func"][f]]
    frame_lib.append(rt["lib"][res] if res is not None and res >= 0 else None)
used = sorted({l for l in frame_lib if l is not None})
mm_index = {l: k for k, l in enumerate(used)}
memory_map = [[libs[l]["debugName"], libs[l]["breakpadId"]] for l in used]
query, qi = [], {}
for f in range(ft["length"]):
    l, a = frame_lib[f], ft["address"][f]
    if l is None or a is None or a < 0:
        continue
    qi[f] = len(query)
    query.append([mm_index[l], a])
req = json.dumps({"jobs": [{"memoryMap": memory_map, "stacks": [query]}]}).encode()
r = urllib.request.urlopen(urllib.request.Request(server + "/symbolicate/v5", data=req,
                           headers={"Content-Type": "application/json"}), timeout=900)
res = json.load(r)["results"][0]["stacks"][0]

def names_of(f):
    if f not in qi:
        return []
    sym = res[qi[f]]
    out = [sym.get("function") or ""]
    out += [inl.get("function", "") for inl in (sym.get("inlines") or [])]
    return out

stall_of = {}
def is_stall(stack):
    if stack is None:
        return False
    if stack not in stall_of:
        hit, x = False, stack
        while x is not None and not hit:
            hit = any("rhai" in n for n in names_of(st["frame"][x]))
            x = st["prefix"][x]
        stall_of[stack] = hit
    return stall_of[stack]

stall_times = [tm for tm, stk in zip(times, s["stack"]) if is_stall(stk)]
interval = float(p["meta"].get("interval", 1.0))
# The stall's samples come back to back; a frame's own Rhai sample is alone.
runs = []
for tm in stall_times:
    if runs and tm - runs[-1][-1] <= 12.0:
        runs[-1].append(tm)
    else:
        runs.append([tm])
long_runs = [r for r in runs if len(r) >= 15]
FRAME_MS = 1000.0 / 60.0
rows, claimed = [], set()
for h in hitch_marks:
    best, overlap = None, 0.0
    for k, r in enumerate(long_runs):
        o = min(r[-1], h["end"]) - max(r[0], h["start"])
        if o > overlap:
            best, overlap = k, o
    row = dict(h)
    if best is not None:
        r = long_runs[best]
        claimed.add(best)
        row.update({
            "stall_run_samples": len(r),
            "stall_run_ms": round(r[-1] - r[0] + interval, 2),
            "run_start_minus_mark_start_ms": round(r[0] - h["start"], 2),
            "mark_end_minus_run_end_ms": round(h["end"] - (r[-1] + interval), 2),
        })
        row["within_a_frame"] = (abs(row["run_start_minus_mark_start_ms"]) <= FRAME_MS
                                 and -FRAME_MS <= row["mark_end_minus_run_end_ms"] <= h["ms"])
    else:
        row["within_a_frame"] = False
    if zero is not None:
        row["zero_plus_wall_time_minus_start_ms"] = round(zero + h["wall_time"] * 1000 - h["start"], 3)
    rows.append(row)
unmarked = [{"start": r[0], "samples": len(r)} for k, r in enumerate(long_runs) if k not in claimed]

out = {
    "thread": ti, "samples": len(times), "stall_samples": len(stall_times),
    "log_lines": [{k: l[k] for k in ("frame", "wall_time", "ms", "class", "cause")} for l in log],
    "zero_mark_ms": zero, "hitch_marks": rows, "stall_runs": len(long_runs),
    "stall_runs_without_a_mark": unmarked,
    "all_mark_names": sorted({mk["name"] for mk in marks})[:80],
    "all_held_within_a_frame": bool(rows) and all(r["within_a_frame"] for r in rows),
}
# The profiler's timeline starts at the earliest thread's start: a range
# around the first hitch for the screenshot, in ms from there
zero_at = min(th.get("processStartupTime", th.get("registerTime", 0)) for th in p["threads"])
first = next((r for r in rows if r.get("stall_run_samples")), rows[0] if rows else None)
out["screenshot_range"] = ([round(first["start"] - zero_at - 60), round(first["ms"] + 120)]
                           if first else None)
json.dump(out, open(out_path, "w", encoding="utf-8"), indent=2)
print(json.dumps(out, indent=2), file=sys.stderr)
r = out["screenshot_range"] or [0, 1000]
print(ti, r[0], r[1])
