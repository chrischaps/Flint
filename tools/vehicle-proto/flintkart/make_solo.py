"""Oval Plus with Blaze (ai1) alone: the other four AI karts and their brains
removed, the player's kart left parked on the grid (race_manager needs it).

    python make_solo.py <FlintKart copy>/scenes/oval_plus.scene.toml <out>.scene.toml
"""
import re
import sys

src, dst = sys.argv[1], sys.argv[2]
lines = open(src, encoding='utf-8').read().split('\n')
out, skip = [], False
drop = re.compile(r'^\[(prefabs\.ai[2-5]|entities\.ai[2-5]_brain)(\.|\])')
for line in lines:
    if line.startswith('['):
        skip = bool(drop.match(line))
    if not skip:
        out.append(line)
open(dst, 'w', encoding='utf-8', newline='\n').write('\n'.join(out))
print(len(lines), '->', len(out))
