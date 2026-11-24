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
NOW = re.compile(r"\bnow\b\D{0,3}([\d.]+(?:e[-+]?\d+)?)")
hitch_marks, zero = [], None
for mk in marks:
    h = HITCH.search(mk["text"])
    if h:
        ms, cls, frame, wall = float(h[1]), h[2], int(h[3]), float(h[4])
        if mk["end"] is not None:  # an interval: the marker file's own span
            start, end = mk["start"], mk["end"]
        else:  # an instant written at `now`: back to the hitch's start
            now = float(NOW.search(mk["text"])[1])
            start = mk["start"] - (now - wall) * 1000.0
            end = start + ms
        hitch_marks.append({"frame": frame, "class": cls, "wall_time": wall, "ms": ms,
                            "start": start, "end": end, "raw": mk["text"][:300]})
    elif "wall_time 0" in mk["text"] or "WallTimeZero" in mk["text"]:
        if mk["end"] is not None:
            zero = mk["start"]
        else:
            zero = mk["start"] - float(NOW.search(mk["text"])[1]) * 1000.0

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
FRAME_MS = 1000.0 / 60.0
rows = []
for h in hitch_marks:
    near = [tm for tm in stall_times if h["start"] - 300 <= tm <= h["end"] + 300]
    inside = [tm for tm in near if h["start"] <= tm <= h["end"]]
    row = dict(h)
    row.update({
        "stall_samples_near": len(near),
        "stall_samples_inside": len(inside),
        "first_stall_minus_start_ms": round(near[0] - h["start"], 2) if near else None,
        "end_minus_last_stall_ms": round(h["end"] - near[-1], 2) if near else None,
        "zero_plus_wall_time_minus_start_ms": round(zero + h["wall_time"] * 1000 - h["start"], 3)
        if zero is not None else None,
    })
    row["within_a_frame"] = bool(near) and all(
        abs(row[k]) <= FRAME_MS for k in ("first_stall_minus_start_ms", "end_minus_last_stall_ms"))
    rows.append(row)

out = {
    "thread": ti, "samples": len(times), "stall_samples": len(stall_times),
    "log_lines": [{k: l[k] for k in ("frame", "wall_time", "ms", "class", "cause")} for l in log],
    "zero_mark_ms": zero, "hitch_marks": rows,
    "all_mark_names": sorted({mk["name"] for mk in marks})[:80],
    "all_held_within_a_frame": bool(rows) and all(r["within_a_frame"] for r in rows),
}
json.dump(out, open(out_path, "w", encoding="utf-8"), indent=2)
print(json.dumps(out, indent=2))
