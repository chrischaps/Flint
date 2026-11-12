// The web player's copyable perf line (`#perf`'s data-line, `summary_line` in
// crates/flint-player/src/player_app/perf.rs), read back into numbers. The
// line is parts joined by " | "; each part starts with its own word.
//
//   flint perf · <game> · t=31s | Chrome 156 · <adapter> | 1920x1080 DPR 1 |
//   scene 1920x1080 scale 1 | frame p50 16.7 p90 16.8 p99 17.1 max 18.0 ms (600 frames/10s) |
//   cpu 1.20 ms: ... | gpu 2.31 ms: Scene 0.80, SSAO 0.91, ASCII 0.11 |
//   41 draws 120.3k tris | webgpu/frame submit 2.0 pass 20.0 bindgroup 3.0 write 31.0 (12.1 KB) buffer 0.0 | ...

const num = (s) => (s === undefined ? undefined : Number(s));

/** Parse one perf line. Fields the line doesn't carry are left out. */
export function parsePerfLine(line) {
  const out = {};
  if (!line) return out;
  for (const part of line.split(' | ')) {
    let m;
    if ((m = /^flint perf · (.*) · t=(\d+)s$/.exec(part))) {
      out.game = m[1];
      out.t = num(m[2]);
    } else if ((m = /^(\d+)x(\d+) DPR ([\d.]+)$/.exec(part))) {
      out.canvas = [num(m[1]), num(m[2])];
      out.dpr = num(m[3]);
    } else if ((m = /^scene (\d+)x(\d+) scale ([\d.]+)$/.exec(part))) {
      out.scene = [num(m[1]), num(m[2])];
      out.scale = num(m[3]);
    } else if ((m = /^frame p50 ([\d.]+) p90 ([\d.]+) p99 ([\d.]+) max ([\d.]+) ms \((\d+) frames\/(\d+)s\)$/.exec(part))) {
      out.frame = { p50: num(m[1]), p90: num(m[2]), p99: num(m[3]), max: num(m[4]), frames: num(m[5]), secs: num(m[6]) };
    } else if ((m = /^cpu ([\d.]+) ms/.exec(part))) {
      out.cpu = num(m[1]);
    } else if ((m = /^present (\S+)$/.exec(part))) {
      out.present = m[1];
    } else if ((m = /^gpu ([\d.]+) ms( [^:]*)?: (.*)$/.exec(part))) {
      // since #380 the times may say how they were taken: " under vsync",
      // " no vsync", or " under vsync at idle clocks (31% busy, reads high; ...)"
      out.gpu = { frame: num(m[1]), passes: {} };
      const note = m[2]?.trim();
      if (note) {
        out.gpu.vsync = note.startsWith('under vsync');
        const busy = /\((\d+)% busy/.exec(note);
        if (busy) out.gpu.idleClocks = num(busy[1]) / 100;
      }
      // a pass's label may hold spaces ("Style mask"); its time is the last word
      for (const pass of m[3].split(', ')) {
        const p = /^(.*) ([\d.]+)$/.exec(pass);
        if (p) out.gpu.passes[p[1]] = num(p[2]);
      }
    } else if (part === 'gpu off' || part === 'gpu untimed') {
      out.gpu = { frame: null, passes: {}, note: part };
    } else if ((m = /^(\d+) draws ([\d.]+\w?) tris$/.exec(part))) {
      out.draws = num(m[1]);
      out.tris = m[2];
    } else if ((m = /^webgpu\/frame submit ([\d.]+) pass ([\d.]+) bindgroup ([\d.]+) write ([\d.]+) \(([\d.]+) KB\) buffer ([\d.]+)$/.exec(part))) {
      out.webgpu = { submits: num(m[1]), passes: num(m[2]), bindGroups: num(m[3]), writes: num(m[4]), writeKB: num(m[5]), buffers: num(m[6]) };
    } else if (out.browser === undefined && / · /.test(part)) {
      const [browser, ...adapter] = part.split(' · ');
      out.browser = browser;
      out.adapter = adapter.join(' · ');
    }
  }
  return out;
}

/** "0.09--0.12" over several readouts, one number when they agree. */
export function range(values, digits = 2) {
  const v = values.filter((x) => typeof x === 'number' && Number.isFinite(x));
  if (!v.length) return '';
  const lo = Math.min(...v).toFixed(digits);
  const hi = Math.max(...v).toFixed(digits);
  return lo === hi ? lo : `${lo}--${hi}`;
}
