// #1249 scratch: open a samply profile (samply load --no-open's URL) in the
// Firefox Profiler in headless Chrome, wait for its symbols, measure the
// symbolicated stacks from the page's own `window.profile`, and screenshot
// the call tree and the flame graph.
//   node profshot.mjs <profiler url> <out prefix> <chrome path>
import { spawn } from 'node:child_process';
import { mkdtempSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';

const [url, out, chromePath, thread] = process.argv.slice(2);
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

async function load(u) {
  await send('Page.navigate', { url: u });
  let last = '', stable = 0;
  for (let i = 0; i < 240; i++) {
    await sleep(1000);
    const text = (await evaluate('document.body ? document.body.innerText : ""')) || '';
    stable = i > 15 && text === last && !/symbolicat/i.test(text) ? stable + 1 : 0;
    last = text;
    if (stable >= 6) break;
    if (i % 20 === 0) console.error(`t=${i}s ${text.slice(0, 200).replace(/\s+/g, ' ')}`);
  }
  writeFileSync(`${out}-text-${Date.now()}.txt`, last);
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
const base = url.replace('/?symbolServer', '/calltree/?symbolServer') + `&thread=${thread}`;
await load(base + '&search=call_updates');
await shot(`${out}-calltree.png`);
await load(url.replace('/?symbolServer', '/flame-graph/?symbolServer') + `&thread=${thread}`);
await shot(`${out}-flame.png`);
ws.close();
chrome.kill();
process.exit(0);
