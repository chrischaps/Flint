"""Generate decorative billboard sprites for E1M1 Hangar level."""
from PIL import Image, ImageDraw
import random
import os

OUT = os.path.join(os.path.dirname(__file__), "sprites")
os.makedirs(OUT, exist_ok=True)


def gen_barrel():
    """Green/brown toxic waste barrel (32x48, transparent background)."""
    W, H = 32, 48
    img = Image.new('RGBA', (W, H), (0, 0, 0, 0))
    draw = ImageDraw.Draw(img)

    # Barrel body (rounded rect approximation)
    body_top, body_bot = 6, 44
    body_left, body_right = 4, 27

    # Main barrel body - brown/gray metal
    draw.rectangle([body_left, body_top, body_right, body_bot], fill=(90, 75, 55, 255))

    # Slight barrel curve shading (lighter center, darker edges)
    for x in range(body_left, body_right + 1):
        center_dist = abs(x - (body_left + body_right) / 2) / ((body_right - body_left) / 2)
        shade = int(20 * (1 - center_dist * center_dist))
        for y in range(body_top, body_bot + 1):
            r, g, b, a = img.getpixel((x, y))
            if a > 0:
                img.putpixel((x, y), (
                    min(255, r + shade),
                    min(255, g + shade),
                    min(255, b + shade // 2),
                    a
                ))

    # Metal bands (horizontal stripes)
    for band_y in [body_top + 2, body_top + 10, body_bot - 10, body_bot - 2]:
        draw.rectangle([body_left, band_y, body_right, band_y + 1],
                       fill=(70, 65, 55, 255))

    # Green toxic symbol / glow in middle
    mid_y = (body_top + body_bot) // 2
    mid_x = (body_left + body_right) // 2
    draw.rectangle([mid_x - 6, mid_y - 5, mid_x + 6, mid_y + 5],
                   fill=(40, 140, 30, 255))
    draw.rectangle([mid_x - 4, mid_y - 3, mid_x + 4, mid_y + 3],
                   fill=(60, 180, 40, 255))
    # Toxic symbol: simple biohazard-ish circle
    draw.ellipse([mid_x - 3, mid_y - 3, mid_x + 3, mid_y + 3],
                 fill=(30, 120, 20, 255))
    draw.ellipse([mid_x - 1, mid_y - 1, mid_x + 1, mid_y + 1],
                 fill=(80, 200, 50, 255))

    # Barrel top (ellipse)
    draw.ellipse([body_left + 1, body_top - 3, body_right - 1, body_top + 3],
                 fill=(100, 85, 65, 255), outline=(70, 60, 45, 255))

    # Pixel noise for texture
    random.seed(111)
    pixels = img.load()
    for y in range(H):
        for x in range(W):
            r, g, b, a = pixels[x, y]
            if a > 0:
                v = random.randint(-8, 8)
                pixels[x, y] = (
                    max(0, min(255, r + v)),
                    max(0, min(255, g + v)),
                    max(0, min(255, b + v)),
                    a,
                )

    img.save(os.path.join(OUT, "barrel.png"))
    print("  barrel.png (32x48)")


def gen_computer_panel():
    """Blue-green glowing terminal screen (48x48, transparent background)."""
    W, H = 48, 48
    img = Image.new('RGBA', (W, H), (0, 0, 0, 0))
    draw = ImageDraw.Draw(img)

    # Monitor casing (dark gray)
    draw.rectangle([2, 2, 45, 45], fill=(50, 50, 55, 255))
    draw.rectangle([3, 3, 44, 44], fill=(60, 60, 65, 255))

    # Screen area (dark blue-green glow)
    draw.rectangle([6, 6, 41, 38], fill=(15, 40, 50, 255))
    # Screen bezel highlight
    draw.rectangle([6, 6, 41, 7], fill=(30, 60, 70, 255))
    draw.rectangle([6, 6, 7, 38], fill=(25, 55, 65, 255))

    # Scan lines on screen
    for y in range(8, 37, 2):
        draw.line([(8, y), (39, y)], fill=(20, 55, 65, 255), width=1)

    # Text lines (bright green/cyan blocks representing text)
    text_color = (50, 200, 150, 255)
    random.seed(222)
    for row in range(5):
        y = 10 + row * 5
        line_len = random.randint(8, 28)
        for seg in range(random.randint(2, 5)):
            x_start = 9 + random.randint(0, 20)
            x_end = min(39, x_start + random.randint(3, 10))
            draw.rectangle([x_start, y, x_end, y + 2], fill=text_color)

    # Cursor blink (bright block)
    draw.rectangle([9, 33, 12, 35], fill=(100, 255, 200, 255))

    # Bottom panel (buttons/LEDs)
    draw.rectangle([8, 40, 14, 43], fill=(40, 40, 45, 255))  # button
    draw.ellipse([38, 40, 42, 44], fill=(200, 50, 30, 255))  # red LED
    draw.ellipse([33, 40, 37, 44], fill=(30, 200, 60, 255))  # green LED

    img.save(os.path.join(OUT, "computer_panel.png"))
    print("  computer_panel.png (48x48)")


def gen_tech_column():
    """Metallic support pillar (24x64, transparent background)."""
    W, H = 24, 64
    img = Image.new('RGBA', (W, H), (0, 0, 0, 0))
    draw = ImageDraw.Draw(img)

    col_left, col_right = 4, 19

    # Main column body
    draw.rectangle([col_left, 2, col_right, 61], fill=(100, 100, 105, 255))

    # Cylindrical shading (lighter in center)
    pixels = img.load()
    for y in range(2, 62):
        for x in range(col_left, col_right + 1):
            r, g, b, a = pixels[x, y]
            if a > 0:
                center = (col_left + col_right) / 2
                dist = abs(x - center) / ((col_right - col_left) / 2)
                shade = int(25 * (1 - dist * dist))
                pixels[x, y] = (
                    min(255, r + shade),
                    min(255, g + shade),
                    min(255, b + shade),
                    a,
                )

    # Horizontal bands/rivets
    draw = ImageDraw.Draw(img)
    for band_y in [4, 15, 30, 45, 59]:
        draw.rectangle([col_left, band_y, col_right, band_y + 2],
                       fill=(75, 75, 80, 255))
        draw.line([(col_left, band_y + 3), (col_right, band_y + 3)],
                  fill=(120, 120, 125, 255), width=1)

    # Base and capital (wider sections)
    draw.rectangle([col_left - 2, 0, col_right + 2, 4], fill=(80, 80, 85, 255))
    draw.rectangle([col_left - 2, 60, col_right + 2, 63], fill=(80, 80, 85, 255))

    # Small detail: warning stripe near middle
    for i in range(4):
        y = 32 + i
        for x in range(col_left + 1, col_right):
            if (x + i) % 4 < 2:
                pixels[x, y] = (180, 150, 30, 255)
            else:
                pixels[x, y] = (40, 40, 45, 255)

    # Noise
    random.seed(333)
    for y in range(H):
        for x in range(W):
            r, g, b, a = pixels[x, y]
            if a > 0:
                v = random.randint(-5, 5)
                pixels[x, y] = (
                    max(0, min(255, r + v)),
                    max(0, min(255, g + v)),
                    max(0, min(255, b + v)),
                    a,
                )

    img.save(os.path.join(OUT, "tech_column.png"))
    print("  tech_column.png (24x64)")


if __name__ == "__main__":
    print("Generating decorative sprites...")
    gen_barrel()
    gen_computer_panel()
    gen_tech_column()
    print(f"Done! Sprites in {OUT}/")
