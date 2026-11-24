"""#1249 scratch: symbolicate a samply profile's flint-player main thread
through samply's own symbol server (POST <server>/symbolicate/v5, what the
Firefox Profiler calls), and measure its stacks.
   python3 symstats.py <profile.json.gz> <samply-url.txt> <out.json>
Prints the main thread's index (for the profiler URL's thread=) last."""
import gzip
import json
import sys
import urllib.parse
import urllib.request

prof_path, url_file, out_path = sys.argv[1:4]
p = json.load(gzip.open(prof_path))
url = open(url_file, encoding="utf-8").read().strip().split()[-1]
server = urllib.parse.unquote(url.split("symbolServer=")[1])

cands = [
    (sum(1 for s in t["samples"]["stack"] if s is not None), i)
    for i, t in enumerate(p["threads"])
    if t.get("isMainThread") and str(t.get("processName", "")).startswith("flint-player")
]
n_samples, ti = max(cands)
t = p["threads"][ti]
ft, fn, rt, st = t["frameTable"], t["funcTable"], t["resourceTable"], t["stackTable"]
libs = p["libs"]

# every frame -> (lib index, module offset)
frame_lib = []
for f in range(ft["length"]):
    res = fn["resource"][ft["func"][f]]
    lib = rt["lib"][res] if res is not None and res >= 0 else None
    frame_lib.append(lib)
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
                           headers={"Content-Type": "application/json"}), timeout=600)
res = json.load(r)["results"][0]["stacks"][0]


def frame_names(f):
    """Outer function first, then what was inlined into it (outermost first)."""
    if f not in qi:
        return [("?", False)]
    s = res[qi[f]]
    name = s.get("function")
    if not name:
        return [(f"0x{ft['address'][f]:x}", False)]
    out = [(name, s.get("line") is not None)]
    for inl in reversed(s.get("inlines") or []):
        out.append((inl.get("function", "?"), inl.get("line") is not None))
    return out


cache = {}
def stack_frames(s):
    out = []
    while s is not None:
        out.append(st["frame"][s])
        s = st["prefix"][s]
    return out[::-1]  # root first


rows = []
for s in t["samples"]["stack"]:
    if s is None:
        continue
    if s not in cache:
        frames = stack_frames(s)
        names = []
        for f in frames:
            names.extend(frame_names(f))
        cache[s] = (len(frames), names)
    rows.append(cache[s])

def summarise(sel):
    if not sel:
        return None
    n = len(sel)
    named = [[x for x, _ in names if x != "?" and not x.startswith("0x")] for _, names in sel]
    return {
        "samples": n,
        "framesPerSample": round(sum(fr for fr, _ in sel) / n, 1),
        "functionsPerSample": round(sum(len(x) for x in named) / n, 1),
        "lineFrac": round(sum(sum(1 for _, ln in names if ln) for _, names in sel)
                          / max(1, sum(len(names) for _, names in sel)), 3),
        "flintSampleFrac": round(sum(1 for x in named if any("flint_" in y for y in x)) / n, 3),
        "distinctFunctions": len({y for x in named for y in x}),
    }

stall = [r for r in rows if any("rhai" in x for x, _ in r[1])]
deepest = max(stall or rows, key=lambda r: len(r[1]))
flint_funcs = sorted({x for _, names in rows for x, _ in names if x.startswith(("flint_", "<flint_"))})
out = {
    "thread": ti, "threadName": t.get("name"), "processName": t.get("processName"),
    "all": summarise(rows), "stall": summarise(stall),
    "deepestStall": [x for x, _ in deepest[1]],
    "flintFunctionsSeen": len(flint_funcs), "flintFunctionsSample": flint_funcs[:60],
}
json.dump(out, open(out_path, "w", encoding="utf-8"), indent=2)
print(json.dumps({k: out[k] for k in ("thread", "all", "stall", "flintFunctionsSeen")}, indent=2), file=sys.stderr)
print(ti)
