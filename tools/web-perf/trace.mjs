// run.mjs --trace (#962): every frame the browser gives the page, as numbers.
// The readout's p99 says a 10 s window had slow frames; a trace says when they
// came and how: one at a time (a frame's own work ran long) or as a spell at
// half rate, 33, 33, 17, 33 ms (the browser missing vsync while the frame's
// CPU phases don't move: Firefox's first 15 to 18 s on hardware vsync, which a
// page with no Flint shows too; README.md, "Firefox runs at half rate").

/**
 * Evaluated in the page once the gate lifts: a requestAnimationFrame loop of
 * its own beside the player's, keeping each callback's timestamp (the frame's
 * vsync time), and the page's long tasks where the browser reports them
 * (Chrome does; Firefox 159 has no `longtask` entries).
 */
export const TRACE_JS = `(() => {
  const t = (window.__flintTrace = { start: performance.now(), frames: [], longTasks: [] });
  const tick = (now) => { t.frames.push(Math.round((now - t.start) * 10) / 10); requestAnimationFrame(tick); };
  requestAnimationFrame(tick);
  try {
    new PerformanceObserver((l) => { for (const e of l.getEntries()) t.longTasks.push([Math.round(e.startTime - t.start), Math.round(e.duration)]); })
      .observe({ type: 'longtask', buffered: false });
  } catch {}
  return true;
})()`;

/**
 * A trace (`{ frames: [ms since start], longTasks: [[at ms, ms]] }`) summed
 * up. A frame is slow past 1.5 vsync intervals (25 ms at 60 Hz). A
 * spell is slow frames with at most one on-time frame between two of them,
 * so a stretch at half rate reads as one spell, not dozens.
 */
export function summarizeTrace({ frames, longTasks = [] }) {
  const gaps = frames.slice(1).map((f, i) => Math.round((f - frames[i]) * 10) / 10);
  if (!gaps.length) return { frames: frames.length, seconds: 0, slow: 0, spells: [], fps: [], slowSeconds: 0, longTasks, gaps };
  const sorted = [...gaps].sort((a, b) => a - b);
  const q = (p) => sorted[Math.min(sorted.length - 1, Math.floor(p * sorted.length))];
  // the display's interval: the shortest that at least 1 frame in 20 has (to
  // the ms), not the median, which a page held at half rate most of its run
  // moves to 33 ms; early callbacks after a missed frame are too rare to count
  const counts = new Map();
  for (const g of gaps) counts.set(Math.round(g), (counts.get(Math.round(g)) ?? 0) + 1);
  const common = [...counts].filter(([ms, n]) => ms > 0 && n >= gaps.length / 20).map(([ms]) => ms);
  const near = (ms) => gaps.filter((g) => Math.round(g) === ms).sort((x, y) => x - y);
  const vsync = common.length ? near(Math.min(...common))[Math.floor(near(Math.min(...common)).length / 2)] : q(0.5);
  const slow = gaps.map((g, i) => [i, g]).filter(([, g]) => g > vsync * 1.5);
  const spells = [];
  for (const [i, g] of slow) {
    const last = spells.at(-1);
    if (last && i - last.lastIndex <= 2) {
      last.lastIndex = i;
      last.slow++;
      last.worst = Math.max(last.worst, g);
    } else spells.push({ firstIndex: i, lastIndex: i, slow: 1, worst: g });
  }
  const s1 = (ms) => Math.round(ms / 100) / 10;
  return {
    frames: frames.length,
    seconds: s1(frames.at(-1) - frames[0]),
    vsync,
    p99: q(0.99),
    max: sorted.at(-1),
    slow: slow.length,
    // each spell: when it began (s from the trace's start), how long it lasted
    // (s, first slow frame's start to last slow frame's end), how many slow
    // frames it held and the worst of them (ms)
    spells: spells.map((s) => ({
      at: s1(frames[s.firstIndex]),
      lasts: s1(frames[s.lastIndex + 1] - frames[s.firstIndex]),
      slow: s.slow,
      worst: s.worst,
    })),
    // frames in each whole second of the trace: 60 is vsync, 30 is half rate
    fps: perSecond(frames),
    // how many of those seconds ran at 45 fps or less: at 60 Hz, mostly at half rate
    slowSeconds: perSecond(frames).filter((n) => n <= 45).length,
    longTasks,
    gaps,
  };
}

function perSecond(frames) {
  const out = Array(Math.max(0, Math.floor((frames.at(-1) ?? 0) / 1000))).fill(0);
  for (const f of frames) if (Math.floor(f / 1000) < out.length) out[Math.floor(f / 1000)]++;
  return out;
}

/** One line of a summary, for the log. */
export function traceLine(t) {
  const longest = t.spells.length ? ` (longest ${Math.max(...t.spells.map((s) => s.lasts))} s)` : '';
  return `${t.frames} frames in ${t.seconds} s, vsync ${t.vsync} ms, p99 ${t.p99} max ${t.max}, ${t.slow} slow in ${t.spells.length} spells${longest}, ${t.slowSeconds} of ${t.fps.length} s at 45 fps or less`;
}
