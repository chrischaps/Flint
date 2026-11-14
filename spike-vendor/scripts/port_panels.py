"""egui 0.36: SidePanel / TopBottomPanel -> Panel, shown on a Ui. argv: files."""
import re
import sys

START = re.compile(r"egui::(SidePanel|TopBottomPanel)::(left|right|top|bottom)\(")
RENAMES = [
    (r"\.default_width\(", ".default_size("),
    (r"\.default_height\(", ".default_size("),
    (r"\.min_width\(", ".min_size("),
    (r"\.min_height\(", ".min_size("),
    (r"\.max_width\(", ".max_size("),
    (r"\.max_height\(", ".max_size("),
    (r"\.exact_width\(", ".exact_size("),
    (r"\.exact_height\(", ".exact_size("),
    (r"\.width_range\(", ".size_range("),
    (r"\.height_range\(", ".size_range("),
]
total = 0
for path in sys.argv[1:]:
    src = open(path, encoding="utf-8", newline="").read()
    out, i = [], 0
    while True:
        m = START.search(src, i)
        if not m:
            out.append(src[i:])
            break
        out.append(src[i:m.start()])
        show = re.compile(r"\.show(_animated)?\(\s*&?ctx\s*,").search(src, m.end())
        chain = src[m.start():show.start()]
        chain = START.sub(lambda s: f"egui::Panel::{s.group(2)}(", chain, count=1)
        for pat, rep in RENAMES:
            chain = re.sub(pat, rep, chain)
        anim = show.group(1) or ""
        out.append(chain + f".show{anim}(ui,")
        i = show.end()
        total += 1
    open(path, "w", encoding="utf-8", newline="").write("".join(out))
print(total)
