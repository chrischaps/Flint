"""Subset the downloaded OFL fonts into examples/fonts/ofl/<Family>/ (#935).

Usage: python -I -X utf8 subset.py <download dir> <out dir>

<download dir> is laid out as google/fonts' `ofl/` folder (`vt323/VT323-Regular.ttf`,
`vt323/OFL.txt`, ...): each font's file and licence from
https://raw.githubusercontent.com/google/fonts/main/ofl/<dir>/<file>. Needs fontTools.

Each font keeps printable ASCII, Latin-1, box drawing and the blocks its
glyph set draws from, without hinting or layout tables (ab_glyph uses
neither). A variable font is pinned to its regular weight first. None of
these fonts has a Reserved Font Name, so a subset may keep its name.
"""
import os, shutil, sys
from fontTools.ttLib import TTFont
from fontTools import subset
from fontTools.varLib import instancer

src, out = sys.argv[1], sys.argv[2]

LATIN = [(0x20, 0x7E), (0xA0, 0xFF), (0x2010, 0x2027), (0x2190, 0x21FF), (0x2500, 0x25FF)]
GRADE1 = ("一右雨円王音下火花貝学気九休玉金空月犬見五口校左三山子四糸字耳七車手十出女小上森"
          "人水正生青夕石赤千川先早草足村大男竹中虫町天田土二日入年白八百文木本名目立力林六"
          "丶品画雷鬱龍鷹書語道風")
HANGUL = ("ㆍㅡㅣㅇㅎ이으을를릴흘훑뷁가나다라마바사아자차카타파하한글별빛달꽃물불바람사랑"
          "밤하늘눈비구름숲길")
CUNEI = "𒀸𒀹𒁹𒁺𒌋𒈫𒀭𒆠𒈗𒂗𒊩𒌓𒉡𒄑𒋾𒀀𒁀𒃲𒅆𒆳𒇻𒈨𒊑𒌉"
JP = LATIN + [(0x3000, 0x30FF), (0xFF00, 0xFFEF)]

FONTS = [
    # (family dir in out, download dir, file, ranges, extra text, keep whole)
    ("VT323", "vt323", "VT323-Regular.ttf", LATIN, "", False),
    ("CutiveMono", "cutivemono", "CutiveMono-Regular.ttf", LATIN, "", False),
    ("RubikMonoOne", "rubikmonoone", "RubikMonoOne-Regular.ttf", LATIN, "", False),
    ("MajorMonoDisplay", "majormonodisplay", "MajorMonoDisplay-Regular.ttf", LATIN, "", False),
    ("NotoSansSymbols2", "notosanssymbols2", "NotoSansSymbols2-Regular.ttf",
     LATIN + [(0x2200, 0x22FF), (0x2600, 0x27BF), (0x2B00, 0x2BFF)], "", False),
    ("DotGothic16", "dotgothic16", "DotGothic16-Regular.ttf", JP, GRADE1, False),
    ("YujiSyuku", "yujisyuku", "YujiSyuku-Regular.ttf", JP, GRADE1, False),
    ("GowunBatang", "gowunbatang", "GowunBatang-Regular.ttf",
     LATIN + [(0x3130, 0x318F), (0xFF00, 0xFFEF)], HANGUL, False),
    ("NotoSansCuneiform", "notosanscuneiform", "NotoSansCuneiform-Regular.ttf",
     LATIN + [(0x12400, 0x1247F)], CUNEI, False),
    ("NotoEmoji", "notoemoji", "NotoEmoji[wght].ttf",
     [(0x20, 0x7E), (0x2190, 0x21FF), (0x2600, 0x26FF), (0x2700, 0x27BF), (0x2B50, 0x2B55),
      (0x1F300, 0x1F32C), (0x1F319, 0x1F31F)], "", False),
    ("NotoSansRunic", "notosansrunic", "NotoSansRunic-Regular.ttf", [], "", True),
]

for family, dl, name, ranges, text, whole in FONTS:
    dest = os.path.join(out, family)
    os.makedirs(dest, exist_ok=True)
    shutil.copy(os.path.join(src, dl, "OFL.txt"), os.path.join(dest, "OFL.txt"))
    in_path = os.path.join(src, dl, name)
    out_name = name.replace("[wght]", "-Regular")
    out_path = os.path.join(dest, out_name)
    if whole:
        shutil.copy(in_path, out_path)
    else:
        font = TTFont(in_path)
        if "fvar" in font:
            font = instancer.instantiateVariableFont(font, {"wght": 400})
        unicodes = [c for lo, hi in ranges for c in range(lo, hi + 1)]
        unicodes += [ord(c) for c in text]
        options = subset.Options()
        options.hinting = False
        options.layout_features = []
        options.name_IDs = ["*"]
        options.name_languages = ["*"]
        options.notdef_outline = True
        sub = subset.Subsetter(options)
        sub.populate(unicodes=unicodes)
        sub.subset(font)
        font.save(out_path)
    before = os.path.getsize(in_path)
    after = os.path.getsize(out_path)
    print(f"{family:20} {out_name:34} {before / 1024:8.0f} KB -> {after / 1024:6.0f} KB  "
          f"{len(TTFont(out_path).getBestCmap())} chars")
