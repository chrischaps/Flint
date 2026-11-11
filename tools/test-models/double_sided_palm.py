"""Writes examples/models/palm_two_sided.glb and palm_one_sided.glb (#623).

The same palm twice: a crown of drooping fronds, each one strip of cards
wound once with its face up, as a game's generator would make them. One
GLB's frond material is glTF `"doubleSided": true`, the other's is left at
glTF's default. `examples/double_sided.scene.toml` stands them side by side and
looks up into the crowns, where every frond shows its underside.

Standard library only; run from anywhere:

  python tools/test-models/double_sided_palm.py
"""

import json
import math
import struct
from pathlib import Path

FRONDS = 11
SEGMENTS = 10
LENGTH = 3.2
WIDTH = 0.62
TOP = 3.6
FROND = [0.30, 0.52, 0.16, 1.0]
TRUNK = [0.36, 0.25, 0.16, 1.0]


def sub(a, b):
    return [a[i] - b[i] for i in range(3)]


def add(a, b):
    return [a[i] + b[i] for i in range(3)]


def scale(a, s):
    return [x * s for x in a]


def cross(a, b):
    return [a[1] * b[2] - a[2] * b[1], a[2] * b[0] - a[0] * b[2], a[0] * b[1] - a[1] * b[0]]


def dot(a, b):
    return sum(a[i] * b[i] for i in range(3))


def norm(a):
    n = math.sqrt(dot(a, a)) or 1.0
    return [x / n for x in a]


def frond_mesh():
    """Every frond's strip, wound so each triangle faces up."""
    positions, normals, indices = [], [], []
    for k in range(FRONDS):
        a = 2 * math.pi * k / FRONDS + 0.3 * math.sin(k * 2.1)
        d = [math.cos(a), 0.0, math.sin(a)]
        side = norm(cross([0.0, 1.0, 0.0], d))
        rise = 0.9 + 0.25 * math.sin(k * 1.7)
        droop = 2.3 + 0.4 * math.cos(k * 1.3)
        base = len(positions)
        for i in range(SEGMENTS + 1):
            t = i / SEGMENTS
            p = add([0.0, TOP, 0.0], add(scale(d, LENGTH * t), [0.0, rise * t - droop * t * t, 0.0]))
            tangent = norm(add(scale(d, LENGTH), [0.0, rise - 2 * droop * t, 0.0]))
            up = norm(cross(tangent, side))
            if up[1] < 0:
                up = scale(up, -1)
            w = WIDTH * math.sin(math.pi * min(t * 0.92 + 0.04, 1.0)) ** 0.8
            fold = 0.22 * w  # a shallow V along the rib
            for s in (-1, 1):
                positions.append(add(p, add(scale(side, s * w), scale(up, fold))))
                normals.append(norm(add(up, scale(side, -s * 0.35))))
        for i in range(SEGMENTS):
            l0, r0, l1, r1 = base + 2 * i, base + 2 * i + 1, base + 2 * i + 2, base + 2 * i + 3
            for tri in ((l0, l1, r1), (l0, r1, r0)):
                a_, b_, c_ = (positions[j] for j in tri)
                face = cross(sub(b_, a_), sub(c_, a_))
                n = normals[tri[0]]
                indices.extend(tri if dot(face, n) > 0 else (tri[0], tri[2], tri[1]))
    return positions, normals, indices


def trunk_mesh():
    """A tapering eight-sided trunk, closed faces out."""
    positions, normals, indices = [], [], []
    sides, rings = 8, 6
    for r in range(rings + 1):
        t = r / rings
        y = TOP * t
        radius = 0.17 * (1 - 0.35 * t)
        lean = 0.35 * t * t
        for s in range(sides + 1):
            a = 2 * math.pi * s / sides
            n = [math.cos(a), 0.0, math.sin(a)]
            positions.append([lean + radius * n[0], y, radius * n[2]])
            normals.append(n)
    for r in range(rings):
        for s in range(sides):
            i0 = r * (sides + 1) + s
            i1, i2, i3 = i0 + 1, i0 + sides + 1, i0 + sides + 2
            for tri in ((i0, i2, i1), (i1, i2, i3)):
                a_, b_, c_ = (positions[j] for j in tri)
                face = cross(sub(b_, a_), sub(c_, a_))
                indices.extend(tri if dot(face, normals[tri[0]]) > 0 else (tri[0], tri[2], tri[1]))
    return positions, normals, indices


def pad4(data: bytes, fill: bytes = b"\0") -> bytes:
    return data + fill * (-len(data) % 4)


def write_glb(path: Path, double_sided: bool) -> None:
    blob = bytearray()
    views, accessors, meshes, nodes = [], [], [], []

    def view(data: bytes, target: int) -> int:
        offset = len(blob)
        blob.extend(pad4(data))
        views.append({"buffer": 0, "byteOffset": offset, "byteLength": len(data), "target": target})
        return len(views) - 1

    parts = [("trunk", trunk_mesh(), 0), ("fronds", frond_mesh(), 1)]
    for name, (positions, normals, indices), material in parts:
        first = len(accessors)
        lo = [min(p[i] for p in positions) for i in range(3)]
        hi = [max(p[i] for p in positions) for i in range(3)]
        pos = b"".join(struct.pack("<3f", *p) for p in positions)
        nrm = b"".join(struct.pack("<3f", *n) for n in normals)
        idx = struct.pack(f"<{len(indices)}I", *indices)
        accessors.append({"bufferView": view(pos, 34962), "componentType": 5126,
                          "count": len(positions), "type": "VEC3", "min": lo, "max": hi})
        accessors.append({"bufferView": view(nrm, 34962), "componentType": 5126,
                          "count": len(normals), "type": "VEC3"})
        accessors.append({"bufferView": view(idx, 34963), "componentType": 5125,
                          "count": len(indices), "type": "SCALAR"})
        meshes.append({"name": name, "primitives": [{
            "attributes": {"POSITION": first, "NORMAL": first + 1},
            "indices": first + 2, "material": material}]})
        nodes.append({"name": name, "mesh": len(meshes) - 1})

    frond = {"name": "frond", "pbrMetallicRoughness": {
        "baseColorFactor": FROND, "metallicFactor": 0.0, "roughnessFactor": 0.8}}
    if double_sided:
        frond["doubleSided"] = True
    gltf = {
        "asset": {"version": "2.0", "generator": "flint tools/test-models/double_sided_palm.py"},
        "scene": 0,
        "scenes": [{"nodes": list(range(len(nodes)))}],
        "nodes": nodes,
        "meshes": meshes,
        "materials": [
            {"name": "trunk", "pbrMetallicRoughness": {
                "baseColorFactor": TRUNK, "metallicFactor": 0.0, "roughnessFactor": 0.95}},
            frond,
        ],
        "accessors": accessors,
        "bufferViews": views,
        "buffers": [{"byteLength": len(blob)}],
    }
    js = pad4(json.dumps(gltf, separators=(",", ":")).encode(), b" ")
    bin_chunk = pad4(bytes(blob))
    total = 12 + 8 + len(js) + 8 + len(bin_chunk)
    path.write_bytes(
        struct.pack("<4sII", b"glTF", 2, total)
        + struct.pack("<I4s", len(js), b"JSON") + js
        + struct.pack("<I4s", len(bin_chunk), b"BIN\0") + bin_chunk
    )
    print(f"wrote {path} ({total} bytes)")


def main() -> None:
    models = Path(__file__).resolve().parents[2] / "examples" / "models"
    write_glb(models / "palm_two_sided.glb", True)
    write_glb(models / "palm_one_sided.glb", False)


if __name__ == "__main__":
    main()
