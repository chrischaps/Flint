# Spike #963: point every flint-render pipeline descriptor at the driver cache.
import pathlib, re
root = pathlib.Path(r"C:\Users\chris\dev\Flint\.claude\worktrees\signal-963-0276\crates\flint-render\src")
n = 0
for p in root.rglob("*.rs"):
    if p.name == "driver_cache.rs":
        continue
    with open(p, encoding="utf-8", newline="") as f:
        t = f.read()
    new, k = re.subn(r"\bcache: None,", "cache: crate::driver_cache::cache(),", t)
    if k:
        with open(p, "w", encoding="utf-8", newline="") as f:
            f.write(new)
        n += k
        print(p.relative_to(root), k)
print("total", n)
