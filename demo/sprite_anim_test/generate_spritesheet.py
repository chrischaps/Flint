"""Generate a stick-figure sprite sheet for the sprite animation test scene.

Layout: 4 columns x 4 rows, 64x64 per frame (256x256 total)
  Row 0: idle    (2 frames) — gentle breathing sway
  Row 1: run     (4 frames) — running cycle
  Row 2: jump    (3 frames) — crouch, launch, airborne
  Row 3: celebrate (4 frames) — arms waving overhead

Outputs: character_sheet.png
"""

from PIL import Image, ImageDraw
import math

CELL = 64
COLS = 4
ROWS = 4
WIDTH = CELL * COLS
HEIGHT = CELL * ROWS

# Warm amber palette — fits the Keep It Warm aesthetic
BG = (30, 20, 15, 0)  # transparent background
BODY = (240, 200, 140)  # warm skin/stick color
OUTLINE = (80, 50, 30)  # dark warm outline
HEAD_FILL = (255, 220, 160)


def draw_stick(draw, cx, cy, pose, scale=1.0):
    """Draw a stick figure centered at (cx, cy) with a given pose dict.

    pose keys:
        head_dy: vertical offset of head
        body_lean: horizontal lean of torso
        l_arm: (shoulder_dx, hand_dx, hand_dy) left arm endpoint relative to shoulder
        r_arm: same for right arm
        l_leg: (hip_dx, foot_dx, foot_dy) left leg
        r_leg: same for right leg
    """
    lw = max(2, int(2.5 * scale))  # line width
    head_r = int(7 * scale)

    # Key points
    head_y = cy - int(22 * scale) + pose.get("head_dy", 0)
    neck_y = head_y + head_r + int(2 * scale)
    shoulder_y = neck_y + int(4 * scale)
    hip_y = shoulder_y + int(14 * scale)
    lean = pose.get("body_lean", 0)

    shoulder_x = cx + lean
    hip_x = cx + lean // 2

    # Torso
    draw.line([(shoulder_x, shoulder_y), (hip_x, hip_y)], fill=BODY, width=lw)

    # Head
    head_x = shoulder_x + pose.get("head_dx", 0)
    draw.ellipse(
        [head_x - head_r, head_y - head_r, head_x + head_r, head_y + head_r],
        fill=HEAD_FILL,
        outline=OUTLINE,
        width=1,
    )

    # Arms
    for arm_key, side in [("l_arm", -1), ("r_arm", 1)]:
        sdx, hdx, hdy = pose.get(arm_key, (0, side * 8, 10))
        sx = shoulder_x + int(sdx * scale)
        hx = shoulder_x + int(hdx * scale)
        hy = shoulder_y + int(hdy * scale)
        draw.line([(sx, shoulder_y), (hx, hy)], fill=BODY, width=lw)

    # Legs
    for leg_key, side in [("l_leg", -1), ("r_leg", 1)]:
        hdx, fdx, fdy = pose.get(leg_key, (0, side * 5, 18))
        hx = hip_x + int(hdx * scale)
        fx = hip_x + int(fdx * scale)
        fy = hip_y + int(fdy * scale)
        # Knee midpoint
        kx = (hx + fx) // 2 + side * int(2 * scale)
        ky = (hip_y + fy) // 2
        draw.line([(hx, hip_y), (kx, ky)], fill=BODY, width=lw)
        draw.line([(kx, ky), (fx, fy)], fill=BODY, width=lw)


# ── Pose definitions ──────────────────────────────────────

idle_poses = [
    {  # frame 0: standing neutral
        "head_dy": 0,
        "l_arm": (0, -10, 12),
        "r_arm": (0, 10, 12),
        "l_leg": (0, -5, 18),
        "r_leg": (0, 5, 18),
    },
    {  # frame 1: slight breath (head bobs up)
        "head_dy": -1,
        "l_arm": (0, -10, 13),
        "r_arm": (0, 10, 13),
        "l_leg": (0, -5, 18),
        "r_leg": (0, 5, 18),
    },
]

run_poses = [
    {  # frame 0: right foot forward, left arm forward
        "body_lean": 3,
        "head_dy": -1,
        "l_arm": (0, 8, 4),
        "r_arm": (0, -6, 14),
        "l_leg": (0, -8, 16),
        "r_leg": (0, 10, 14),
    },
    {  # frame 1: passing (upright, legs crossing)
        "body_lean": 2,
        "head_dy": -3,
        "l_arm": (0, -4, 10),
        "r_arm": (0, 4, 10),
        "l_leg": (0, -3, 18),
        "r_leg": (0, 3, 18),
    },
    {  # frame 2: left foot forward, right arm forward
        "body_lean": 3,
        "head_dy": -1,
        "l_arm": (0, -6, 14),
        "r_arm": (0, 8, 4),
        "l_leg": (0, 10, 14),
        "r_leg": (0, -8, 16),
    },
    {  # frame 3: passing (other side)
        "body_lean": 2,
        "head_dy": -2,
        "l_arm": (0, 4, 10),
        "r_arm": (0, -4, 10),
        "l_leg": (0, 4, 18),
        "r_leg": (0, -4, 18),
    },
]

jump_poses = [
    {  # frame 0: crouch (preparing)
        "head_dy": 6,
        "l_arm": (0, -10, 6),
        "r_arm": (0, 10, 6),
        "l_leg": (-2, -8, 14),
        "r_leg": (2, 8, 14),
    },
    {  # frame 1: launch (extending upward, arms up)
        "head_dy": -6,
        "l_arm": (0, -8, -10),
        "r_arm": (0, 8, -10),
        "l_leg": (0, -4, 18),
        "r_leg": (0, 4, 18),
    },
    {  # frame 2: airborne (tucked, floating)
        "head_dy": -10,
        "l_arm": (0, -12, -4),
        "r_arm": (0, 12, -4),
        "l_leg": (-2, -6, 12),
        "r_leg": (2, 6, 12),
    },
]

celebrate_poses = [
    {  # frame 0: arms up-left
        "head_dy": -2,
        "l_arm": (0, -14, -12),
        "r_arm": (0, 6, -14),
        "l_leg": (0, -6, 18),
        "r_leg": (0, 6, 18),
    },
    {  # frame 1: arms up wide
        "head_dy": -3,
        "l_arm": (0, -14, -14),
        "r_arm": (0, 14, -14),
        "l_leg": (0, -6, 18),
        "r_leg": (0, 6, 18),
    },
    {  # frame 2: arms up-right
        "head_dy": -2,
        "l_arm": (0, -6, -14),
        "r_arm": (0, 14, -12),
        "l_leg": (0, -6, 18),
        "r_leg": (0, 6, 18),
    },
    {  # frame 3: arms extra wide (peak celebration)
        "head_dy": -4,
        "l_arm": (0, -16, -10),
        "r_arm": (0, 16, -10),
        "l_leg": (0, -8, 18),
        "r_leg": (0, 8, 18),
    },
]

all_rows = [idle_poses, run_poses, jump_poses, celebrate_poses]
row_labels = ["idle", "run", "jump", "celebrate"]

# ── Render ────────────────────────────────────────────────

img = Image.new("RGBA", (WIDTH, HEIGHT), BG)
draw = ImageDraw.Draw(img)

# Subtle grid lines for debugging (very faint)
for r in range(ROWS + 1):
    y = r * CELL
    draw.line([(0, y), (WIDTH, y)], fill=(60, 40, 30, 40), width=1)
for c in range(COLS + 1):
    x = c * CELL
    draw.line([(x, 0), (x, HEIGHT)], fill=(60, 40, 30, 40), width=1)

for row_idx, poses in enumerate(all_rows):
    for col_idx, pose in enumerate(poses):
        cx = col_idx * CELL + CELL // 2
        cy = row_idx * CELL + CELL // 2 + 4  # slight downward offset for feet room
        draw_stick(draw, cx, cy, pose)

out_path = "character_sheet.png"
img.save(out_path)
print(f"Saved {WIDTH}x{HEIGHT} sprite sheet to {out_path}")
print(f"  {len(all_rows)} rows, {COLS} columns, {CELL}x{CELL} per frame")
for i, (label, poses) in enumerate(zip(row_labels, all_rows)):
    print(f"  Row {i}: {label} ({len(poses)} frames)")
