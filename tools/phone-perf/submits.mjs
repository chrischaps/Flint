// #568: what a pass costs on the phone, where Mali's timestamp-query reads
// 0.00 for every pass after Scene. Counts the player's submissions (one a
// frame, #139) over 10 s by wrapping `GPUQueue.submit` in the page, and the
// render passes recorded between them. Unpaced (`fps=native`) and GPU-bound,
// the frames in 10 s are the GPU's throughput: 10 000 / frames is the GPU's
// ms a frame, which doesn't snap to whole refreshes as one frame's time does.
// Make the scene nearly free with BASE (`&scale=0.25&quality=ssao=0,...`) and
// what's left is the display stage's passes, at the canvas's full size.
//
// usage: [BASE='&...'] node submits.mjs <outDir> "<item>;<item>;..." [warmSeconds] [rounds]
// An item is a scene with its own query after '+':
//   ascii_none+style=ascii:edges=0,color=mono
// split on ';', so a style word keeps its commas. `dir/scene` plays the site
// copy in `dir/` (a patched wasm). Each round plays every item once, in order,
// so heat shows as a trend across rounds rather than as a step between items.
// Same adb/CDP setup as phoneperf.mjs: take the pixel lock first.
import { execFileSync } from 'node:child_process';
import { writeFileSync, mkdirSync, appendFileSync } from 'node:fs';
const ADB = process.env.ADB ?? 'C:/Users/chris/AppData/Local/Microsoft/WinGet/Packages/Google.PlatformTools_Microsoft.Winget.Source_8wekyb3d8bbwe/platform-tools/adb.exe';
const PORT = 8378, CDP = 9378;
const PACK = process.env.PACK ?? 'asciiweb.flintpack';
const BASE = process.env.BASE ?? '';
const [outDir, itemList, warm = '15', rounds = '2'] = process.argv.slice(2);
mkdirSync(outDir, { recursive: true });
const log = (s) => { console.log(s); appendFileSync(`${outDir}/log.txt`, s + '\n'); };
const adb = (...a) => execFileSync(ADB, a, { encoding: 'utf8' });
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
const thermal = () => {
  const t = adb('shell', 'dumpsys', 'thermalservice');
  const status = /Thermal Status: (\d)/.exec(t)?.[1];
  const hal = t.split('Current temperatures from HAL')[1] ?? t;
  const skin = /mValue=([\d.]+), mType=3, mName=VIRTUAL-SKIN,/.exec(hal)?.[1];
  return { status: Number(status), skin: skin ? Number(Number(skin).toFixed(1)) : null };
};
adb('reverse', `tcp:${PORT}`, `tcp:${PORT}`);
const url = (s) => {
  const [head, ...q] = s.split('+');
  const [dir, name] = head.includes('/') ? head.split('/') : ['', head];
  // a copy reads the root's pack (one download, cached)
  return `http://localhost:${PORT}/${dir ? dir + '/' : ''}?bundle=${dir ? '../' : ''}${PACK}&fps=native&begin=auto${BASE}&scene=${name}.scene.toml${q.map((x) => '&' + x).join('')}`;
};
const items = itemList.split(';');
adb('shell', 'am', 'start', '-a', 'android.intent.action.VIEW', '-d', `'${url(items[0])}'`, 'com.android.chrome');
await sleep(3000);
adb('forward', `tcp:${CDP}`, 'localabstract:chrome_devtools_remote');
let target;
for (let i = 0; i < 40 && !target; i++) {
  try {
    const list = await (await fetch(`http://127.0.0.1:${CDP}/json`)).json();
    target = list.find((t) => t.type === 'page' && t.url.startsWith(`http://localhost:${PORT}`));
  } catch {}
  if (!target) await sleep(500);
}
if (!target) { log('no localhost tab found'); process.exit(1); }
const ws = new WebSocket(target.webSocketDebuggerUrl);
await new Promise((r) => ws.addEventListener('open', r));
let id = 0; const pending = new Map();
ws.addEventListener('message', (ev) => { const m = JSON.parse(ev.data); if (m.id && pending.has(m.id)) { pending.get(m.id)(m); pending.delete(m.id); } });
const send = (method, params = {}, ms = 30000) => new Promise((r) => { const i = ++id; pending.set(i, r); ws.send(JSON.stringify({ id: i, method, params })); setTimeout(() => { if (pending.has(i)) { pending.delete(i); r({ timeout: true }); } }, ms); });
const ev = async (expression) => (await send('Runtime.evaluate', { expression, returnByValue: true, awaitPromise: true })).result?.result?.value;
await send('Runtime.enable'); await send('Page.enable');
// Wraps submit for `secs` seconds: the submissions, the time between them,
// and the render passes recorded before each
const probe = (secs) => `(async () => {
  const q = GPUQueue.prototype, e = GPUCommandEncoder.prototype;
  const realSubmit = q.submit, realPass = e.beginRenderPass;
  const gaps = [], passes = []; let open = 0, last = null;
  e.beginRenderPass = function (...a) { open++; return realPass.apply(this, a); };
  q.submit = function (...a) {
    const now = performance.now();
    if (last !== null) gaps.push(now - last);
    last = now; passes.push(open); open = 0;
    return realSubmit.apply(this, a);
  };
  const t0 = performance.now();
  await new Promise((r) => setTimeout(r, ${secs * 1000}));
  const span = performance.now() - t0;
  q.submit = realSubmit; e.beginRenderPass = realPass;
  const s = gaps.slice().sort((a, b) => a - b), at = (p) => s[Math.min(s.length - 1, Math.floor(p * s.length))];
  const mid = passes.slice().sort((a, b) => a - b)[Math.floor(passes.length / 2)] ?? 0;
  const c = document.querySelector('canvas');
  return JSON.stringify({ frames: passes.length, ms: span / Math.max(passes.length, 1), p50: at(0.5), p90: at(0.9), passes: mid, canvas: [c?.width, c?.height] });
})()`;
const results = [];
for (let round = 0; round < Number(rounds); round++) {
  for (const [n, item] of items.entries()) {
    if (round > 0 || n > 0) await send('Page.navigate', { url: url(item) });
    // Warm from the first drawn frame, not from the navigation: a pack
    // can take seconds to arrive
    for (let i = 0; i < 240 && !((await ev(`document.querySelector('canvas')?.width > 300`)) ?? false); i++) await sleep(250);
    await sleep(Number(warm) * 1000);
    const before = thermal();
    const r = JSON.parse((await ev(probe(10))) ?? '{}');
    results.push({ round, item, ...r, thermal: before });
    const f = (x) => (x ?? NaN).toFixed(2).padStart(6);
    log(`r${round} ${item.padEnd(56)} ${String(r.frames).padStart(5)} frames  ${f(r.ms)} ms a frame  p50 ${f(r.p50)} p90 ${f(r.p90)}  passes ${r.passes}  canvas ${r.canvas}  thermal ${before.status}/${before.skin}`);
    writeFileSync(`${outDir}/submits.json`, JSON.stringify(results, null, 1));
  }
}
await send('Page.navigate', { url: 'about:blank' });
ws.close();
// Leave no tab of ours behind: a live one draws beside the next run
try { await fetch(`http://127.0.0.1:${CDP}/json/close/${target.id}`); } catch {}
log('done');
process.exit(0);
