// SET + DRIFT on an emulated iPad (Float#3, docs/design/web.md "No Pointer
// Lock API"): desktop Chrome over CDP with touch emulation, an iPad's size
// and user agent, and optionally the APIs iOS WebKit lacks taken away.
// Begin, tap the title, drag to look, drag back fast. A main-thread hang
// shows as Runtime.evaluate timing out; a game that stopped (an exception
// thrown through the event loop) as screenshots that no longer change while
// the page's own rAF counter keeps going: the run ends FROZEN or LIVE.
//   node tools/web-touch/ipad.mjs <url> <outDir> [port] [gaps]
//   node tools/web-touch/ipad.mjs "http://127.0.0.1:8080/?game=float&perf=1" out 9341 coalesced,dpbox,lock
// gaps: comma list of WebKit API gaps to emulate: coalesced
// (getCoalescedEvents), dpbox (devicePixelContentBoxSize), lock (the
// Pointer Lock API, which iOS has none of).
// Needs Chrome at its default Windows path; edit the spawn below elsewhere.
import { spawn } from 'node:child_process';
import { writeFileSync, mkdirSync } from 'node:fs';
import { resolve } from 'node:path';
const [url, outDir, port = '9341', gapArg = ''] = process.argv.slice(2);
const gaps = new Set(gapArg.split(',').filter(Boolean));
mkdirSync(outDir, { recursive: true });
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
const chrome = spawn('C:/Program Files/Google/Chrome/Application/chrome.exe', [
  `--remote-debugging-port=${port}`, `--user-data-dir=${resolve(outDir, 'profile')}`,
  '--enable-unsafe-webgpu', '--no-first-run', '--no-default-browser-check',
  '--no-proxy-server', '--disable-features=NetworkServiceSandbox', '--enable-features=NetworkServiceInProcess',
  '--disable-backgrounding-occluded-windows', '--disable-renderer-backgrounding',
  '--disable-background-timer-throttling', '--autoplay-policy=no-user-gesture-required',
  '--window-size=1180,820', '--force-device-scale-factor=1', 'about:blank',
], { stdio: 'ignore' });
let targets;
for (let i = 0; i < 60; i++) {
  try { targets = await (await fetch(`http://127.0.0.1:${port}/json`)).json(); break; } catch { await sleep(250); }
}
const page = targets.find((t) => t.type === 'page');
const ws = new WebSocket(page.webSocketDebuggerUrl);
await new Promise((r) => ws.addEventListener('open', r));
let id = 0; const pending = new Map(); const log = [];
const t0 = Date.now(); const stamp = () => ((Date.now() - t0) / 1000).toFixed(1);
ws.addEventListener('message', (ev) => {
  const m = JSON.parse(ev.data);
  if (m.id && pending.has(m.id)) { pending.get(m.id)(m); pending.delete(m.id); }
  if (m.method === 'Runtime.consoleAPICalled') log.push(`${stamp()} [${m.params.type}] ` + m.params.args.map((a) => a.value ?? a.description ?? '').join(' '));
  if (m.method === 'Runtime.exceptionThrown') log.push(`${stamp()} [exception] ` + (m.params.exceptionDetails.exception?.description ?? m.params.exceptionDetails.text));
});
const send = (method, params = {}, ms = 8000) => new Promise((r) => {
  const i = ++id; pending.set(i, r); ws.send(JSON.stringify({ id: i, method, params }));
  setTimeout(() => { if (pending.has(i)) { pending.delete(i); r({ timeout: true }); } }, ms);
});
const ev = async (expression, userGesture = false, ms = 8000) => {
  const r = await send('Runtime.evaluate', { expression, returnByValue: true, userGesture }, ms);
  return r.timeout ? { timeout: true } : r.result?.result?.value;
};
const shot = async (n) => { const s = await send('Page.captureScreenshot', { format: 'png' }, 5000); if (s.result) writeFileSync(`${outDir}/${n}.png`, Buffer.from(s.result.data, 'base64')); };
const fingers = new Map();
const flush = (kind) => {
  const touchPoints = [...fingers].map(([fid, [x, y]]) => ({ x, y, id: fid, radiusX: 8, radiusY: 8, force: 1 }));
  return send('Input.dispatchTouchEvent', touchPoints.length ? { type: kind, touchPoints } : { type: 'touchEnd', touchPoints: [] }, 2000);
};
const down = (f, x, y) => { fingers.set(f, [x, y]); return flush('touchStart'); };
const move = (f, x, y) => { fingers.set(f, [x, y]); return flush('touchMove'); };
const up = (f) => { fingers.delete(f); return flush(fingers.size ? 'touchMove' : 'touchEnd'); };
// Liveness: frames the page drew in the last second, and the main thread answering.
const alive = async (label) => {
  const a = await ev(`window.__raf`, false, 3000);
  await sleep(1000);
  const b = await ev(`window.__raf`, false, 3000);
  const r = (a?.timeout || b?.timeout) ? 'MAIN THREAD HUNG (evaluate timed out)' : `${b - a} rAF/s`;
  log.push(`${stamp()} [alive ${label}] ${r}`); console.log(label, r);
  return r;
};

await send('Runtime.enable'); await send('Page.enable');
// iPad Pro 11" landscape, as Chrome on iPadOS presents itself (desktop-class UA).
await send('Emulation.setDeviceMetricsOverride', { width: 1180, height: 820, deviceScaleFactor: 2, mobile: true });
await send('Emulation.setTouchEmulationEnabled', { enabled: true, maxTouchPoints: 5 });
await send('Emulation.setUserAgentOverride', {
  userAgent: 'Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/605.1.15 (KHTML, like Gecko) CriOS/141.0 Mobile/15E148 Safari/604.1',
  platform: 'MacIntel',
});
let pre = `window.__raf = 0; (function f(){ window.__raf++; requestAnimationFrame(f); })();`;
if (gaps.has('coalesced')) pre += `delete PointerEvent.prototype.getCoalescedEvents;`;
if (gaps.has('dpbox')) pre += `delete ResizeObserverEntry.prototype.devicePixelContentBoxSize;`;
if (gaps.has('lock')) pre += `delete Element.prototype.requestPointerLock; delete Document.prototype.exitPointerLock;`;
await send('Page.addScriptToEvaluateOnNewDocument', { source: pre });
await send('Page.navigate', { url });
for (let i = 0; i < 480; i++) {
  const ok = await ev(`!!document.querySelector('#gate[data-ready]')`);
  if (ok === true && log.some((l) => /late bundle/.test(l))) break;
  await sleep(250);
}
await sleep(1500);
await shot('01-gate');
await ev(`document.getElementById('begin').click()`, true);
await sleep(3500);
const dims = await ev(`JSON.stringify([innerWidth, innerHeight, matchMedia('(pointer: coarse)').matches, !!document.pointerLockElement])`);
console.log('viewport', dims);
const [W, H] = JSON.parse(dims);
await alive('after-begin');
await down(1, W / 2, H / 2); await sleep(80); await up(1);          // lift the title
await sleep(2500);
await shot('02-sea');
await alive('after-tap');
const yaw0 = await ev(`document.getElementById('perf')?.dataset.line || ''`);
// Drag to look: across a third of the screen and back.
await down(2, W * 0.4, H * 0.45);
for (let k = 1; k <= 30; k++) { const r = await move(2, W * 0.4 + W * 0.3 * k / 30, H * 0.45 - H * 0.08 * k / 30); if (r.timeout) log.push(`${stamp()} touchMove ${k} unacked`); await sleep(33); }
await shot('03-looked');
await alive('mid-drag');
await up(2);
await sleep(1000);
await alive('after-drag');
// A second, fast, long drag the other way.
await down(3, W * 0.8, H * 0.5);
for (let k = 1; k <= 10; k++) { await move(3, W * 0.8 - W * 0.6 * k / 10, H * 0.5 + H * 0.1 * k / 10); await sleep(16); }
await up(3);
await sleep(1500);
await shot('04-after-fast-drag');
await alive('end');
// The sea always moves, so two identical shots seconds apart mean the game stopped.
const { readFileSync } = await import('node:fs');
const same = (a, b) => readFileSync(`${outDir}/${a}.png`).equals(readFileSync(`${outDir}/${b}.png`));
const frozen = same('02-sea', '03-looked') && same('03-looked', '04-after-fast-drag');
log.push(`${stamp()} [game] ${frozen ? 'FROZEN: the frame stopped changing' : 'LIVE'}`);
console.log(frozen ? 'FROZEN' : 'LIVE');
writeFileSync(`${outDir}/log.txt`, log.join('\n'));
console.log(log.filter((l) => /panic|error|exception|alive|warn|unacked/i.test(l)).slice(-40).join('\n'));
ws.close(); chrome.kill(); process.exit(0);
