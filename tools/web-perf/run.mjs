// The web player's ?perf=1 readout, page by page (#381): headed Chrome (over
// CDP) or Firefox (over WebDriver BiDi, --browser firefox, #430) plays each scene of a bundle at a fixed device-pixel canvas, waits for the
// readout to fill, reads it a few times ten seconds apart, and saves what it
// read, the page's WebGPU call totals and a screenshot. Every web perf ticket
// used to write this script again; README.md beside it says how to run it.
//
// usage: node tools/web-perf/run.mjs <site-dir|url> <bundle> <scene[+query],...> --out <dir>
//          [--dpr 1,2] [--size 1920x1080] [--seconds 25] [--reads 2]
//          [--query '&fps=native'] [--passes 'Style mask,ASCII'] [--begin auto|click]
//          [--port 9381] [--browser chrome|firefox] [--chrome <path>] [--firefox <path>]
//
// <site-dir> is served on 127.0.0.1 for the run; a URL is used as it is.
// <bundle> is relative to the page (`games/<id>/<id>.flintpack` in a
// `flint export web` folder). A scene item may carry its own query after '+':
// `ascii_none+style=invert`. --size is the canvas in device pixels at every DPR,
// so the CSS size is size / dpr. Writes <out>/<scene>@dpr<d>.json and .png for
// each page, and <out>/summary.json and <out>/table.md over them all. Exits 1
// when a page never started or drew on another canvas size than asked.
import { spawn } from 'node:child_process';
import { createServer } from 'node:http';
import { createReadStream, existsSync, mkdirSync, mkdtempSync, rmSync, statSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { extname, join, normalize, resolve, sep } from 'node:path';
import { parsePerfLine, range } from './perfline.mjs';

// ---- arguments -------------------------------------------------------------

const argv = process.argv.slice(2);
const flags = {};
const positional = [];
for (let i = 0; i < argv.length; i++) {
  if (argv[i].startsWith('--')) flags[argv[i].slice(2)] = argv[++i];
  else positional.push(argv[i]);
}
const [site, bundle, sceneList] = positional;
if (!site || !bundle || !sceneList || !flags.out) {
  console.error('usage: node tools/web-perf/run.mjs <site-dir|url> <bundle> <scene,...> --out <dir> [--dpr 1,2] [--size 1920x1080] [--seconds 25] [--reads 2] [--query ...] [--passes ...] [--begin auto|click] [--port 9381] [--browser chrome|firefox] [--chrome <path>] [--firefox <path>]');
  process.exit(2);
}
const outDir = resolve(flags.out);
const dprs = (flags.dpr ?? '1').split(',').map(Number);
const [width, height] = (flags.size ?? '1920x1080').split('x').map(Number);
const warmSecs = Number(flags.seconds ?? 25);
const reads = Math.max(1, Number(flags.reads ?? 2));
const extra = flags.query ?? '';
const begin = flags.begin ?? 'auto';
const cdpPort = Number(flags.port ?? 9381);
const scenes = sceneList.split(',').filter(Boolean);
const chromePath =
  flags.chrome ??
  process.env.CHROME ??
  {
    win32: 'C:/Program Files/Google/Chrome/Application/chrome.exe',
    darwin: '/Applications/Google Chrome.app/Contents/MacOS/Google Chrome',
  }[process.platform] ??
  'google-chrome';
const browser = flags.browser ?? 'chrome';
const firefoxPath =
  flags.firefox ??
  process.env.FIREFOX ??
  {
    win32: 'C:/Program Files/Mozilla Firefox/firefox.exe',
    darwin: '/Applications/Firefox.app/Contents/MacOS/firefox',
  }[process.platform] ??
  'firefox';
if (browser !== 'chrome' && browser !== 'firefox') {
  console.error('--browser is chrome or firefox');
  process.exit(2);
}
if (![width, height, warmSecs, ...dprs].every((n) => Number.isFinite(n) && n > 0)) {
  console.error('--size, --seconds and --dpr take positive numbers');
  process.exit(2);
}
if (begin !== 'auto' && begin !== 'click') {
  console.error('--begin is auto (?begin=auto) or click (a click over CDP)');
  process.exit(2);
}
mkdirSync(outDir, { recursive: true });
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
const log = (s) => console.log(`[web-perf] ${s}`);

// ---- a static server for a site folder ---------------------------------------

const TYPES = {
  '.html': 'text/html; charset=utf-8', '.js': 'text/javascript', '.mjs': 'text/javascript',
  '.wasm': 'application/wasm', '.json': 'application/json', '.css': 'text/css',
  '.png': 'image/png', '.jpg': 'image/jpeg', '.svg': 'image/svg+xml', '.webp': 'image/webp',
  '.mp4': 'video/mp4', '.webm': 'video/webm', '.ogg': 'audio/ogg', '.toml': 'text/plain; charset=utf-8',
};

async function serve(dir) {
  const root = resolve(dir);
  const server = createServer((req, res) => {
    const path = decodeURIComponent(new URL(req.url, 'http://x').pathname);
    let file = normalize(join(root, path));
    if (file !== root && !file.startsWith(root + sep)) return res.writeHead(403).end();
    if (existsSync(file) && statSync(file).isDirectory()) file = join(file, 'index.html');
    if (!existsSync(file)) return res.writeHead(404).end();
    res.writeHead(200, {
      'content-type': TYPES[extname(file).toLowerCase()] ?? 'application/octet-stream',
      'content-length': statSync(file).size,
      'cache-control': 'no-store',
    });
    createReadStream(file).pipe(res);
  });
  await new Promise((r) => server.listen(0, '127.0.0.1', r));
  return { server, base: `http://127.0.0.1:${server.address().port}` };
}

let server = null;
let base = site.replace(/\/$/, '');
if (!/^https?:\/\//.test(site)) {
  if (!existsSync(join(site, 'index.html'))) {
    console.error(`${site} has no index.html: give a trunk build or a \`flint export web\` folder, or a URL`);
    process.exit(2);
  }
  ({ server, base } = await serve(site));
  log(`serving ${resolve(site)} at ${base}`);
}

// ---- Chrome, headed, on screen ------------------------------------------------
// Headless Chrome has no WebGPU, and a window off screen or covered gets no
// frames, so the window sits at 0,0 with every throttle off (the flags the web
// sessions found). One Chrome per DPR, launched with --force-device-scale-factor:
// CDP's Emulation.setDeviceMetricsOverride changes devicePixelRatio but not the
// device-pixel box a ResizeObserver reports, which is what winit sizes the
// canvas from, so under it the canvas kept the display's own scale (#381).

async function openChrome(dpr, css) {
  const profile = mkdtempSync(join(tmpdir(), 'flint-web-perf-'));
  const chrome = spawn(chromePath, [
    `--remote-debugging-port=${cdpPort}`, `--user-data-dir=${profile}`,
    '--enable-unsafe-webgpu', '--no-first-run', '--no-default-browser-check',
    '--no-proxy-server',
    '--disable-features=NetworkServiceSandbox,CalculateNativeWinOcclusion',
    '--enable-features=NetworkServiceInProcess',
    '--disable-backgrounding-occluded-windows', '--disable-renderer-backgrounding',
    '--disable-background-timer-throttling', '--autoplay-policy=no-user-gesture-required',
    `--force-device-scale-factor=${dpr}`,
    '--window-position=0,0', `--window-size=${css[0] + 60},${css[1] + 160}`, 'about:blank',
  ], { stdio: 'ignore' });
  let failed = null;
  chrome.on('error', (e) => (failed = e));
  let targets;
  for (let i = 0; i < 80 && !targets && !failed; i++) {
    await sleep(250);
    try {
      targets = await (await fetch(`http://127.0.0.1:${cdpPort}/json`)).json();
    } catch {}
  }
  if (failed) throw new Error(`couldn't start Chrome at ${chromePath}: ${failed.message} (--chrome or CHROME names another)`);
  const page = targets?.find((t) => t.type === 'page');
  if (!page) {
    chrome.kill();
    throw new Error(`no Chrome page on CDP port ${cdpPort} (is another Chrome holding it? --port picks another)`);
  }
  const ws = new WebSocket(page.webSocketDebuggerUrl);
  await new Promise((r) => ws.addEventListener('open', r));
  let nextId = 0;
  const pending = new Map();
  ws.addEventListener('message', (ev) => {
    const m = JSON.parse(ev.data);
    if (m.id && pending.has(m.id)) {
      pending.get(m.id)(m);
      pending.delete(m.id);
    }
  });
  // every call has a timeout: a page that stops drawing stops answering some
  const send = (method, params = {}, ms = 15000) =>
    new Promise((r) => {
      const id = ++nextId;
      pending.set(id, r);
      ws.send(JSON.stringify({ id, method, params }));
      setTimeout(() => {
        if (pending.has(id)) {
          pending.delete(id);
          r({ timeout: true });
        }
      }, ms);
    });
  const evaluate = async (expression, userGesture = false) =>
    (await send('Runtime.evaluate', { expression, returnByValue: true, awaitPromise: true, userGesture })).result?.result?.value;
  await send('Runtime.enable');
  await send('Page.enable');
  await send('Page.bringToFront');
  const close = async () => {
    try {
      ws.close();
    } catch {}
    chrome.kill();
    // Chrome holds its profile for a moment after it's told to go
    for (let i = 0; i < 20; i++) {
      try {
        rmSync(profile, { recursive: true, force: true });
        return;
      } catch {
        await sleep(250);
      }
    }
  };
  const navigate = (url) => send('Page.navigate', { url });
  const screenshot = async () => {
    const shot = await send('Page.captureScreenshot', { format: 'png' });
    return shot.result ? shot.result.data : { why: shot.timeout ? 'timed out' : JSON.stringify(shot.error) };
  };
  return { navigate, evaluate, screenshot, close };
}

// ---- Firefox, headed, over WebDriver BiDi (#430) ------------------------------
// Firefox has no CDP. It speaks WebDriver BiDi on --remote-debugging-port, at
// ws://127.0.0.1:<port>/session. It runs on a profile of its own in a temp
// folder (never the user's), with prefs for what Chrome's flags do:
// layout.css.devPixelsPerPx for --force-device-scale-factor, and the
// occlusion and background throttles off.

async function openFirefox(dpr, css) {
  const profile = mkdtempSync(join(tmpdir(), 'flint-web-perf-ff-'));
  const prefs = {
    'layout.css.devPixelsPerPx': String(dpr),
    'dom.webgpu.enabled': true,
    'browser.shell.checkDefaultBrowser': false,
    'browser.startup.homepage_override.mstone': 'ignore',
    'browser.aboutwelcome.enabled': false,
    'datareporting.policy.dataSubmissionEnabled': false,
    'toolkit.telemetry.reportingpolicy.firstRun': false,
    'widget.windows.window_occlusion_tracking.enabled': false,
    'dom.min_background_timeout_value': 4,
    'media.autoplay.default': 0,
    'media.autoplay.blocking_policy': 0,
    'network.proxy.type': 0,
  };
  const lines = Object.entries(prefs).map(([k, v]) => `user_pref(${JSON.stringify(k)}, ${JSON.stringify(v)});`);
  writeFileSync(join(profile, 'user.js'), lines.join('\n') + '\n');
  const firefox = spawn(firefoxPath, [
    '--remote-debugging-port', String(cdpPort), '-profile', profile, '-no-remote', '-new-instance',
    '-width', String(css[0] + 40), '-height', String(css[1] + 140),
  ], { stdio: 'ignore' });
  let failed = null;
  firefox.on('error', (e) => (failed = e));
  let ws;
  for (let i = 0; i < 80 && !ws && !failed; i++) {
    await sleep(250);
    try {
      const sock = new WebSocket(`ws://127.0.0.1:${cdpPort}/session`);
      await new Promise((ok, no) => {
        sock.addEventListener('open', ok);
        sock.addEventListener('error', no);
      });
      ws = sock;
    } catch {}
  }
  if (failed) throw new Error(`couldn't start Firefox at ${firefoxPath}: ${failed.message} (--firefox or FIREFOX names another)`);
  if (!ws) {
    firefox.kill();
    throw new Error(`no WebDriver BiDi on port ${cdpPort} (is another Firefox holding it? --port picks another)`);
  }
  let nextId = 0;
  const pending = new Map();
  ws.addEventListener('message', (ev) => {
    const m = JSON.parse(ev.data);
    if (m.id && pending.has(m.id)) {
      pending.get(m.id)(m);
      pending.delete(m.id);
    }
  });
  const send = (method, params = {}, ms = 15000) =>
    new Promise((r) => {
      const id = ++nextId;
      pending.set(id, r);
      ws.send(JSON.stringify({ id, method, params }));
      setTimeout(() => {
        if (pending.has(id)) {
          pending.delete(id);
          r({ timeout: true });
        }
      }, ms);
    });
  const started = await send('session.new', { capabilities: {} });
  if (started.type !== 'success') {
    firefox.kill();
    throw new Error(`Firefox refused a BiDi session: ${JSON.stringify(started.error ?? started)}`);
  }
  const tree = await send('browsingContext.getTree', {});
  const context = tree.result?.contexts?.[0]?.context;
  await send('browsingContext.activate', { context });
  // BiDi hands back typed values; the driver only asks for primitives
  const evaluate = async (expression, userActivation = false) => {
    const m = await send('script.evaluate', {
      expression, target: { context }, awaitPromise: true, resultOwnership: 'none', userActivation,
    });
    const v = m.result?.result;
    return v && 'value' in v ? v.value : undefined;
  };
  const navigate = (url) => send('browsingContext.navigate', { context, url, wait: 'none' });
  const screenshot = async () => {
    const shot = await send('browsingContext.captureScreenshot', { context });
    return shot.result ? shot.result.data : { why: shot.timeout ? 'timed out' : JSON.stringify(shot.error ?? shot.message) };
  };
  const close = async () => {
    try {
      await send('session.end', {}, 3000);
      ws.close();
    } catch {}
    firefox.kill();
    // Firefox holds its profile for a moment after it's told to go
    for (let i = 0; i < 20; i++) {
      try {
        rmSync(profile, { recursive: true, force: true });
        return;
      } catch {
        await sleep(250);
      }
    }
  };
  return { navigate, evaluate, screenshot, close };
}

// ---- each page -------------------------------------------------------------

const urlFor = (scene, css) => {
  const [name, ...query] = scene.split('+');
  const file = name.endsWith('.toml') ? name : `${name}.scene.toml`;
  return (
    `${base}/?bundle=${encodeURIComponent(bundle).replace(/%2F/g, '/')}&scene=${encodeURIComponent(file)}` +
    `&perf=1&width=${css[0]}&height=${css[1]}${begin === 'auto' ? '&begin=auto' : ''}` +
    `${extra}${query.map((q) => '&' + q).join('')}`
  );
};
const labelFor = (scene, dpr) => `${scene.replace(/[^\w.=-]+/g, '_')}@dpr${dpr}`;

async function playPage({ navigate, evaluate, screenshot }, scene, dpr, css) {
  const label = labelFor(scene, dpr);
  const url = urlFor(scene, css);
  log(`${label}: ${url}`);
  const result = { label, scene, dpr, css, url, reads: [] };
  await navigate(url);
  // the gate lifts once the scene is loaded and the player is running
  const want = begin === 'auto' ? 'data-gone' : 'data-ready';
  let status = '';
  for (let i = 0; i < 240; i++) {
    if (await evaluate(`!!document.getElementById('gate')?.hasAttribute('${want}')`)) break;
    status = (await evaluate(`document.getElementById('status')?.textContent`)) ?? '';
    if (/couldn't start/.test(status)) break;
    await sleep(500);
  }
  if (begin === 'click' && (await evaluate(`!!document.getElementById('gate')?.hasAttribute('data-ready')`))) {
    await evaluate(`document.getElementById('begin').click()`, true);
  }
  if (!(await evaluate(`!!document.getElementById('gate')?.hasAttribute('data-gone')`))) {
    result.error = status || 'the gate never lifted';
    log(`${label}: ${result.error}`);
    return result;
  }
  result.env = JSON.parse(
    (await evaluate(`(async () => JSON.stringify({
      dpr: devicePixelRatio, viewport: [innerWidth, innerHeight],
      canvas: [document.querySelector('canvas')?.width, document.querySelector('canvas')?.height],
      ua: navigator.userAgent,
      timestampQuery: !!(await navigator.gpu?.requestAdapter({ powerPreference: 'high-performance' }))?.features.has('timestamp-query'),
    }))()`)) ?? '{}',
  );
  await sleep(warmSecs * 1000);
  for (let r = 0; r < reads; r++) {
    if (r > 0) await sleep(10000); // the readout's window is 10 s: each read is its own
    const text = await evaluate(`document.getElementById('perf-text')?.innerText`);
    const line = await evaluate(`document.getElementById('perf')?.dataset.line`);
    result.reads.push({ after: warmSecs + r * 10, text, line, parsed: parsePerfLine(line) });
    log(`${label} read ${r + 1}/${reads}: ${line ?? '(no line yet)'}`);
  }
  // the canvas the frames were drawn on, as the readout says it, not as asked
  const drawn = result.reads.at(-1)?.parsed.canvas;
  if (!drawn || drawn[0] !== width || drawn[1] !== height) {
    result.warning = `the canvas was ${drawn ? drawn.join('x') : 'unread'}, not ${width}x${height}`;
    log(`${label}: ${result.warning}`);
  }
  result.calls = JSON.parse((await evaluate(`JSON.stringify(window.flintGpuCalls ?? null)`)) ?? 'null');
  const shot = await screenshot();
  if (typeof shot === 'string') writeFileSync(join(outDir, `${label}.png`), Buffer.from(shot, 'base64'));
  else log(`${label}: no screenshot (${shot.why})`);
  return result;
}

const pages = [];
for (const dpr of dprs) {
  const css = [Math.round(width / dpr), Math.round(height / dpr)];
  let chrome;
  try {
    chrome = await (browser === 'firefox' ? openFirefox : openChrome)(dpr, css);
  } catch (e) {
    console.error(e.message);
    server?.close();
    process.exit(1);
  }
  for (const scene of scenes) {
    const result = await playPage(chrome, scene, dpr, css);
    writeFileSync(join(outDir, `${result.label}.json`), JSON.stringify(result, null, 1));
    pages.push(result);
  }
  await chrome.close();
}

// ---- the table ---------------------------------------------------------------

const passNames = flags.passes
  ? flags.passes.split(',').map((s) => s.trim()).filter(Boolean)
  : [...new Set(pages.flatMap((p) => p.reads.flatMap((r) => Object.keys(r.parsed.gpu?.passes ?? {}))))];
const over = (p, f) => p.reads.map((r) => f(r.parsed)).filter((v) => v !== undefined);
const sizeOf = (p, f) => [...new Set(over(p, f).map((s) => s.join('x')))].join(', ');
const head = ['page', 'canvas', 'scene', ...passNames.map((n) => `${n} ms`), 'GPU frame ms', 'GPU timed', 'bind groups a frame', 'passes a frame', 'frame p50 ms'];
// how the readout says its GPU times were taken (#380): under vsync an idle GPU
// clocks down and its passes read high
const timed = (p) => {
  const g = p.reads.map((r) => r.parsed.gpu).filter((x) => x && x.vsync !== undefined);
  if (!g.length) return '';
  if (!g.some((x) => x.vsync)) return 'no vsync';
  const busy = g.map((x) => x.idleClocks).filter((x) => x !== undefined);
  return busy.length ? `vsync, idle clocks (${range(busy.map((b) => b * 100), 0)}% busy)` : 'vsync';
};
const rows = pages.map((p) =>
  p.error
    ? [`DPR ${p.dpr}, ${p.scene}`, p.error, ...head.slice(2).map(() => '')]
    : [
        `DPR ${p.dpr}, ${p.scene}`,
        sizeOf(p, (x) => x.canvas) + (p.warning ? ' (!)' : ''),
        sizeOf(p, (x) => x.scene),
        ...passNames.map((n) => range(over(p, (x) => x.gpu?.passes?.[n]))),
        range(over(p, (x) => x.gpu?.frame ?? undefined)),
        timed(p),
        range(over(p, (x) => x.webgpu?.bindGroups), 1),
        range(over(p, (x) => x.webgpu?.passes), 1),
        range(over(p, (x) => x.frame?.p50), 1),
      ],
);
const first = pages.find((p) => p.env)?.env;
const adapter = pages.flatMap((p) => p.reads).find((r) => r.parsed.adapter)?.parsed.adapter;
const table = [
  `${browser === 'firefox' ? 'Firefox over WebDriver BiDi' : 'Chrome over CDP'}, ?perf=1, a ${width}x${height} device-pixel canvas, read ${reads}x from ${warmSecs} s after Begin, 10 s apart.`,
  first ? `${first.ua.match(/Chrome\/[\d.]+/)?.[0] ?? first.ua}, adapter ${adapter ?? 'unread'}, timestamp-query ${first.timestampQuery ? 'on' : 'off'}.` : '',
  '',
  `| ${head.join(' | ')} |`,
  `|${head.map(() => '---').join('|')}|`,
  ...rows.map((r) => `| ${r.join(' | ')} |`),
  ...pages.filter((p) => p.warning).map((p) => `\n(!) ${p.label}: ${p.warning}.`),
  '',
].join('\n');
writeFileSync(join(outDir, 'table.md'), table);
writeFileSync(join(outDir, 'summary.json'), JSON.stringify({ base, bundle, size: [width, height], dprs, warmSecs, reads, pages }, null, 1));
console.log('\n' + table);
log(`wrote ${pages.length} pages to ${outDir}`);
server?.close();
process.exit(pages.some((p) => p.error || p.warning) ? 1 : 0);
