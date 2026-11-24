// run.mjs --save-trace (#1266): a trace the page's Save trace downloaded, as
// numbers. A scope's `dur` is two reads of the recorder's clock, which on the
// web is `performance.now()`; a browser coarsens that on a page that isn't
// cross-origin isolated, so a scope shorter than the clock's step reads as 0.
// This says how many did, by name, and the step the durations come in.

/**
 * A Chrome trace-event JSON (`flint_core::profile::chrome`, the player's
 * `TraceJob::to_json`) summed up: its scopes are the `"ph":"X"` events on a
 * thread track (`args.depth` is set on every scope and on nothing else).
 * Durations are in µs, as the file has them.
 */
export function summarizeScopes(trace) {
  const events = Array.isArray(trace) ? trace : trace.traceEvents ?? [];
  const scopes = events.filter((e) => e.ph === 'X' && e.args && 'depth' in e.args);
  const byName = new Map();
  let zero = 0;
  let quantum = Infinity;
  const steps = new Map();
  for (const s of scopes) {
    const dur = s.dur ?? 0;
    if (dur === 0) zero++;
    else {
      quantum = Math.min(quantum, dur);
      const key = Math.round(dur * 1000) / 1000;
      steps.set(key, (steps.get(key) ?? 0) + 1);
    }
    const n = byName.get(s.name) ?? { name: s.name, count: 0, zero: 0, totalUs: 0 };
    n.count++;
    if (dur === 0) n.zero++;
    n.totalUs += dur;
    byName.set(s.name, n);
  }
  const names = [...byName.values()]
    .map((n) => ({ ...n, meanUs: round(n.totalUs / n.count), zeroShare: round(n.zero / n.count) }))
    .sort((a, b) => b.count - a.count || a.name.localeCompare(b.name));
  // the smallest durations seen, in µs, with their counts: on a coarsened
  // clock they are whole steps (100, 200 ...), on a fine one they spread
  const smallest = [...steps.entries()].sort((a, b) => a[0] - b[0]).slice(0, 6);
  const header = trace.otherData ?? {};
  return {
    scopes: scopes.length,
    zero,
    zeroShare: scopes.length ? round(zero / scopes.length) : 0,
    smallestUs: quantum === Infinity ? null : round(quantum),
    smallest,
    frames: new Set(scopes.map((s) => s.args.frame)).size,
    names,
    reason: header.reason,
  };
}

/** One line for the log and the table. */
export function scopesLine(s) {
  const smallest = s.smallestUs === null ? 'none above 0' : `${s.smallestUs} µs`;
  return `${s.scopes} scopes over ${s.frames} frames, ${s.zero} read 0 (${Math.round(s.zeroShare * 100)}%), the shortest above 0 ${smallest}`;
}

const round = (x) => Math.round(x * 1000) / 1000;
