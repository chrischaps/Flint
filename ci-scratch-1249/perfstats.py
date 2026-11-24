"""#1249 scratch: summarise `perf script` output: frames a sample, how many
named, how many in Flint's crates.   python3 perfstats.py <perf script file> <comm>"""
import json
import sys

path, comm = sys.argv[1], sys.argv[2]
samples = []
cur = None
with open(path, encoding="utf-8", errors="replace") as f:
    for line in f:
        if not line.strip():
            if cur is not None:
                samples.append(cur)
            cur = None
            continue
        if not line.startswith(("\t", " ")):
            cur = {"head": line.strip(), "frames": []}
            continue
        if cur is not None:
            parts = line.strip().split(" ", 1)
            sym = parts[1] if len(parts) > 1 else ""
            cur["frames"].append(sym)
if cur is not None:
    samples.append(cur)

mine = [s for s in samples if s["head"].startswith(comm) or comm in s["head"].split(" ")[0]]
mine = mine or samples
n = len(mine)
frames = sum(len(s["frames"]) for s in mine)
unknown = sum(sum(1 for x in s["frames"] if x.startswith("[unknown]")) for s in mine)
flint = sum(1 for s in mine if any("flint_" in x for x in s["frames"]))
stall = [s for s in mine if any("rhai" in x for x in s["frames"])]
deepest = max(stall or mine, key=lambda s: len(s["frames"]), default={"frames": []})
out = {
    "samples": n,
    "avgDepth": round(frames / n, 1) if n else 0,
    "unknownFrac": round(unknown / frames, 3) if frames else 0,
    "flintSampleFrac": round(flint / n, 3) if n else 0,
    "stallSamples": len(stall),
    "stallAvgDepth": round(sum(len(s["frames"]) for s in stall) / len(stall), 1) if stall else 0,
    "deepestStall": [x.split(" (")[0] for x in deepest["frames"]][:80],
}
print(json.dumps(out, indent=2))
