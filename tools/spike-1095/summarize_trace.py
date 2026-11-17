"""Spike #1095: what a recorded trace holds."""
import json
import sys
from collections import defaultdict

doc = json.load(open(sys.argv[1], encoding="utf-8"))
ev = [e for e in doc["traceEvents"] if e["ph"] == "X"]
names = defaultdict(list)
for e in ev:
    names[e["name"]].append(e["dur"])
frames = {e["args"]["frame"] for e in ev}
print(f"{len(ev)} scopes, {len(names)} distinct names, {len(frames)} frames, "
      f"{len(ev) / max(1, len(frames)):.1f} scopes a frame")
for n, d in sorted(names.items(), key=lambda kv: -sum(kv[1])):
    d.sort()
    print(f"  {n:<28} n={len(d):>5}  median {d[len(d)//2]:>9.1f} us  max {d[-1]:>10.1f} us")
worst = max((e for e in ev if e["name"] == "tick"), key=lambda e: e["dur"])
print(f"slowest tick: frame {worst['args']['frame']}, {worst['dur']/1000:.1f} ms, children:")
for e in ev:
    if (e["args"]["frame"] == worst["args"]["frame"] and e["args"]["depth"] > 0
            and worst["ts"] <= e["ts"] <= worst["ts"] + worst["dur"]):
        print(f"    {'  ' * e['args']['depth']}{e['name']} {e['dur']/1000:.2f} ms")
