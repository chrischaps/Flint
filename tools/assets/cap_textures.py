"""Bring textures under the repository's caps (#759, ADR 0121).

    python tools/assets/cap_textures.py flint-examples/models/wooden_bench.glb
    python tools/assets/cap_textures.py --colour flint-examples/textures/wood_planks.png
    python tools/assets/cap_textures.py --dry-run flint-examples/models/*.glb

The caps are `scripts/size-check.py`'s: a texture is at most `--side` (2048)
texels on its longer side and `--bytes` (3 MB) encoded. A texture already
under both is left alone, byte for byte.

**Halving.** A texture over the side cap is halved until it fits, each step a
2x2 box filter in linear light for colour and as stored for data, which is
exactly what `flint_pixels::mip_chain` does on load. So the halved texture is
the renderer's own next mip level, up to one rounding: a frame that sampled
level 1 or below of the old one samples nearly the same texels now.

**Encoding.** A colour map (glTF base colour or emissive; a loose file given
`--colour`) is written as JPEG, quality 92 with no chroma subsampling, and
loses its alpha only when every material using it is `OPAQUE`, where glTF
says alpha is ignored. A data map (normal, metallic-roughness, occlusion; a
loose file given `--data`) stays PNG, since JPEG's blocks show in lighting.
A loose JPEG stays a JPEG, a loose PNG that becomes a JPEG gets a `.jpg`
beside it and the PNG is removed: rename the scene's references
(`texture = "wood_planks.jpg"`).

**GLBs** are rewritten whole: the JSON chunk's image `mimeType`s and buffer
view offsets, then the BIN chunk with every view at a 4-byte boundary, as
glTF asks. Nothing else in the file changes. `cargo test -p flint-import
--test repo_models` imports every checked-in GLB afterwards.

Needs numpy and Pillow (`tools/requirements.txt`).
"""

from __future__ import annotations

import argparse
import io
import json
import struct
import sys
from pathlib import Path

import numpy as np
from PIL import Image

Image.MAX_IMAGE_PIXELS = None

JPEG_QUALITY = 92


def to_linear(c: np.ndarray) -> np.ndarray:
    c = c / 255.0
    return np.where(c <= 0.04045, c / 12.92, ((c + 0.055) / 1.055) ** 2.4)


def to_srgb(v: np.ndarray) -> np.ndarray:
    v = np.clip(v, 0.0, 1.0)
    return np.where(v <= 0.0031308, v * 12.92, 1.055 * v ** (1 / 2.4) - 0.055)


def halve_until(img: Image.Image, side: int, srgb: bool) -> Image.Image:
    """`flint_pixels::mip_chain`'s levels, until the longer side fits."""
    if max(img.size) <= side:
        return img
    mode = "RGBA" if "A" in img.getbands() else "RGB"
    a = np.asarray(img.convert(mode), dtype=np.float64)
    f = a / 255.0
    if srgb:
        f[..., :3] = to_linear(a[..., :3])
    h, w = f.shape[:2]
    while max(w, h) > side:
        nw, nh = max(w // 2, 1), max(h // 2, 1)
        ys0 = np.minimum(2 * np.arange(nh), h - 1)
        ys1 = np.minimum(2 * np.arange(nh) + 1, h - 1)
        xs0 = np.minimum(2 * np.arange(nw), w - 1)
        xs1 = np.minimum(2 * np.arange(nw) + 1, w - 1)
        f = 0.25 * (
            f[ys0][:, xs0] + f[ys0][:, xs1] + f[ys1][:, xs0] + f[ys1][:, xs1]
        )
        w, h = nw, nh
    out = f.copy()
    if srgb:
        out[..., :3] = to_srgb(f[..., :3])
    return Image.fromarray((np.clip(out, 0, 1) * 255.0 + 0.5).astype(np.uint8), mode)


def encode(img: Image.Image, as_jpeg: bool, keep_alpha: bool) -> tuple[bytes, str]:
    buf = io.BytesIO()
    if as_jpeg and not (keep_alpha and "A" in img.getbands()):
        img.convert("RGB").save(buf, "JPEG", quality=JPEG_QUALITY, subsampling=0, optimize=True)
        return buf.getvalue(), "image/jpeg"
    img.save(buf, "PNG", optimize=True)
    return buf.getvalue(), "image/png"


def over(img: Image.Image, size: int, side: int, cap: int) -> bool:
    return max(img.size) > side or size > cap


def cap_image(data: bytes, colour: bool, opaque: bool, side: int, cap: int) -> tuple[bytes, str] | None:
    """New bytes and MIME type, or None when the image is under both caps."""
    img = Image.open(io.BytesIO(data))
    if not over(img, len(data), side, cap):
        return None
    was_jpeg = img.format == "JPEG"
    img.load()
    small = halve_until(img, side, srgb=colour)
    return encode(small, as_jpeg=colour or was_jpeg, keep_alpha=not opaque)


def read_glb(path: Path) -> tuple[dict, bytes]:
    data = path.read_bytes()
    magic, version, _ = struct.unpack_from("<4sII", data, 0)
    if magic != b"glTF" or version != 2:
        raise SystemExit(f"{path}: not a glTF 2 binary")
    off, js, binc = 12, None, b""
    while off + 8 <= len(data):
        clen, ctype = struct.unpack_from("<I4s", data, off)
        chunk = data[off + 8 : off + 8 + clen]
        if ctype == b"JSON":
            js = json.loads(chunk)
        elif ctype == b"BIN\0":
            binc = chunk
        off += 8 + clen
    return js, binc


def write_glb(path: Path, js: dict, binc: bytes) -> None:
    text = json.dumps(js, separators=(",", ":")).encode()
    text += b" " * (-len(text) % 4)
    binc += b"\0" * (-len(binc) % 4)
    total = 12 + 8 + len(text) + 8 + len(binc)
    path.write_bytes(
        struct.pack("<4sII", b"glTF", 2, total)
        + struct.pack("<I4s", len(text), b"JSON")
        + text
        + struct.pack("<I4s", len(binc), b"BIN\0")
        + binc
    )


def image_roles(js: dict) -> dict[int, tuple[bool, bool]]:
    """Image index -> (is colour, every user opaque)."""
    tex_image = {i: t.get("source") for i, t in enumerate(js.get("textures", []))}
    roles: dict[int, list] = {}
    for m in js.get("materials", []):
        opaque = m.get("alphaMode", "OPAQUE") == "OPAQUE"
        pbr = m.get("pbrMetallicRoughness", {})
        slots = [
            (pbr.get("baseColorTexture"), True),
            (m.get("emissiveTexture"), True),
            (pbr.get("metallicRoughnessTexture"), False),
            (m.get("normalTexture"), False),
            (m.get("occlusionTexture"), False),
        ]
        for slot, colour in slots:
            if slot is None or tex_image.get(slot["index"]) is None:
                continue
            r = roles.setdefault(tex_image[slot["index"]], [False, True])
            r[0] = r[0] or colour
            r[1] = r[1] and opaque
    return {k: (v[0], v[1]) for k, v in roles.items()}


def cap_glb(path: Path, side: int, cap: int, dry: bool) -> int:
    js, binc = read_glb(path)
    roles = image_roles(js)
    views = js.get("bufferViews", [])
    new_view_bytes: dict[int, bytes] = {}
    for i, im in enumerate(js.get("images", [])):
        if "bufferView" not in im:
            continue
        v = views[im["bufferView"]]
        o = v.get("byteOffset", 0)
        old = binc[o : o + v["byteLength"]]
        colour, opaque = roles.get(i, (False, True))
        got = cap_image(old, colour, opaque, side, cap)
        if got is None:
            continue
        new, mime = got
        print(f"  {path} image {i} {im.get('name', '')!r} ({'colour' if colour else 'data'}): "
              f"{len(old) / 1e6:.2f} -> {len(new) / 1e6:.2f} MB, {mime}")
        new_view_bytes[im["bufferView"]] = new
        im["mimeType"] = mime
    if not new_view_bytes or dry:
        return len(new_view_bytes)
    if len(js.get("buffers", [])) != 1 or "uri" in js["buffers"][0]:
        raise SystemExit(f"{path}: expected one embedded buffer")
    out = bytearray()
    for vi, v in enumerate(views):
        if v.get("buffer", 0) != 0:
            raise SystemExit(f"{path}: view {vi} is in another buffer")
        o = v.get("byteOffset", 0)
        body = new_view_bytes.get(vi, binc[o : o + v["byteLength"]])
        out += b"\0" * (-len(out) % 4)
        v["byteOffset"] = len(out)
        v["byteLength"] = len(body)
        out += body
    js["buffers"][0]["byteLength"] = len(out)
    write_glb(path, js, bytes(out))
    print(f"  {path}: {path.stat().st_size / 1e6:.2f} MB")
    return len(new_view_bytes)


def cap_loose(path: Path, colour: bool, side: int, cap: int, dry: bool) -> int:
    data = path.read_bytes()
    got = cap_image(data, colour, opaque=True, side=side, cap=cap)
    if got is None:
        return 0
    new, mime = got
    dest = path.with_suffix(".jpg") if mime == "image/jpeg" and path.suffix.lower() == ".png" else path
    print(f"  {path} -> {dest}: {len(data) / 1e6:.2f} -> {len(new) / 1e6:.2f} MB")
    if not dry:
        dest.write_bytes(new)
        if dest != path:
            path.unlink()
    return 1


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    ap.add_argument("files", nargs="+", type=Path)
    kind = ap.add_mutually_exclusive_group()
    kind.add_argument("--colour", action="store_true", help="loose files are colour maps (JPEG, linear-light filter)")
    kind.add_argument("--data", action="store_true", help="loose files are data maps (PNG, filtered as stored)")
    ap.add_argument("--side", type=int, default=2048)
    ap.add_argument("--bytes", type=int, default=3_000_000)
    ap.add_argument("--dry-run", action="store_true")
    a = ap.parse_args()
    changed = 0
    for f in a.files:
        if f.suffix.lower() == ".glb":
            changed += cap_glb(f, a.side, a.bytes, a.dry_run)
        else:
            if not (a.colour or a.data):
                print(f"{f}: say --colour or --data for a loose texture", file=sys.stderr)
                return 2
            changed += cap_loose(f, a.colour, a.side, a.bytes, a.dry_run)
    print(f"{changed} texture(s) {'would change' if a.dry_run else 'changed'}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
