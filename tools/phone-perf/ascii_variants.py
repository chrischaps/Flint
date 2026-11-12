import sys, shutil, pathlib
src = pathlib.Path(sys.argv[1]); out = pathlib.Path(sys.argv[2]); eng = pathlib.Path(sys.argv[3])
out.mkdir(parents=True, exist_ok=True)
text = src.read_text()

def strip_ascii(t):
    # drop the styles table and every [entities.*.ascii_style] table with its keys
    keep, skip = [], False
    for l in t.splitlines():
        if l.startswith('['):
            s = l.strip()
            skip = s == '[post_process.styles.ascii]' or s.endswith('.ascii_style]')
        if not skip:
            keep.append(l)
    return '\n'.join(keep) + '\n'

full = text.replace('scope = "entities"\n', '')
assert full != text
def edge(t):
    return t.replace('[post_process.styles.ascii]\n', '[post_process.styles.ascii]\nedges = 1.0\n')

v = {'ascii_none': strip_ascii(text), 'ascii_entities': text, 'ascii_full': full,
     'ascii_entities_edges': edge(text), 'ascii_full_edges': edge(full)}
for k, t in v.items():
    (out / f'{k}.scene.toml').write_text(t)
for d in ['models', 'mesh_meadow']:
    shutil.copytree(src.parent / d, out / d, dirs_exist_ok=True)
shutil.copytree(eng / 'schemas', out / 'engine' / 'schemas', dirs_exist_ok=True)
(out / 'bundle.toml').write_text('scene = "ascii_entities.scene.toml"\nschemas = ["engine/schemas", "engine/schemas/examples"]\n')
print(sorted(p.name for p in out.iterdir()))
print('none ascii mentions:', v['ascii_none'].count('ascii'))
