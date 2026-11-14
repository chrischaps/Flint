"""egui-wgpu 0.36: Renderer::new takes RendererOptions; textures_delta.set holds a list per id."""
import os
import re
import sys

counts = {}
CTOR = re.compile(
    r"egui_wgpu::Renderer::new\(\s*([^,]+?),\s*([^,]+?),\s*(None|Some\([^)]*\)),\s*(\d+),\s*(true|false),?\s*\)",
    re.S,
)
DELTA = re.compile(
    r"for \(id, (\w+)\) in &(\w+(?:\.\w+)*)\.textures_delta\.set \{(\s*)(\w+)\.update_texture\(([^;]*?), \*id, \1\);(\s*)\}",
    re.S,
)

for root in sys.argv[1:]:
    for d, _, fs in os.walk(root):
        for f in fs:
            if not f.endswith(".rs"):
                continue
            p = os.path.join(d, f)
            src = open(p, encoding="utf-8", newline="").read()
            new, n = CTOR.subn(
                lambda m: (
                    f"egui_wgpu::Renderer::new({m.group(1)}, {m.group(2)}, egui_wgpu::RendererOptions {{ "
                    f"depth_stencil_format: {m.group(3)}, msaa_samples: {m.group(4)}, dithering: {m.group(5)}, "
                    f"..Default::default() }})"
                ),
                src,
            )
            if n:
                counts["Renderer::new"] = counts.get("Renderer::new", 0) + n
            new, n = DELTA.subn(
                lambda m: (
                    f"for (id, deltas) in &{m.group(2)}.textures_delta.set {{{m.group(3)}"
                    f"for {m.group(1)} in deltas {{ {m.group(4)}.update_texture({m.group(5)}, *id, {m.group(1)}); }}{m.group(6)}}}"
                ),
                new,
            )
            if n:
                counts["textures_delta loop"] = counts.get("textures_delta loop", 0) + n
            if new != src:
                open(p, "w", encoding="utf-8", newline="").write(new)
print(counts)
