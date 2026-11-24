// #1249 scratch: open a samply profile (samply load --no-open's URL) in the
// Firefox Profiler in headless Chrome, wait for its symbols, measure the
// symbolicated stacks from the page's own `window.profile`, and screenshot
// the call tree and the flame graph.
//   node profshot.mjs <profiler url> <out prefix> <chrome path>
import { spawn } from 'node:child_process';
import { mkdtempSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';

const [url, out, chromePath] = process.argv.slice(2);
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
const port = 9333;

const chrome = spawn(chromePath, [
  '--headless=new', `--remote-debugging-port=${port}`,
  `--user-data-dir=${mkdtempSync(join(tmpdir(), 'profshot-'))}`,
  '--no-first-run', '--no-default-browser-check', '--window-size=1600,1000',
  // an https page fetching samply's 127.0.0.1 server: a person's Chrome asks; headless can't
  '--disable-features=LocalNetworkAccessChecks,PrivateNetworkAccessSendPreflights,PrivateNetworkAccessRespectPreflightResults,BlockInsecurePrivateNetworkRequests',
  'about:blank',
], { stdio: 'ignore' });

let targets;
for (let i = 0; i < 120 && !targets; i++) {
  await sleep(250);
  try { targets = await (await fetch(`http://127.0.0.1:${port}/json`)).json(); } catch {}
}
const page = targets.find((t) => t.type === 'page');
const ws = new WebSocket(page.webSocketDebuggerUrl);
await new Promise((r) => ws.addEventListener('open', r));
let nextId = 0;
const pending = new Map();
ws.addEventListener('message', (ev) => {
  const m = JSON.parse(ev.data);
  if (m.id && pending.has(m.id)) { pending.get(m.id)(m); pending.delete(m.id); }
});
const send = (method, params = {}) => new Promise((r) => {
  const id = ++nextId; pending.set(id, r); ws.send(JSON.stringify({ id, method, params }));
});
const evaluate = async (expr) => {
  const r = await send('Runtime.evaluate', { expression: expr, returnByValue: true });
  if (r.result?.exceptionDetails) console.error('evaluate threw:', JSON.stringify(r.result.exceptionDetails).slice(0, 800));
  return r.result?.result?.value;
};

// Every thread's stacks, read the way the profiler shows them.
const STATS = `(() => {
  const p = window.profile;
  if (!p || !p.threads) return null;
  const sh = p.shared || {};
  const hex = /^(0x)?[0-9a-f]{6,}$/i;
  const threads = [];
  for (const [ti, t] of p.threads.entries()) {
    const strs = sh.stringArray || t.stringArray || (t.stringTable && t.stringTable._array) || [];
    const st = t.stackTable || sh.stackTable, ft = t.frameTable || sh.frameTable, fn = t.funcTable || sh.funcTable;
    const stacks = t.samples.stack;
    const name = (f) => strs[fn.name[ft.func[f]]] || '';
    let n = 0, frames = 0, named = 0, lined = 0, flintSamples = 0;
    let stall = { n: 0, frames: 0, named: 0, deepest: [] };
    const distinct = new Set(), self = new Map();
    for (const s0 of stacks) {
      if (s0 === null || s0 === undefined) continue;
      n++;
      const names = [];
      let hasLine = 0;
      for (let s = s0; s !== null && s !== undefined && s >= 0; s = st.prefix[s]) {
        const f = st.frame[s];
        names.push(name(f));
        if (ft.line && ft.line[f] !== null && ft.line[f] !== undefined) hasLine++;
      }
      const nm = names.filter((x) => x && !hex.test(x));
      frames += names.length; named += nm.length; lined += hasLine;
      nm.forEach((x) => distinct.add(x));
      if (names.some((x) => x.startsWith('flint_'))) flintSamples++;
      self.set(names[0], (self.get(names[0]) || 0) + 1);
      if (names.some((x) => /rhai/i.test(x))) {
        stall.n++; stall.frames += names.length; stall.named += nm.length;
        if (names.length > stall.deepest.length) stall.deepest = names.slice().reverse();
      }
    }
    if (!n) continue;
    threads.push({
      index: ti, name: t.name, processName: t.processName, isMainThread: !!t.isMainThread,
      samples: n, avgDepth: +(frames / n).toFixed(1), avgNamed: +(named / n).toFixed(1),
      namedFrac: +(named / frames).toFixed(3), linedFrac: +(lined / frames).toFixed(3),
      flintSampleFrac: +(flintSamples / n).toFixed(3), distinctFunctions: distinct.size,
      topSelf: [...self.entries()].sort((a, b) => b[1] - a[1]).slice(0, 12),
      stall: stall.n ? { samples: stall.n, avgDepth: +(stall.frames / stall.n).toFixed(1),
        avgNamed: +(stall.named / stall.n).toFixed(1), deepest: stall.deepest } : null,
    });
  }
  threads.sort((a, b) => b.samples - a.samples);
  return { meta: { product: p.meta.product, oscpu: p.meta.oscpu, interval: p.meta.interval }, threads };
})()`;

async function load(u, waitForSymbols) {
  await send('Page.navigate', { url: u });
  let last = null, stable = 0, stats = null;
  for (let i = 0; i < 240; i++) {
    await sleep(1000);
    const text = (await evaluate('document.body ? document.body.innerText : ""')) || '';
    stats = await evaluate(STATS);
    const key = stats ? JSON.stringify(stats.threads.map((t) => t.namedFrac)) : null;
    const busy = /symbolicat/i.test(text);
    stable = key && key === last && !busy ? stable + 1 : 0;
    last = key;
    if (stable >= (waitForSymbols ? 5 : 2)) break;
    if (i % 20 === 0) console.error(`t=${i}s has window.profile: ${await evaluate('!!window.profile')} text: ${text.slice(0, 300).replace(/\s+/g, ' ')}`);
  }
  return stats;
}

async function shot(file) {
  const r = await send('Page.captureScreenshot', { format: 'png' });
  writeFileSync(file, Buffer.from(r.result.data, 'base64'));
}

await send('Page.enable');
await send('Runtime.enable');
await send('Log.enable');
ws.addEventListener('message', (ev) => {
  const m = JSON.parse(ev.data);
  if (m.method === 'Log.entryAdded') console.error('page log:', m.params.entry.level, m.params.entry.text.slice(0, 300));
  if (m.method === 'Runtime.exceptionThrown') console.error('page exception:', JSON.stringify(m.params).slice(0, 500));
});
const stats = await load(url, true);
await shot(`${out}-calltree.png`);
writeFileSync(`${out}-stats.json`, JSON.stringify(stats ?? null, null, 2));
console.log(JSON.stringify(stats ?? null, null, 2));
const main = stats?.threads.find((t) => t.isMainThread) || stats?.threads[0];
const flame = url.replace('/?symbolServer', '/flame-graph/?symbolServer') + (main ? `&thread=${main.index}` : '');
await load(flame, true);
await sleep(2000);
await shot(`${out}-flame.png`);
ws.close();
chrome.kill();
process.exit(0);
