"""rep.py FILE OLD NEW [all]: replace text, matching the file's line endings.

OLD and NEW are read from files when they start with @ (so no shell escaping).
"""
import sys

path, old, new = sys.argv[1], sys.argv[2], sys.argv[3]
every = len(sys.argv) > 4 and sys.argv[4] == "all"
if old.startswith("@"):
    old = open(old[1:], encoding="utf-8").read()
if new.startswith("@"):
    new = open(new[1:], encoding="utf-8").read()
src = open(path, encoding="utf-8", newline="").read()
crlf = "\r\n" in src
if crlf:
    old = old.replace("\r\n", "\n").replace("\n", "\r\n")
    new = new.replace("\r\n", "\n").replace("\n", "\r\n")
n = src.count(old)
if n == 0:
    sys.exit(f"NOT FOUND in {path}: {old[:80]!r}")
if n > 1 and not every:
    sys.exit(f"{n} matches in {path}; pass 'all'")
src = src.replace(old, new)
open(path, "w", encoding="utf-8", newline="").write(src)
print(f"{path}: {n}")
