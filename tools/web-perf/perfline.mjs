// The web player's copyable perf line (`#perf`'s data-line, `summary_line` in
// crates/flint-player/src/player_app/perf.rs), read back into numbers. The
// line is parts joined by " | "; each part starts with its own word.
//
//   flint perf · <game> · t=31s | Chrome 156 · <adapter> | 1920x1080 DPR 1 |
//   scene 1920x1080 scale 1 | frame p50 16.7 p90 16.8 p99 17.1 max 18.0 ms (600 frames/10s) |
//   cpu 1.20 ms: ... | gpu 2.31 ms: Scene 0.80, SSAO 0.91, ASCII 0.11 |
//   41 draws 120.3k tris | webgpu/frame submit 2.0 pass 20.0 bindgroup 3.0 write 31.0 (12.1 KB) buffer 0.0 |
//   gpu/frame create 0.0 (buffer 0.0 bindgroup 0.0 texture 0.0 pipeline 0.0) in 0 frames write 17.0 (27.4 KB) |
//   gpu mem textures 102.1 MB (scene 8.2 style 8.5 shadow 64.0 model 21.3 glyph 0.0) meshes 6.3 MB | ...

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
    } else if ((m = /^mean ([\d.]+) ms a frame(?:, GPU-bound \(guess: unpaced, player ([\d.]+) ms, (?:refresh ([\d.]+) ms|no vsync)\))?$/.exec(part))) {
      // since #1318: the window's span over its frames, which doesn't snap to
      // refreshes, and whether it reads as the GPU's ms a frame
      out.throughput = { ms: num(m[1]), gpuBound: m[2] !== undefined };
      if (m[2] !== undefined) out.throughput.guess = { player: num(m[2]), refresh: m[3] === undefined ? null : num(m[3]) };
    } else if ((m = /^hitches (\d+) in (\d+)s \((\d+) in run\)(?:, worst ([\d.]+) ms: (.*))?$/.exec(part))) {
      // since #1079: the window's hitches, the run's, and the worst one's cause
      out.hitches = { window: num(m[1]), secs: num(m[2]), run: num(m[3]) };
      if (m[4] !== undefined) out.hitches.worst = { ms: num(m[4]), cause: m[5] };
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
    } else if (part.startsWith('GPU pass times unavailable')) {
      // The adapter's timestamps read zero (#1316): no passes to list
      out.gpu = { frame: null, passes: {}, note: part, unavailable: true };
    } else if ((m = /^(\d+) draws ([\d.]+\w?) tris$/.exec(part))) {
      out.draws = num(m[1]);
      out.tris = m[2];
    } else if ((m = /^gpu\/frame create ([\d.]+) \(buffer ([\d.]+) bindgroup ([\d.]+) texture ([\d.]+) pipeline ([\d.]+)\) in (\d+) frames write ([\d.]+) \(([\d.]+) KB\)$/.exec(part))) {
      // The renderer's own count (#1091), beside the page's webgpu/frame
      out.created = { objects: num(m[1]), buffers: num(m[2]), bindGroups: num(m[3]), textures: num(m[4]), pipelines: num(m[5]), framesCreating: num(m[6]), writes: num(m[7]), writeKB: num(m[8]) };
    } else if ((m = /^gpu mem textures ([\d.]+) MB \(scene ([\d.]+) style ([\d.]+) shadow ([\d.]+) model ([\d.]+) glyph ([\d.]+)\) meshes ([\d.]+) MB(?: device ([\d.]+)\/([\d.]+) MB)?$/.exec(part))) {
      out.memory = { texturesMB: num(m[1]), sceneMB: num(m[2]), styleMB: num(m[3]), shadowMB: num(m[4]), modelMB: num(m[5]), glyphMB: num(m[6]), meshesMB: num(m[7]), deviceBuffersMB: num(m[8]), deviceTexturesMB: num(m[9]) };
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
