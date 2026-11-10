"""Write demo/shear_road.scene.toml (and, with --yaw, the yawed "before").

A scanline racer's road (#625): tiles of 8 m laid down the screen, a curve
being each tile pushed sideways by its distance (x += dx; dx += curve) and a
hill each one pushed up the same way. With `transform.shear` every tile is
one entity whose near edge meets the last tile's far edge exactly, so the
road's edges, lane lines and the wide ground stay continuous. With --yaw the
same road is laid by turning each tile instead: wide tiles swing their far
corners through their neighbours, which is why the issue asked for shear.

    python demo/tools/shear_road.py            # demo/shear_road.scene.toml
    python demo/tools/shear_road.py --yaw      # demo/shear_road_yawed.scene.toml
"""

import math
import sys
from pathlib import Path

TILES = 22
LENGTH = 8.0  # metres per tile, along -z
GROUND_W = 120.0
ROAD_W = 10.0
CURB_W = 0.7
LINE_W = 0.25


def road():
    """(x, y) of each tile's near edge, TILES + 1 of them."""
    x, dx, y, dy = 0.0, 0.0, 0.0, 0.0
    out = [(x, y)]
    for i in range(TILES):
        # a bend to the right that eases out, then back left
        curve = 0.34 if i < 9 else (-0.42 if i < 18 else 0.0)
        hill = 0.06 if 4 <= i < 11 else (-0.07 if 11 <= i < 17 else 0.0)
        dx += curve
        dy += hill
        x += dx
        y += dy
        out.append((x, y))
    return out


def lerp(a, b, t):
    return tuple(a[i] + (b[i] - a[i]) * t for i in range(len(a)))


def fmt(v):
    return "[" + ", ".join(f"{round(c, 4):g}" for c in v) + "]"


def main():
    yaw = "--yaw" in sys.argv
    pts = road()
    lines = []
    w = lines.append
    title = "Shear Road (yawed tiles, the before)" if yaw else "Shear Road"
    w("# ================================================================")
    w(f"#  {title.upper()} (#625)")
    w("# ================================================================")
    w("#")
    w("#  A scanline racer's road: 8 m tiles, each pushed sideways and up by")
    w("#  its distance (x += dx; dx += curve). Written by")
    w("#  demo/tools/shear_road.py; edit that, not this.")
    w("#")
    if yaw:
        w("#  Each tile is YAWED to point at the next one, the old way: the")
        w("#  120 m ground and the road's edges swing their far corners through")
        w("#  the neighbouring tiles. Compare demo/shear_road.scene.toml.")
    else:
        w("#  Each tile is ONE entity with transform.shear = { xz, yz }: x and y")
        w("#  move with z, so a tile's near edge lands exactly on the last one's")
        w("#  far edge and every stripe runs on unbroken. Road, curbs and lane")
        w("#  line are children of the ground tile and inherit its shear.")
        w("#  scripts/shear_road.rhai re-lays it every frame with set_shear,")
        w("#  the bend breathing right to left and back.")
    w("#")
    w("#  Render:  cargo run --bin flint -- render demo/shear_road.scene.toml \\")
    w("#             --output renders/shear_road.png --schemas schemas --no-grid")
    if not yaw:
        w("#  Moving:  ... --scripts --time 0..10.5 --fps 12 -o renders/shear_road.gif")
    w("#  Play:    cargo run --bin flint -- play demo/shear_road.scene.toml")
    w("# ================================================================")
    w("")
    w("[scene]")
    w(f'name = "{title}"')
    w('version = "1.0"')
    w('description = "Scanline road of sheared tiles: one entity per tile, edges continuous through the curve (#625)"')
    w("")
    if not yaw:
        w("[entities.road_driver]")
        w("[entities.road_driver.script]")
        w('source = "shear_road.rhai"')
        w("")
    w("[camera]")
    w("position = [0, 3.2, -1]")
    w("target = [7, 1.6, -60]")
    w("fov = 55.0")
    w("")
    w("[post_process]")
    w("bloom_enabled = true")
    w("bloom_intensity = 0.06")
    w("fog_enabled = true")
    w("fog_density = 0.0055")
    w("fog_color = [0.98, 0.62, 0.48]")
    w("fog_height_falloff = 0.05")
    w("vignette_enabled = true")
    w("vignette_intensity = 0.3")
    w("")
    w("[entities.sky]")
    w("[entities.sky.sky]")
    w("zenith_color = [0.16, 0.12, 0.34, 1.0]")
    w("horizon_color = [1.0, 0.56, 0.42, 1.0]")
    w("haze_color = [1.0, 0.66, 0.50, 0.7]")
    w("cloud_coverage = 0.1")
    w("sun_glow = 0.8")
    w("")
    w("[entities.sun]")
    w("[entities.sun.transform]")
    w("position = [0, 50, 0]")
    w("[entities.sun.light]")
    w('type = "directional"')
    w("direction = [0.35, 0.45, -0.82]")
    w("color = [1.0, 0.78, 0.62]")
    w("intensity = 1.5")
    w("")

    # ground bands alternate light/dark, curbs red/white: the strips
    ground = [(0.30, 0.52, 0.30), (0.25, 0.45, 0.26)]
    asphalt = [(0.30, 0.29, 0.33), (0.27, 0.26, 0.30)]
    curb = [(0.86, 0.20, 0.22), (0.95, 0.93, 0.90)]
    for i in range(TILES):
        (x0, y0), (x1, y1) = pts[i], pts[i + 1]
        cx, cy = (x0 + x1) / 2, (y0 + y1) / 2
        cz = -(i + 0.5) * LENGTH
        # local z runs from +4 (near) to -4 (far): x' = x + xz*z, so the far
        # end (z = -4) moves by -4*xz = (x1 - x0) / 2 from the centre.
        xz = -(x1 - x0) / LENGTH
        yz = -(y1 - y0) / LENGTH
        name = f"tile_{i:02d}"
        band = i % 2
        w(f"[entities.{name}]")
        w(f"[entities.{name}.transform]")
        w(f"position = {fmt((cx, cy - 0.05, cz))}")
        if yaw:
            # turn the tile to face the next, and stretch it to reach
            yaw_deg = math.degrees(math.atan2(x1 - x0, LENGTH))
            pitch_deg = math.degrees(math.atan2(y1 - y0, LENGTH))
            w(f"rotation = {fmt((pitch_deg, -yaw_deg, 0.0))}")
        else:
            w(f"shear = {{ xz = {xz + 0.0:.5f}, yz = {yz + 0.0:.5f} }}")
        w(f"[entities.{name}.bounds]")
        w(f"min = {fmt((-GROUND_W / 2, -0.05, -LENGTH / 2))}")
        w(f"max = {fmt((GROUND_W / 2, 0.0, LENGTH / 2))}")
        w(f"[entities.{name}.material]")
        w(f"color = {fmt(ground[band] + (1.0,))}")
        w("roughness = 0.95")
        w("")

        def child(suffix, x, width, color, height, depth=LENGTH):
            n = f"{name}_{suffix}"
            w(f"[entities.{n}]")
            w(f'parent = "{name}"')
            w(f"[entities.{n}.transform]")
            w(f"position = {fmt((x, 0.0, 0.0))}")
            w(f"[entities.{n}.bounds]")
            w(f"min = {fmt((-width / 2, 0.0, -depth / 2))}")
            w(f"max = {fmt((width / 2, height, depth / 2))}")
            w(f"[entities.{n}.material]")
            w(f"color = {fmt(color + (1.0,))}")
            w("roughness = 0.7")
            w("")

        child("road", 0.0, ROAD_W, asphalt[band], 0.02)
        child("curb_l", -ROAD_W / 2 - CURB_W / 2, CURB_W, curb[band], 0.06)
        child("curb_r", ROAD_W / 2 + CURB_W / 2, CURB_W, curb[band], 0.06)
        if band == 0:
            child("line", 0.0, LINE_W, (0.98, 0.92, 0.70), 0.03)

    out = Path(__file__).resolve().parent.parent / (
        "shear_road_yawed.scene.toml" if yaw else "shear_road.scene.toml"
    )
    out.write_text("\n".join(lines) + "\n", encoding="utf-8", newline="\n")
    print(out)


if __name__ == "__main__":
    main()
