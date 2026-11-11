"""Write a UV sphere as a self-contained GLB: examples/models/Sphere.glb.

    python -I tools/make_sphere_glb.py [out.glb] [--rings 32] [--segments 48]

Radius 0.5 (a 1 m sphere, like Box.glb is a 1 m cube), smooth normals, one
plain material. Made for the ASCII edge tests (#267), which needed a
silhouette with every direction in it; no dependencies beyond the standard
library, so it runs anywhere.
"""
import json
import math
import struct
import sys


def sphere(rings, segments, radius=0.5):
    pos, nrm, idx = [], [], []
    for r in range(rings + 1):
        v = r / rings
        phi = v * math.pi
        for s in range(segments + 1):
            u = s / segments
            theta = u * 2.0 * math.pi
            n = (math.sin(phi) * math.cos(theta), math.cos(phi), math.sin(phi) * math.sin(theta))
            nrm.append(n)
            pos.append(tuple(radius * c for c in n))
    row = segments + 1
    for r in range(rings):
        for s in range(segments):
            a = r * row + s
            b = a + row
            # Counter-clockwise seen from outside
            idx += [a, a + 1, b, a + 1, b + 1, b]
    return pos, nrm, idx


def glb(pos, nrm, idx):
    def pack(fmt, items):
        return b"".join(struct.pack(fmt, *i) if isinstance(i, tuple) else struct.pack(fmt, i) for i in items)

    p = pack("<3f", pos)
    n = pack("<3f", nrm)
    i = pack("<I", idx)
    blob = p + n + i
    blob += b"\0" * (-len(blob) % 4)
    lo = [min(c[k] for c in pos) for k in range(3)]
    hi = [max(c[k] for c in pos) for k in range(3)]
    doc = {
        "asset": {"version": "2.0", "generator": "flint tools/make_sphere_glb.py"},
        "scene": 0,
        "scenes": [{"nodes": [0]}],
        "nodes": [{"mesh": 0, "name": "Sphere"}],
        "meshes": [{"name": "Sphere", "primitives": [{
            "attributes": {"POSITION": 0, "NORMAL": 1}, "indices": 2, "material": 0}]}],
        "materials": [{"name": "Sphere", "pbrMetallicRoughness": {
            "baseColorFactor": [0.8, 0.8, 0.8, 1.0], "metallicFactor": 0.0, "roughnessFactor": 0.6}}],
        "buffers": [{"byteLength": len(blob)}],
        "bufferViews": [
            {"buffer": 0, "byteOffset": 0, "byteLength": len(p), "target": 34962},
            {"buffer": 0, "byteOffset": len(p), "byteLength": len(n), "target": 34962},
            {"buffer": 0, "byteOffset": len(p) + len(n), "byteLength": len(i), "target": 34963},
        ],
        "accessors": [
            {"bufferView": 0, "componentType": 5126, "count": len(pos), "type": "VEC3", "min": lo, "max": hi},
            {"bufferView": 1, "componentType": 5126, "count": len(nrm), "type": "VEC3"},
            {"bufferView": 2, "componentType": 5125, "count": len(idx), "type": "SCALAR"},
        ],
    }
    js = json.dumps(doc, separators=(",", ":")).encode()
    js += b" " * (-len(js) % 4)
    body = struct.pack("<II", len(js), 0x4E4F534A) + js + struct.pack("<II", len(blob), 0x004E4942) + blob
    return struct.pack("<III", 0x46546C67, 2, 12 + len(body)) + body


def main(argv):
    out, rings, segments = "examples/models/Sphere.glb", 32, 48
    args = list(argv)
    while args:
        a = args.pop(0)
        if a == "--rings":
            rings = int(args.pop(0))
        elif a == "--segments":
            segments = int(args.pop(0))
        else:
            out = a
    data = glb(*sphere(rings, segments))
    with open(out, "wb") as f:
        f.write(data)
    print(f"wrote {out}: {len(data)} bytes, {rings}x{segments}")


if __name__ == "__main__":
    main(sys.argv[1:])
