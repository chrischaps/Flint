"""Mechanical wgpu 23 -> 30 renames for the #934 spike. Run with the repo root as argv[1]."""
import os
import re
import sys

root = sys.argv[1]
counts = {}
EOL = r"(?=\r?$)"


def bump(key, n):
    if n:
        counts[key] = counts.get(key, 0) + n


SIMPLE = [
    ("multiview: None", r"\bmultiview: None,", "multiview_mask: None,"),
    ("push_constant_ranges", r"\bpush_constant_ranges: &\[\],", "immediate_size: 0,"),
    ("depth_write_enabled", r"\bdepth_write_enabled: (true|false|depth_write),", r"depth_write_enabled: Some(\1),"),
    ("depth_write_enabled shorthand", r"^([ \t]*)depth_write_enabled," + EOL, r"\1depth_write_enabled: Some(depth_write_enabled),"),
    ("depth_compare", r"\bdepth_compare: (wgpu::CompareFunction::\w+),", r"depth_compare: Some(\1),"),
    ("ImageDataLayout", r"\bwgpu::ImageDataLayout\b", "wgpu::TexelCopyBufferLayout"),
    ("ImageCopyTexture", r"\bwgpu::ImageCopyTexture\b", "wgpu::TexelCopyTextureInfo"),
    ("ImageCopyBuffer", r"\bwgpu::ImageCopyBuffer\b", "wgpu::TexelCopyBufferInfo"),
    ("Maintain::Wait", r"\bwgpu::Maintain::Wait\b", "wgpu::PollType::wait_indefinitely()"),
    ("Maintain::Poll", r"\bwgpu::Maintain::Poll\b", "wgpu::PollType::Poll"),
    ("mipmap_filter", r"\bmipmap_filter: wgpu::FilterMode::", "mipmap_filter: wgpu::MipmapFilterMode::"),
    ("get_mapped_range()", r"\.get_mapped_range\(\)", '.get_mapped_range().expect("mapped buffer range")'),
    ("vertex buffers desc()", r"\bbuffers: &\[(\w+::desc\(\))\]", r"buffers: &[Some(\1)]"),
    ("vertex buffers [buffer]", r"\bbuffers: &\[buffer\]", "buffers: &[Some(buffer)]"),
]

# The two struct fields every render pass and colour attachment gained.
ADD_AFTER = [
    ("depth_slice", re.compile(r"^([ \t]*)resolve_target: [^\r\n]*," + EOL, re.M), "depth_slice: None,"),
    ("multiview_mask (pass)", re.compile(r"^([ \t]*)occlusion_query_set: [^\r\n]*," + EOL, re.M), "multiview_mask: None,"),
]


def wrap_elems(body):
    """Wrap each element of an array literal body in Some(...)."""
    if "\n" not in body:
        parts = [p.strip() for p in body.split(",")]
        parts = [p for p in parts if p]
        return ", ".join(p if p.startswith("Some(") else f"Some({p})" for p in parts)
    out = []
    for line in body.split("\n"):
        cr = "\r" if line.endswith("\r") else ""
        line = line.rstrip("\r")
        m = re.match(r"^(\s*)([^/\s][^,/]*?)(,?)(\s*//.*)?$", line)
        if m and m.group(2).strip() and not m.group(2).startswith("Some("):
            out.append(f"{m.group(1)}Some({m.group(2).strip()}){m.group(3)}{m.group(4) or ''}{cr}")
        else:
            out.append(line + cr)
    return "\n".join(out)


BGL = re.compile(r"bind_group_layouts: &\[(?!\])(.*?)\]", re.S)

for dirpath, _, files in os.walk(os.path.join(root, "crates")):
    if "target" in dirpath.split(os.sep):
        continue
    for f in files:
        if not f.endswith(".rs"):
            continue
        p = os.path.join(dirpath, f)
        with open(p, encoding="utf-8", newline="") as fh:
            src = fh.read()
        nl = "\r\n" if "\r\n" in src else "\n"
        new = src
        for key, pat, rep in SIMPLE:
            new, n = re.subn(pat, rep, new, flags=re.M)
            bump(key, n)
        for key, pat, field in ADD_AFTER:
            new, n = pat.subn(lambda m: f"{m.group(0)}{nl}{m.group(1)}{field}", new)
            bump(key, n)
        new, n = BGL.subn(lambda m: "bind_group_layouts: &[" + wrap_elems(m.group(1)) + "]", new)
        bump("bind_group_layouts literal", n)
        if new != src:
            with open(p, "w", encoding="utf-8", newline="") as fh:
                fh.write(new)
            bump("files changed", 1)

for k, v in sorted(counts.items()):
    print(f"{v:5} {k}")
