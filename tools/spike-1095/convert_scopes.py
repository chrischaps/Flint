"""Spike #1095: turn each #[profiling::function] into a flint_core::scope! line."""
import re
import sys

total = 0
for path in sys.argv[1:]:
    lines = open(path, encoding="utf-8").read().split("\n")
    out = []
    i = 0
    while i < len(lines):
        line = lines[i]
        if line.strip() == "#[profiling::function]":
            # Find the fn line, then the line that opens its body.
            j = i + 1
            while not re.search(r"\bfn\s+\w+", lines[j]):
                j += 1
            name = re.search(r"\bfn\s+(\w+)", lines[j]).group(1)
            k = j
            while not lines[k].rstrip().endswith("{"):
                k += 1
            body_indent = re.match(r"\s*", lines[j]).group(0) + "    "
            out.extend(lines[i + 1 : k + 1])
            out.append(f'{body_indent}flint_core::scope!("{name}");')
            total += 1
            i = k + 1
            continue
        out.append(line)
        i += 1
    open(path, "w", encoding="utf-8").write("\n".join(out))
print(f"converted {total} scopes")
