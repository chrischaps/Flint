"""Compare two goldens --out-dir folders render by render: hash, then pixel stats. argv: A B [json_out]"""
import hashlib
import json
import os
import sys

from PIL import Image, ImageChops

a, b = sys.argv[1], sys.argv[2]
out = sys.argv[3] if len(sys.argv) > 3 else None
names = sorted(f for f in os.listdir(a) if f.endswith(".png") and not f.endswith(".diff.png"))
rows, same, differ, missing = [], 0, 0, []
for n in names:
    pb = os.path.join(b, n)
    if not os.path.exists(pb):
        missing.append(n)
        continue
    ha = hashlib.sha256(open(os.path.join(a, n), "rb").read()).hexdigest()
    hb = hashlib.sha256(open(pb, "rb").read()).hexdigest()
    if ha == hb:
        same += 1
        continue
    ia = Image.open(os.path.join(a, n)).convert("RGBA")
    ib = Image.open(pb).convert("RGBA")
    if ia.size != ib.size:
        rows.append({"name": n, "size": [ia.size, ib.size]})
        differ += 1
        continue
    d = ImageChops.difference(ia, ib)
    px = list(d.getdata())
    changed = sum(1 for p in px if max(p) > 0)
    mx = max(max(p) for p in px)
    mean = sum(sum(p[:3]) for p in px) / (3 * len(px))
    rows.append({"name": n, "changed_px": changed, "changed_pct": 100.0 * changed / len(px), "max": mx, "mean": mean})
    differ += 1
print(f"{len(names)} renders: {same} byte-identical, {differ} differ, {len(missing)} missing in B")
for r in sorted(rows, key=lambda r: -r.get("changed_pct", 100)):
    if "size" in r:
        print(f"  {r['name']}: size {r['size']}")
    else:
        print(f"  {r['name']:48} {r['changed_pct']:7.3f}% px  max {r['max']:3}  mean {r['mean']:.4f}")
if out:
    json.dump({"total": len(names), "same": same, "differ": differ, "rows": rows, "missing": missing}, open(out, "w"), indent=1)
