"""Mechanical egui 0.30 -> 0.36 renames for the #934 spike. argv: files or dirs."""
import os
import re
import sys

counts = {}

RULES = [
    ("copied_text", r"(\w+)\.output_mut\(\|o\| o\.copied_text = ([^)]+)\)", r"\1.ctx().copy_text(\2)"),
    ("screen_rect()", r"\.screen_rect\(\)", ".content_rect()"),
    ("Frame::none()", r"\bFrame::none\(\)", "Frame::NONE"),
    ("frame .rounding(", r"\.rounding\(", ".corner_radius("),
    ("Rounding::", r"\bRounding::", "CornerRadius::"),
    ("ctx.style()", r"\b(ctx|egui_ctx|self\.ctx|context)\.style\(\)", r"\1.global_style()"),
    ("ctx.set_style(", r"\b(ctx|egui_ctx|self\.ctx|context)\.set_style\(", r"\1.set_global_style("),
    ("ctx.style_mut(", r"\b(ctx|egui_ctx|self\.ctx|context)\.style_mut\(", r"\1.global_style_mut("),
    ("id_source(", r"\.id_source\(", ".id_salt("),
    ("from_id_source(", r"::from_id_source\(", "::from_id_salt("),
    ("close_menu()", r"\.close_menu\(\)", ".close()"),
    # ctx.run(input, |ctx| { CentralPanel::default().show(ctx, ...) })
    ("ctx.run -> run_ui", r"\.run\(([^,()]+(?:\([^()]*\))?), \|ctx\| \{", r".run_ui(\1, |ui| { let ctx = &ui.ctx().clone();"),
    ("CentralPanel show(ctx", r"(CentralPanel::default\(\)(?:\s*\.frame\([^)]*\))?\s*\.show)\(ctx,", r"\1(ui,"),
]


def margin_ints(src):
    # Margin::same(8.0) / symmetric(12.0, 9.0): i8 since 0.31
    def fix(m):
        args = [a.strip() for a in m.group(2).split(",")]
        out = []
        for a in args:
            if re.fullmatch(r"-?\d+(\.0*)?(_?f32)?", a):
                out.append(str(int(float(a.replace("_f32", "").replace("f32", "")))))
            else:
                out.append(a)
        return f"Margin::{m.group(1)}({', '.join(out)})"
    return re.subn(r"\bMargin::(same|symmetric)\(([^()]*)\)", fix, src)


def walk(paths):
    for p in paths:
        if os.path.isdir(p):
            for d, _, fs in os.walk(p):
                for f in fs:
                    if f.endswith(".rs"):
                        yield os.path.join(d, f)
        else:
            yield p


for path in walk(sys.argv[1:]):
    src = open(path, encoding="utf-8", newline="").read()
    if "egui" not in src:
        continue
    new = src
    for key, pat, rep in RULES:
        new, n = re.subn(pat, rep, new)
        if n:
            counts[key] = counts.get(key, 0) + n
    new, n = margin_ints(new)
    if n:
        counts["Margin ints"] = counts.get("Margin ints", 0) + n
    if new != src:
        open(path, "w", encoding="utf-8", newline="").write(new)
        counts["files"] = counts.get("files", 0) + 1

for k, v in sorted(counts.items()):
    print(f"{v:5} {k}")
