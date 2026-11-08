// #378: play each ASCII scene on the Pixel 8 Pro's Chrome with ?perf=1 and read the
// overlay. The phone loads http://localhost:PORT through `adb reverse` (a secure
// context), Chrome's devtools socket is forwarded to CDP_PORT, and only our own
// localhost tab is touched.
// usage: [EXTRA='&fps=native'] node phoneperf.mjs <outDir> <scenes,comma> [warmSeconds] [samples]
// A scene item may carry its own query after '+': ascii_none+style=invert.
// log.txt keeps every load; phone.json keeps the last load of each scene.
import { execFileSync } from 'node:child_process';
import { writeFileSync, mkdirSync, appendFileSync } from 'node:fs';
// winget's platform-tools on Chris's machine; set ADB elsewhere (forward slashes)
const ADB = process.env.ADB ?? 'C:/Users/chris/AppData/Local/Microsoft/WinGet/Packages/Google.PlatformTools_Microsoft.Winget.Source_8wekyb3d8bbwe/platform-tools/adb.exe';
const PORT = 8378, CDP = 9378;
const [outDir, sceneList, warm = '40', samples = '3'] = process.argv.slice(2);
mkdirSync(outDir, { recursive: true });
const log = (s) => { console.log(s); appendFileSync(`${outDir}/log.txt`, s + '\n'); };
const adb = (...a) => execFileSync(ADB, a, { encoding: 'utf8' });
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
const thermal = () => {
  const t = adb('shell', 'dumpsys', 'thermalservice');
  const status = /Thermal Status: (\d)/.exec(t)?.[1];
  const hal = t.split('Current temperatures from HAL')[1] ?? t;
  const skin = /mName=VIRTUAL-SKIN, mStatus/.test(hal) ? /mValue=([\d.]+), mType=3, mName=VIRTUAL-SKIN,/.exec(hal)?.[1] : null;
  return { status: Number(status), skin: skin ? Number(Number(skin).toFixed(1)) : null };
};
adb('reverse', `tcp:${PORT}`, `tcp:${PORT}`);
const base = `http://localhost:${PORT}/?bundle=asciiweb.flintpack&perf=1${process.env.EXTRA ?? ''}`;
// a scene item may carry its own query after '+': ascii_none+style=invert
const url = (s) => { const [name, ...q] = s.split('+'); return `${base}&scene=${name}.scene.toml${q.map((x) => '&' + x).join('')}`; };
const fileName = (s) => s.replace(/[^\w.-]+/g, '_');
const scenes = sceneList.split(',');
adb('shell', 'am', 'start', '-a', 'android.intent.action.VIEW', '-d', `'${url(scenes[0])}'`, 'com.android.chrome');
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
const send = (method, params = {}, ms = 15000) => new Promise((r) => { const i = ++id; pending.set(i, r); ws.send(JSON.stringify({ id: i, method, params })); setTimeout(() => { if (pending.has(i)) { pending.delete(i); r({ timeout: true }); } }, ms); });
const ev = async (expression, userGesture = false) => (await send('Runtime.evaluate', { expression, returnByValue: true, awaitPromise: true, userGesture })).result?.result?.value;
await send('Runtime.enable'); await send('Page.enable');
const results = {};
for (const [n, scene] of scenes.entries()) {
  if (n > 0) await send('Page.navigate', { url: url(scene) });
  await sleep(1500);
  for (let i = 0; i < 120 && (await ev(`!!document.getElementById('begin')?.offsetParent`)) !== true; i++) await sleep(500);
  await ev(`document.getElementById('begin').click()`, true);
  const before = thermal();
  const env = await ev(`(async () => JSON.stringify({dpr: devicePixelRatio, w: innerWidth, h: innerHeight, canvas: [document.querySelector('canvas')?.width, document.querySelector('canvas')?.height], ua: navigator.userAgent, ts: !!navigator.gpu && !!(await navigator.gpu.requestAdapter())?.features.has('timestamp-query')}))()`);
  log(`${scene}: begin, thermal ${JSON.stringify(before)} env ${env}`);
  await sleep(Number(warm) * 1000);
  const reads = [];
  for (let s = 0; s < Number(samples); s++) {
    const text = await ev(`document.getElementById('perf-text')?.innerText`);
    const line = await ev(`document.getElementById('perf')?.dataset.line`);
    reads.push({ t: Number(warm) + s * 10, text, line, thermal: thermal() });
    log(`--- ${scene} sample ${s}\n${text}\nline: ${line}`);
    if (s + 1 < Number(samples)) await sleep(10000);
  }
  const shot = await send('Page.captureScreenshot', { format: 'png' }, 15000);
  if (shot.result) writeFileSync(`${outDir}/${fileName(scene)}.png`, Buffer.from(shot.result.data, 'base64'));
  results[scene] = { env: JSON.parse(env || '{}'), before, reads, calls: await ev(`JSON.stringify(window.flintGpuCalls)`) };
  writeFileSync(`${outDir}/phone.json`, JSON.stringify(results, null, 1));
}
await send('Page.navigate', { url: 'about:blank' });
try { await fetch(`http://127.0.0.1:${CDP}/json/close/${target.id}`); } catch {}
ws.close();
log('done');
process.exit(0);
