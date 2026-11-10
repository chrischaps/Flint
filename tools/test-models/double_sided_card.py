"""Writes testModels/DoubleSidedCard.glb, the fixture for glTF doubleSided (#623).

Two flat 2 m cards side by side in the XZ plane, each wound once with its
normal up (+Y):

  card_two_sided   x = -1.25, material "two_sided" with "doubleSided": true
  card_one_sided   x = +1.25, material "one_sided" (glTF's default, false)

Seen from above both draw. Seen from below only the double-sided one should.
Standard library only; run from anywhere:

  python tools/test-models/double_sided_card.py
"""

import json
import struct
from pathlib import Path

HALF = 1.0
CARDS = [
    ("card_two_sided", "two_sided", -1.25, [0.9, 0.55, 0.2, 1.0], True),
    ("card_one_sided", "one_sided", 1.25, [0.25, 0.55, 0.9, 1.0], False),
]


def pad4(data: bytes, fill: bytes = b"\0") -> bytes:
    return data + fill * (-len(data) % 4)


def main() -> None:
    blob = bytearray()
    views, accessors, meshes, nodes, materials = [], [], [], [], []

    def add_view(data: bytes, target: int) -> int:
        offset = len(blob)
        blob.extend(pad4(data))
        views.append({"buffer": 0, "byteOffset": offset, "byteLength": len(data), "target": target})
        return len(views) - 1

    for i, (node, material, x, color, double_sided) in enumerate(CARDS):
        # (x, z) corners; indices 0 1 2, 0 2 3 face +Y (counter-clockwise from above)
        corners = [(-HALF, -HALF), (-HALF, HALF), (HALF, HALF), (HALF, -HALF)]
        # Offsets baked into the vertices: the renderer uploads meshes, not nodes
        positions = b"".join(struct.pack("<3f", x + cx, 0.0, cz) for cx, cz in corners)
        normals = struct.pack("<3f", 0.0, 1.0, 0.0) * 4
        indices = struct.pack("<6H", 0, 1, 2, 0, 2, 3)

        pos_view = add_view(positions, 34962)
        accessors.append({
            "bufferView": pos_view, "componentType": 5126, "count": 4, "type": "VEC3",
            "min": [x - HALF, 0.0, -HALF], "max": [x + HALF, 0.0, HALF],
        })
        nrm_view = add_view(normals, 34962)
        accessors.append({"bufferView": nrm_view, "componentType": 5126, "count": 4, "type": "VEC3"})
        idx_view = add_view(indices, 34963)
        accessors.append({"bufferView": idx_view, "componentType": 5123, "count": 6, "type": "SCALAR"})

        mat = {
            "name": material,
            "pbrMetallicRoughness": {
                "baseColorFactor": color, "metallicFactor": 0.0, "roughnessFactor": 1.0,
            },
        }
        if double_sided:
            mat["doubleSided"] = True
        materials.append(mat)
        meshes.append({
            "name": node,
            "primitives": [{
                "attributes": {"POSITION": 3 * i, "NORMAL": 3 * i + 1},
                "indices": 3 * i + 2,
                "material": i,
            }],
        })
        nodes.append({"name": node, "mesh": i})

    gltf = {
        "asset": {"version": "2.0", "generator": "flint tools/test-models/double_sided_card.py"},
        "scene": 0,
        "scenes": [{"nodes": list(range(len(nodes)))}],
        "nodes": nodes,
        "meshes": meshes,
        "materials": materials,
        "accessors": accessors,
        "bufferViews": views,
        "buffers": [{"byteLength": len(blob)}],
    }
    js = pad4(json.dumps(gltf, separators=(",", ":")).encode(), b" ")
    bin_chunk = pad4(bytes(blob))
    total = 12 + 8 + len(js) + 8 + len(bin_chunk)
    out = Path(__file__).resolve().parents[2] / "testModels" / "DoubleSidedCard.glb"
    out.write_bytes(
        struct.pack("<4sII", b"glTF", 2, total)
        + struct.pack("<I4s", len(js), b"JSON") + js
        + struct.pack("<I4s", len(bin_chunk), b"BIN\0") + bin_chunk
    )
    print(f"wrote {out} ({total} bytes)")


if __name__ == "__main__":
    main()
