"""Generate 10 seamless tiling textures (128x128) for E1M1 Hangar level."""
from PIL import Image, ImageDraw, ImageFilter
import random
import os

OUT = os.path.join(os.path.dirname(__file__), "textures")
SIZE = 128
os.makedirs(OUT, exist_ok=True)


def gen_brown_brick():
    """Warm brown brick pattern for start room walls."""
    img = Image.new('RGB', (SIZE, SIZE), (120, 70, 40))
    draw = ImageDraw.Draw(img)
    brick_h, brick_w = 16, 32
    mortar = (60, 40, 25)

    for row in range(SIZE // brick_h + 1):
        y = row * brick_h
        draw.rectangle([0, y, SIZE, y + 1], fill=mortar)
        offset = (brick_w // 2) if row % 2 == 1 else 0
        for col in range(-1, SIZE // brick_w + 2):
            x = col * brick_w + offset
            draw.rectangle([x, y, x + 1, y + brick_h], fill=mortar)

    pixels = img.load()
    for y in range(SIZE):
        for x in range(SIZE):
            r, g, b = pixels[x, y]
            if (r, g, b) != mortar:
                random.seed((y // brick_h) * 10 + (x // brick_w) + 101)
                bv = random.randint(-20, 20)
                random.seed(y * SIZE + x + 42)
                pv = random.randint(-8, 8)
                pixels[x, y] = (
                    max(0, min(255, r + bv + pv)),
                    max(0, min(255, g + bv // 2 + pv)),
                    max(0, min(255, b + bv // 3 + pv)),
                )

    img.save(os.path.join(OUT, "brown_brick.png"))
    print("  brown_brick.png")


def gen_brown_stone_floor():
    """Brown stone/dirt floor for start room."""
    img = Image.new('RGB', (SIZE, SIZE), (100, 65, 35))
    pixels = img.load()
    random.seed(202)
    for y in range(SIZE):
        for x in range(SIZE):
            v = random.randint(-20, 20)
            r, g, b = pixels[x, y]
            pixels[x, y] = (
                max(0, min(255, r + v)),
                max(0, min(255, g + v)),
                max(0, min(255, b + int(v * 0.6))),
            )
    draw = ImageDraw.Draw(img)
    for i in range(SIZE // 32 + 1):
        pos = i * 32
        draw.line([(pos, 0), (pos, SIZE)], fill=(70, 45, 25), width=1)
        draw.line([(0, pos), (SIZE, pos)], fill=(70, 45, 25), width=1)

    img = img.filter(ImageFilter.SMOOTH)
    img.save(os.path.join(OUT, "brown_stone_floor.png"))
    print("  brown_stone_floor.png")


def gen_gray_concrete():
    """Gray concrete with subtle cracks for hallway walls."""
    img = Image.new('RGB', (SIZE, SIZE), (110, 110, 105))
    pixels = img.load()
    random.seed(303)
    for y in range(SIZE):
        for x in range(SIZE):
            v = random.randint(-15, 15)
            r, g, b = pixels[x, y]
            pixels[x, y] = (
                max(0, min(255, r + v)),
                max(0, min(255, g + v)),
                max(0, min(255, b + v)),
            )
    draw = ImageDraw.Draw(img)
    crack_color = (75, 75, 72)
    random.seed(304)
    cx, cy = 20, 0
    for _ in range(60):
        nx = cx + random.randint(-1, 2)
        ny = cy + random.randint(1, 3)
        draw.line([(cx % SIZE, cy % SIZE), (nx % SIZE, ny % SIZE)], fill=crack_color, width=1)
        cx, cy = nx, ny
    cx, cy = 90, 30
    for _ in range(40):
        nx = cx + random.randint(-2, 1)
        ny = cy + random.randint(1, 3)
        draw.line([(cx % SIZE, cy % SIZE), (nx % SIZE, ny % SIZE)], fill=crack_color, width=1)
        cx, cy = nx, ny

    img = img.filter(ImageFilter.SMOOTH_MORE)
    img.save(os.path.join(OUT, "gray_concrete.png"))
    print("  gray_concrete.png")


def gen_gray_tile_floor():
    """Gray industrial tile for hallway floor."""
    img = Image.new('RGB', (SIZE, SIZE), (95, 100, 100))
    draw = ImageDraw.Draw(img)
    tile_size = 32
    groove = (65, 68, 70)
    for i in range(SIZE // tile_size + 1):
        pos = i * tile_size
        draw.line([(pos, 0), (pos, SIZE)], fill=groove, width=2)
        draw.line([(0, pos), (SIZE, pos)], fill=groove, width=2)

    pixels = img.load()
    for y in range(SIZE):
        for x in range(SIZE):
            r, g, b = pixels[x, y]
            if abs(r - groove[0]) > 5:
                random.seed((y // tile_size) * 4 + (x // tile_size) + 404)
                tv = random.randint(-8, 8)
                random.seed(y * SIZE + x + 405)
                v = random.randint(-10, 10)
                pixels[x, y] = (
                    max(0, min(255, r + v + tv)),
                    max(0, min(255, g + v + tv)),
                    max(0, min(255, b + v + tv)),
                )

    img.save(os.path.join(OUT, "gray_tile_floor.png"))
    print("  gray_tile_floor.png")


def gen_dark_teal_metal():
    """Dark teal metal plating for zigzag room."""
    img = Image.new('RGB', (SIZE, SIZE), (35, 75, 65))
    draw = ImageDraw.Draw(img)
    for y in [0, 32, 64, 96]:
        draw.line([(0, y), (SIZE, y)], fill=(25, 55, 48), width=2)
        draw.line([(0, y + 1), (SIZE, y + 1)], fill=(50, 95, 82), width=1)
    for ry in [16, 48, 80, 112]:
        for rx in [16, 48, 80, 112]:
            draw.ellipse([rx - 2, ry - 2, rx + 2, ry + 2], fill=(28, 60, 52))
            draw.ellipse([rx - 1, ry - 1, rx + 1, ry + 1], fill=(45, 90, 78))

    pixels = img.load()
    random.seed(505)
    for y in range(SIZE):
        for x in range(SIZE):
            r, g, b = pixels[x, y]
            v = random.randint(-6, 6)
            pixels[x, y] = (
                max(0, min(255, r + v)),
                max(0, min(255, g + v)),
                max(0, min(255, b + v)),
            )

    img.save(os.path.join(OUT, "dark_teal_metal.png"))
    print("  dark_teal_metal.png")


def gen_tan_walkway():
    """Tan/brown textured walkway for zigzag platforms."""
    img = Image.new('RGB', (SIZE, SIZE), (140, 115, 70))
    draw = ImageDraw.Draw(img)
    spacing = 16
    for row in range(SIZE // spacing + 1):
        for col in range(SIZE // spacing + 1):
            cx = (col * spacing + (spacing // 2 if row % 2 else 0)) % SIZE
            cy = (row * spacing) % SIZE
            pts = [(cx, cy - 3), (cx + 3, cy), (cx, cy + 3), (cx - 3, cy)]
            draw.polygon(pts, fill=(155, 128, 80))
            draw.polygon(pts, outline=(120, 98, 60))

    pixels = img.load()
    random.seed(606)
    for y in range(SIZE):
        for x in range(SIZE):
            r, g, b = pixels[x, y]
            v = random.randint(-10, 10)
            pixels[x, y] = (
                max(0, min(255, r + v)),
                max(0, min(255, g + v)),
                max(0, min(255, b + int(v * 0.5))),
            )

    img.save(os.path.join(OUT, "tan_walkway.png"))
    print("  tan_walkway.png")


def gen_blue_tech_panel():
    """Blue-gray tech panel with circuit lines for computer room."""
    img = Image.new('RGB', (SIZE, SIZE), (55, 60, 95))
    draw = ImageDraw.Draw(img)
    draw.rectangle([0, 0, SIZE - 1, SIZE - 1], outline=(40, 45, 75), width=3)
    draw.rectangle([3, 3, SIZE - 4, SIZE - 4], outline=(70, 78, 120), width=1)
    for y in [24, 48, 72, 96]:
        draw.line([(8, y), (SIZE - 8, y)], fill=(45, 50, 80), width=1)
        for x in [24, 64, 104]:
            if x < SIZE - 8:
                draw.rectangle([x - 2, y - 2, x + 2, y + 2], fill=(70, 80, 130))
    for x in [32, 96]:
        draw.line([(x, 8), (x, SIZE - 8)], fill=(45, 50, 80), width=1)

    pixels = img.load()
    random.seed(707)
    for y in range(SIZE):
        for x in range(SIZE):
            r, g, b = pixels[x, y]
            v = random.randint(-5, 5)
            pixels[x, y] = (
                max(0, min(255, r + v)),
                max(0, min(255, g + v)),
                max(0, min(255, b + v)),
            )

    img.save(os.path.join(OUT, "blue_tech_panel.png"))
    print("  blue_tech_panel.png")


def gen_tech_floor():
    """Dark tech floor grating for computer room."""
    img = Image.new('RGB', (SIZE, SIZE), (50, 48, 55))
    draw = ImageDraw.Draw(img)
    cell = 16
    gap = 2
    for row in range(SIZE // cell):
        for col in range(SIZE // cell):
            x0 = col * cell + gap
            y0 = row * cell + gap
            x1 = (col + 1) * cell - gap
            y1 = (row + 1) * cell - gap
            draw.rectangle([x0, y0, x1, y1], fill=(60, 58, 65))
            draw.rectangle([x0 + 1, y0 + 1, x1 - 1, y1 - 1], fill=(55, 53, 60))
    for row in range(SIZE // cell):
        for col in range(SIZE // cell):
            cx = col * cell + cell // 2
            cy = row * cell + cell // 2
            draw.rectangle([cx - 3, cy - 3, cx + 3, cy + 3], fill=(30, 28, 35))

    pixels = img.load()
    random.seed(808)
    for y in range(SIZE):
        for x in range(SIZE):
            r, g, b = pixels[x, y]
            v = random.randint(-4, 4)
            pixels[x, y] = (
                max(0, min(255, r + v)),
                max(0, min(255, g + v)),
                max(0, min(255, b + v)),
            )

    img.save(os.path.join(OUT, "tech_floor.png"))
    print("  tech_floor.png")


def gen_green_stone():
    """Dark green rough stone for armor alcove."""
    img = Image.new('RGB', (SIZE, SIZE), (55, 80, 45))
    pixels = img.load()
    random.seed(909)
    for y in range(SIZE):
        for x in range(SIZE):
            v = random.randint(-25, 25)
            r, g, b = pixels[x, y]
            pixels[x, y] = (
                max(0, min(255, r + v)),
                max(0, min(255, g + v + random.randint(-5, 5))),
                max(0, min(255, b + v)),
            )
    draw = ImageDraw.Draw(img)
    for i in range(SIZE // 32 + 1):
        pos = i * 32
        draw.line([(pos, 0), (pos, SIZE)], fill=(40, 60, 33), width=1)
        draw.line([(0, pos), (SIZE, pos)], fill=(40, 60, 33), width=1)

    img = img.filter(ImageFilter.SMOOTH)
    img.save(os.path.join(OUT, "green_stone.png"))
    print("  green_stone.png")


def gen_ceiling_panel():
    """Generic dark ceiling panel for multiple rooms."""
    img = Image.new('RGB', (SIZE, SIZE), (65, 62, 58))
    draw = ImageDraw.Draw(img)
    tile = 64
    for i in range(SIZE // tile + 1):
        pos = i * tile
        draw.line([(pos, 0), (pos, SIZE)], fill=(45, 42, 38), width=2)
        draw.line([(0, pos), (SIZE, pos)], fill=(45, 42, 38), width=2)
    for row in range(SIZE // tile):
        for col in range(SIZE // tile):
            x0 = col * tile
            y0 = row * tile
            draw.rectangle([x0 + 4, y0 + 4, x0 + tile - 4, y0 + tile - 4],
                           outline=(72, 69, 65), width=1)

    pixels = img.load()
    random.seed(1010)
    for y in range(SIZE):
        for x in range(SIZE):
            r, g, b = pixels[x, y]
            v = random.randint(-8, 8)
            pixels[x, y] = (
                max(0, min(255, r + v)),
                max(0, min(255, g + v)),
                max(0, min(255, b + v)),
            )

    img.save(os.path.join(OUT, "ceiling_panel.png"))
    print("  ceiling_panel.png")


if __name__ == "__main__":
    print("Generating tiling textures (128x128)...")
    gen_brown_brick()
    gen_brown_stone_floor()
    gen_gray_concrete()
    gen_gray_tile_floor()
    gen_dark_teal_metal()
    gen_tan_walkway()
    gen_blue_tech_panel()
    gen_tech_floor()
    gen_green_stone()
    gen_ceiling_panel()
    print(f"Done! 10 textures in {OUT}/")
