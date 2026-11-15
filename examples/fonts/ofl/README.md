# Bundled fonts (#935)

Every font here is under the **SIL Open Font License 1.1**; each folder holds the font and its
`OFL.txt` (copyright and licence, as published). None declares a Reserved Font Name, so the
subsets below may keep their names. The engine repo is carried into every game as a subtree,
so only OFL or public-domain fonts belong here, with their licence beside them.

| Glyph set | Font | Folder | Source | Kept |
|---|---|---|---|---|
| `phosphor` | VT323 | `VT323/` | [google/fonts `ofl/vt323`](https://github.com/google/fonts/tree/main/ofl/vt323) | subset |
| `typewriter` | Cutive Mono | `CutiveMono/` | [`ofl/cutivemono`](https://github.com/google/fonts/tree/main/ofl/cutivemono) | subset |
| `poster` | Rubik Mono One | `RubikMonoOne/` | [`ofl/rubikmonoone`](https://github.com/google/fonts/tree/main/ofl/rubikmonoone) | subset |
| `geometric` | Major Mono Display | `MajorMonoDisplay/` | [`ofl/majormonodisplay`](https://github.com/google/fonts/tree/main/ofl/majormonodisplay) | subset |
| `stars` | Noto Sans Symbols 2 | `NotoSansSymbols2/` | [`ofl/notosanssymbols2`](https://github.com/google/fonts/tree/main/ofl/notosanssymbols2) | subset |
| `kana` | DotGothic16 | `DotGothic16/` | [`ofl/dotgothic16`](https://github.com/google/fonts/tree/main/ofl/dotgothic16) | subset |
| `brush` | Yuji Syuku | `YujiSyuku/` | [`ofl/yujisyuku`](https://github.com/google/fonts/tree/main/ofl/yujisyuku) | subset |
| `hangul` | Gowun Batang | `GowunBatang/` | [`ofl/gowunbatang`](https://github.com/google/fonts/tree/main/ofl/gowunbatang) | subset |
| `cuneiform` | Noto Sans Cuneiform | `NotoSansCuneiform/` | [`ofl/notosanscuneiform`](https://github.com/google/fonts/tree/main/ofl/notosanscuneiform) | subset |
| `moon` | Noto Emoji | `NotoEmoji/` | [`ofl/notoemoji`](https://github.com/google/fonts/tree/main/ofl/notoemoji) | subset, weight 400 |
| `runes` | Noto Sans Runic | `NotoSansRunic/` | [`ofl/notosansrunic`](https://github.com/google/fonts/tree/main/ofl/notosansrunic) | whole |

**Subsets.** `subset.py` made them from a download laid out as google/fonts' `ofl/` folder:
printable ASCII, Latin-1, arrows and box drawing, plus the blocks each set draws from (kana and
full-width forms, Hangul jamo, the cuneiform numerals, the symbol and dingbat blocks), with
hinting and layout tables removed (ab_glyph reads neither). The CJK, Hangul and cuneiform
fonts keep a few dozen ideographs or syllables beyond their ramps, not the whole script: an
8 MB font is 90-240 KB here. A glyph you add to a ramp that the subset left out fails to load
with `the font has no glyph for '…'`; add it to the font's text in `subset.py` and run it
again over the original download:

```text
python -I -X utf8 examples/fonts/ofl/subset.py <download>/ofl examples/fonts/ofl
```

**Tried and dropped:** Caveat (handwriting, OFL). A proportional script centred in a cell
as wide as its widest letter leaves its strokes thin (the most ink any glyph has is 0.15 of
the cell), and at ASCII cell sizes the hand reads as a field of `#` and `@` with nothing of the
handwriting left. Nanum Gothic Coding was passed over for Hangul because its licence reserves
the name Nanum, so a subset could not keep it; Gowun Batang reserves none.
