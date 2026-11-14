"""Give every 3-argument rect_stroke( call egui 0.31+'s StrokeKind (Middle keeps 0.30's look)."""
import os
import sys


def split_args(s):
    depth, cur, out = 0, "", []
    for ch in s:
        if ch in "([{":
            depth += 1
        elif ch in ")]}":
            depth -= 1
        if ch == "," and depth == 0:
            out.append(cur)
            cur = ""
        else:
            cur += ch
    out.append(cur)
    return [a for a in out if a.strip()]


total = 0
for root in sys.argv[1:]:
    for d, _, fs in os.walk(root):
        for f in fs:
            if not f.endswith(".rs"):
                continue
            p = os.path.join(d, f)
            src = open(p, encoding="utf-8", newline="").read()
            i, out, changed = 0, [], False
            while True:
                j = src.find("rect_stroke(", i)
                if j < 0:
                    out.append(src[i:])
                    break
                start = j + len("rect_stroke(")
                depth, k = 1, start
                while depth:
                    if src[k] == "(":
                        depth += 1
                    elif src[k] == ")":
                        depth -= 1
                    k += 1
                inner = src[start:k - 1]
                args = split_args(inner)
                if len(args) == 3:
                    stripped = inner.rstrip()
                    trail = inner[len(stripped):]
                    if stripped.endswith(","):
                        nl = "\r\n" if "\r\n" in inner else "\n"
                        indent = args[0][: len(args[0]) - len(args[0].lstrip())].replace("\r", "").replace("\n", "")
                        inner = stripped + nl + indent + "egui::StrokeKind::Middle," + trail
                    else:
                        inner = stripped + ", egui::StrokeKind::Middle" + trail
                    changed = True
                    total += 1
                out.append(src[i:start] + inner + ")")
                i = k
            if changed:
                open(p, "w", encoding="utf-8", newline="").write("".join(out))
print(total)
