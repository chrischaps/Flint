import re, sys, json
log = open(sys.argv[1], encoding='utf8').read()
rows = []
for block in re.split(r'^--- ', log, flags=re.M)[1:]:
    head, *rest = block.split('\n')
    scene, sample = re.match(r'(\S+) sample (\d+)', head).groups()
    line = [l for l in rest if l.startswith('line: ')][0]
    g = lambda pat: (re.search(pat, line) or [None, None])[1]
    frame = re.search(r'frame p50 ([\d.]+) p90 ([\d.]+) p99 ([\d.]+)', line).groups()
    gpu = dict(re.findall(r'(Grass|Shadows|Scene|Style mask|SSAO|Bloom|Composite|ASCII|ASCII edges|UI|[A-Z][\w ]*?) ([\d.]+)(?:,| \|)', line.split('gpu ')[-1].split('|')[0] + ' |'))
    rows.append(dict(scene=scene, s=int(sample), p50=float(frame[0]), p90=float(frame[1]), p99=float(frame[2]),
                     cpu=float(g(r'cpu ([\d.]+) ms')), gputot=float(g(r'gpu ([\d.]+) ms')),
                     passes=float(g(r'pass ([\d.]+)')), bg=float(g(r'bindgroup ([\d.]+)')), gpu=gpu, t=g(r't=(\d+)s')))
for r in rows:
    print(f"{r['scene']:22} s{r['s']} t={r['t']:>3} p50 {r['p50']:5.1f} p90 {r['p90']:5.1f} p99 {r['p99']:5.1f} cpu {r['cpu']:5.2f} gpu {r['gputot']:5.2f} pass {r['passes']:4.1f} bg {r['bg']} | " +
          ' '.join(f"{k}={v}" for k, v in r['gpu'].items()))
json.dump(rows, open(sys.argv[2], 'w'), indent=1)
