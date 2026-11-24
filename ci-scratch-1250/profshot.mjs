// #1250 scratch: the hitch marks in the Firefox Profiler, from samply's
// server (samply load --no-open's URL), in headless Chrome: the marker
// chart filtered to the hitches, and the stack chart over the first one.
//   node profshot.mjs <profiler url> <out prefix> <chrome path> <thread> <range start ms> <range ms>
import { spawn } from 'node:child_process';
import { mkdtempSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';

const [url, out, chromePath, thread, rStart, rLen] = process.argv.slice(2);
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
const port = 9334;

const chrome = spawn(chromePath, [
  '--headless=new', `--remote-debugging-port=${port}`,
  `--user-data-dir=${mkdtempSync(join(tmpdir(), 'profshot-'))}`,
  '--no-first-run', '--no-default-browser-check', '--window-size=1600,900',
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
  return r.result?.result?.value;
};

async function load(u, name) {
  await send('Page.navigate', { url: u });
  let last = '', stable = 0;
  for (let i = 0; i < 240; i++) {
    await sleep(1000);
    const text = (await evaluate('document.body ? document.body.innerText : ""')) || '';
    stable = i > 15 && text === last && !/symbolicat/i.test(text) ? stable + 1 : 0;
    last = text;
    if (stable >= 6) break;
  }
  writeFileSync(`${out}-${name}.txt`, `${u}\n\n${last}`);
  const r = await send('Page.captureScreenshot', { format: 'png' });
  writeFileSync(`${out}-${name}.png`, Buffer.from(r.result.data, 'base64'));
}

await send('Page.enable');
await send('Runtime.enable');
const view = (v) => url.replace('/?symbolServer', `/${v}/?symbolServer`) + `&thread=${thread}`;
await load(view('marker-chart') + '&markerSearch=hitch', 'marker-chart');
await load(view('marker-table') + '&markerSearch=hitch', 'marker-table');
await load(view('stack-chart') + `&range=${rStart}m${rLen}`, 'stack-chart');
await load(view('marker-chart') + `&range=${rStart}m${rLen}`, 'marker-chart-zoom');
ws.close();
chrome.kill();
