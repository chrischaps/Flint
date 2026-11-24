// The web player's ?perf=1 readout, page by page (#381): headed Chrome (over
// CDP) or Firefox (over WebDriver BiDi, --browser firefox, #430) plays each scene of a bundle at a fixed device-pixel canvas, waits for the
// readout to fill, reads it a few times ten seconds apart, and saves what it
// read, the page's WebGPU call totals and a screenshot. Every web perf ticket
// used to write this script again; README.md beside it says how to run it.
//
// usage: node tools/web-perf/run.mjs <site-dir|url> <bundle> <scene[+query],...> --out <dir>
//          [--dpr 1,2] [--size 1920x1080] [--seconds 25] [--reads 2]
//          [--query '&fps=native'] [--passes 'Style mask,ASCII'] [--begin auto|click] [--rounds 1]
//          [--port 9381] [--browser chrome|firefox] [--chrome <path>] [--firefox <path>] [--prefs k=v,...] [--trace]
//          [--save-trace] [--isolate]
//
// <site-dir> is served on 127.0.0.1 for the run; a URL is used as it is.
// <bundle> is relative to the page (`games/<id>/<id>.flintpack` in a
// `flint export web` folder). A scene item may carry its own query after '+':
// `ascii_none+style=invert`. --size is the canvas in device pixels at every DPR,
// so the CSS size is size / dpr. Writes <out>/<scene>@dpr<d>.json and .png for
// each page, and <out>/summary.json and <out>/table.md over them all. Exits 1
// when a page never started or drew on another canvas size than asked.
// --rounds N plays every scene once a round, N rounds, in one browser per DPR,
// so heat or another job shows as a trend across rounds, not a step between
// scenes (tools/phone-perf/submits.mjs's alternation, #568, #1318).
// --save-trace presses the readout's Save trace after the last read (a build
// with the scope recorder, `trunk build --features trace`) and keeps the file
// as <out>/<label>.trace.json, summed up by scopes.mjs (#1266). --isolate
// serves the folder cross-origin isolated (COOP + COEP), which a browser
// needs before it gives `performance.now()` its finer step.
import { spawn } from 'node:child_process';
import { createServer } from 'node:http';
import { connect } from 'node:net';
import { copyFileSync, createReadStream, existsSync, mkdirSync, mkdtempSync, readFileSync, readdirSync, rmSync, statSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { extname, join, normalize, resolve, sep } from 'node:path';
import { parsePerfLine, range } from './perfline.mjs';
import { TRACE_JS, summarizeTrace, traceLine } from './trace.mjs';
import { scopesLine, summarizeScopes } from './scopes.mjs';

// ---- arguments -------------------------------------------------------------

const argv = process.argv.slice(2);
const flags = {};
const positional = [];
for (let i = 0; i < argv.length; i++) {
  // the flags with no value
  if (argv[i] === '--trace' || argv[i] === '--save-trace' || argv[i] === '--isolate') flags[argv[i].slice(2)] = true;
  else if (argv[i].startsWith('--')) flags[argv[i].slice(2)] = argv[++i];
  else positional.push(argv[i]);
}
const [site, bundle, sceneList] = positional;
if (!site || !bundle || !sceneList || !flags.out) {
  console.error('usage: node tools/web-perf/run.mjs <site-dir|url> <bundle> <scene,...> --out <dir> [--dpr 1,2] [--size 1920x1080] [--seconds 25] [--reads 2] [--query ...] [--passes ...] [--begin auto|click] [--rounds 1] [--port 9381] [--browser chrome|firefox] [--chrome <path>] [--firefox <path>] [--prefs k=v,...] [--trace] [--save-trace] [--isolate]');
  process.exit(2);
}
const outDir = resolve(flags.out);
const dprs = (flags.dpr ?? '1').split(',').map(Number);
const [width, height] = (flags.size ?? '1920x1080').split('x').map(Number);
const warmSecs = Number(flags.seconds ?? 25);
const reads = Math.max(1, Number(flags.reads ?? 2));
const rounds = Math.max(1, Number(flags.rounds ?? 1));
const extra = flags.query ?? '';
const begin = flags.begin ?? 'auto';
const trace = 'trace' in flags;
const saveTrace = 'save-trace' in flags;
const isolate = 'isolate' in flags;
const extraPrefs = Object.fromEntries(
  (flags.prefs ?? '').split(',').filter(Boolean).map((kv) => {
    const [k, v = ''] = kv.split('=');
    return [k.trim(), v === 'true' ? true : v === 'false' ? false : /^-?\d+$/.test(v) ? Number(v) : v];
  }),
);
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
      // --isolate: cross-origin isolated, so performance.now() steps finely
      // (every file is this server's, so none needs a CORP header)
      ...(isolate ? { 'cross-origin-opener-policy': 'same-origin', 'cross-origin-embedder-policy': 'require-corp' } : {}),
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
  log(`serving ${resolve(site)} at ${base}${isolate ? ', cross-origin isolated' : ''}`);
} else if (isolate) {
  console.error('--isolate needs a folder to serve: a URL sends its own headers');
  process.exit(2);
}

// ---- Chrome, headed, on screen ------------------------------------------------
// Headless Chrome has no WebGPU, and a window off screen or covered gets no
// frames, so the window sits at 0,0 with every throttle off (the flags the web
// sessions found). One Chrome per DPR, launched with --force-device-scale-factor:
// CDP's Emulation.setDeviceMetricsOverride changes devicePixelRatio but not the
// device-pixel box a ResizeObserver reports, which is what winit sizes the
// canvas from, so under it the canvas kept the display's own scale (#381).

// Whether anything accepts a connection on the devtools port. A browser left
// from an earlier run would be driven in place of the one launched (#1266).
const portAnswers = (port) =>
  new Promise((done) => {
    const sock = connect({ host: '127.0.0.1', port });
    sock.once('connect', () => (sock.destroy(), done(true)));
    sock.once('error', () => done(false));
  });
async function refuseHeldPort() {
  if (await portAnswers(cdpPort)) {
    throw new Error(`something already listens on port ${cdpPort} (a browser left from an earlier run?): close it, or --port picks another`);
  }
}

async function openChrome(dpr, css) {
  await refuseHeldPort();
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
  // --save-trace: the page's downloads land in a folder of the run's own
  const downloads = mkdtempSync(join(tmpdir(), 'flint-web-perf-dl-'));
  await send('Browser.setDownloadBehavior', { behavior: 'allow', downloadPath: downloads });
  await send('Page.setDownloadBehavior', { behavior: 'allow', downloadPath: downloads });
  const close = async () => {
    try {
      ws.close();
    } catch {}
    chrome.kill();
    rmSync(downloads, { recursive: true, force: true });
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
  return { navigate, evaluate, screenshot, close, downloads };
}

// ---- Firefox, headed, over WebDriver BiDi (#430) ------------------------------
// Firefox has no CDP. It speaks WebDriver BiDi on --remote-debugging-port, at
// ws://127.0.0.1:<port>/session. It runs on a profile of its own in a temp
// folder (never the user's), with prefs for what Chrome's flags do:
// layout.css.devPixelsPerPx for --force-device-scale-factor, and the
// occlusion and background throttles off.

async function openFirefox(dpr, css) {
  await refuseHeldPort();
  const profile = mkdtempSync(join(tmpdir(), 'flint-web-perf-ff-'));
  // --save-trace: the page's downloads land in a folder of the run's own, unasked
  const downloads = mkdtempSync(join(tmpdir(), 'flint-web-perf-dl-'));
  const prefs = {
    'browser.download.dir': downloads,
    'browser.download.folderList': 2,
    'browser.download.useDownloadDir': true,
    'browser.download.always_ask_before_handling_new_types': false,
    'browser.download.alwaysOpenPanel': false,
    'browser.helperApps.neverAsk.saveToDisk': 'application/json',
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
    // --prefs 'k=v,k=v': more, or these overridden, for one run (#962)
    ...extraPrefs,
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
    // browser.close, not only a kill: on Windows the process spawned is
    // Firefox's launcher, and the browser it starts outlived the kill, still
    // holding the port, so the next run drove the old one (#1266)
    try {
      await send('browser.close', {}, 5000);
      ws.close();
    } catch {}
    firefox.kill();
    for (let i = 0; i < 40 && (await portAnswers(cdpPort)); i++) await sleep(250);
    // Firefox holds its profile (and its downloads) for a moment after it's told to go
    for (let i = 0; i < 20; i++) {
      try {
        rmSync(downloads, { recursive: true, force: true });
        rmSync(profile, { recursive: true, force: true });
        return;
      } catch {
        await sleep(250);
      }
    }
  };
  return { navigate, evaluate, screenshot, close, downloads };
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
const labelFor = (scene, dpr, round) =>
  `${scene.replace(/[^\w.=-]+/g, '_')}@dpr${dpr}${rounds > 1 ? `-r${round + 1}` : ''}`;

// --save-trace (#1266): press the readout's Save trace, as a person would,
// and wait for the file the page offers. The button shows only where the build
// has the scope recorder and ?perf=1 started it.
async function saveTheTrace({ evaluate, downloads }, label) {
  if (!(await evaluate(`!!document.getElementById('perf')?.hasAttribute('data-trace')`))) {
    return { error: 'no Save trace on the page: the build has no scope recorder (trunk build --features trace), or ?scope_bench took its place' };
  }
  const before = new Set(readdirSync(downloads));
  await evaluate(`(document.getElementById('perf-trace').click(), true)`, true);
  const partial = (f) => /\.(crdownload|part|tmp)$/i.test(f);
  let seen = null;
  for (let i = 0; i < 120; i++) {
    await sleep(500);
    const fresh = readdirSync(downloads).filter((f) => !before.has(f) && !partial(f));
    if (!fresh.length || readdirSync(downloads).some(partial)) continue;
    const file = join(downloads, fresh[0]);
    const size = statSync(file).size;
    if (size > 0 && seen?.file === file && seen.size === size) {
      const kept = join(outDir, `${label}.trace.json`);
      copyFileSync(file, kept);
      const saved = { file: kept, offeredAs: fresh[0], bytes: size, button: await evaluate(`document.getElementById('perf-trace')?.textContent`) };
      try {
        const doc = JSON.parse(readFileSync(kept, 'utf8'));
        saved.summary = summarizeScopes(doc);
        saved.header = doc.otherData ?? null;
      } catch (e) {
        saved.error = `the file isn't JSON: ${e.message}`;
      }
      return saved;
    }
    seen = { file, size };
  }
  return { error: 'no file arrived within 60 s of Save trace' };
}

async function playPage(browserHandle, scene, dpr, css, round = 0) {
  const { navigate, evaluate, screenshot } = browserHandle;
  const label = labelFor(scene, dpr, round);
  const url = urlFor(scene, css);
  log(`${label}: ${url}`);
  const result = { label, scene, dpr, round: round + 1, css, url, reads: [] };
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
      crossOriginIsolated: !!window.crossOriginIsolated,
      timestampQuery: !!(await navigator.gpu?.requestAdapter({ powerPreference: 'high-performance' }))?.features.has('timestamp-query'),
    }))()`)) ?? '{}',
  );
  // --trace: every frame the browser gives the page, from here on (#962). The
  // readout's p99 says a window had slow frames; the trace says when, and
  // whether they came one at a time (the frame's own work) or as a spell at
  // half rate (the browser or the machine missing vsync)
  if (trace) await evaluate(TRACE_JS);
  // ?scope_bench=1 (#1266): what a scope cost in this browser, timed before
  // the game began (flint_core::profile::cost)
  const scopeCost = JSON.parse((await evaluate(`JSON.stringify(window.flintScopeCost ?? null)`)) ?? 'null');
  if (scopeCost) {
    result.scopeCost = scopeCost;
    log(`${label} scope bench (${scopeCost.cross_origin_isolated ? '' : 'not '}cross-origin isolated): ${scopeCost.line}`);
  }
  await sleep(warmSecs * 1000);
  for (let r = 0; r < reads; r++) {
    if (r > 0) await sleep(10000); // the readout's window is 10 s: each read is its own
    const text = await evaluate(`document.getElementById('perf-text')?.innerText`);
    const line = await evaluate(`document.getElementById('perf')?.dataset.line`);
    // the page's running totals too: the readout rounds a frame's count to one
    // decimal, the difference of two totals is exact (#565)
    const calls = JSON.parse((await evaluate(`JSON.stringify(window.flintGpuCalls ?? null)`)) ?? 'null');
    result.reads.push({ after: warmSecs + r * 10, text, line, parsed: parsePerfLine(line), calls });
    log(`${label} read ${r + 1}/${reads}: ${line ?? '(no line yet)'}`);
  }
  // exact counts between the first read and the last: `submit` is one a frame
  const [first, last] = [result.reads[0]?.calls, result.reads.at(-1)?.calls];
  if (reads > 1 && first && last) {
    result.between = Object.fromEntries(Object.keys(last).map((k) => [k, last[k] - (first[k] ?? 0)]));
    log(`${label} between reads: ${JSON.stringify(result.between)}`);
  }
  // the canvas the frames were drawn on, as the readout says it, not as asked
  const drawn = result.reads.at(-1)?.parsed.canvas;
  if (!drawn || drawn[0] !== width || drawn[1] !== height) {
    result.warning = `the canvas was ${drawn ? drawn.join('x') : 'unread'}, not ${width}x${height}`;
    log(`${label}: ${result.warning}`);
  }
  result.calls = JSON.parse((await evaluate(`JSON.stringify(window.flintGpuCalls ?? null)`)) ?? 'null');
  if (trace) {
    const t = JSON.parse((await evaluate(`JSON.stringify(window.__flintTrace ?? null)`)) ?? 'null');
    if (t) {
      result.trace = summarizeTrace(t);
      log(`${label} trace: ${traceLine(result.trace)}`);
    }
  }
  const shot = await screenshot();
  if (typeof shot === 'string') writeFileSync(join(outDir, `${label}.png`), Buffer.from(shot, 'base64'));
  else log(`${label}: no screenshot (${shot.why})`);
  // a ?scope_bench page plays without the recorder, so it has no trace to save
  if (saveTrace && !result.scopeCost) {
    result.savedTrace = await saveTheTrace(browserHandle, label);
    const s = result.savedTrace;
    log(`${label} Save trace: ${s.error ?? `${s.offeredAs}, ${(s.bytes / 1e6).toFixed(2)} MB: ${scopesLine(s.summary)}`}`);
  }
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
  // a round plays every scene once, so a scene is read against the others of
  // its own round (#568's alternation)
  for (let round = 0; round < rounds; round++) {
    for (const scene of scenes) {
      const result = await playPage(chrome, scene, dpr, css, round);
      writeFileSync(join(outDir, `${result.label}.json`), JSON.stringify(result, null, 1));
      pages.push(result);
    }
  }
  await chrome.close();
}

// ---- the table ---------------------------------------------------------------

const passNames = flags.passes
  ? flags.passes.split(',').map((s) => s.trim()).filter(Boolean)
  : [...new Set(pages.flatMap((p) => p.reads.flatMap((r) => Object.keys(r.parsed.gpu?.passes ?? {}))))];
const benched = pages.some((p) => p.scopeCost);
const stepText = (ns) => (ns >= 1e6 ? `${(ns / 1e6).toFixed(2)} ms` : ns >= 1e3 ? `${(ns / 1e3).toFixed(1)} µs` : `${Math.round(ns)} ns`);
const over = (p, f) => p.reads.map((r) => f(r.parsed)).filter((v) => v !== undefined);
const sizeOf = (p, f) => [...new Set(over(p, f).map((s) => s.join('x')))].join(', ');
const head = ['page', 'canvas', 'scene', ...passNames.map((n) => `${n} ms`), 'GPU frame ms', 'GPU timed', 'bind groups a frame', 'passes a frame', 'frame p50 ms', 'mean ms a frame', 'GPU-bound', ...(trace ? ['slow frames (spells, longest s)'] : []), ...(benched ? ['a scope recording ns (off, clock step)'] : []), ...(saveTrace ? ['saved trace: scopes read 0'] : [])];
// how the readout says its GPU times were taken (#380): under vsync an idle GPU
// clocks down and its passes read high
const timed = (p) => {
  const g = p.reads.map((r) => r.parsed.gpu).filter((x) => x && x.vsync !== undefined);
  if (!g.length) return '';
  if (!g.some((x) => x.vsync)) return 'no vsync';
  const busy = g.map((x) => x.idleClocks).filter((x) => x !== undefined);
  return busy.length ? `vsync, idle clocks (${range(busy.map((b) => b * 100), 0)}% busy)` : 'vsync';
};
// how many reads said GPU-bound (#1318)
const gpuBound = (p) => {
  const t = p.reads.map((r) => r.parsed.throughput).filter(Boolean);
  if (!t.length) return '';
  const n = t.filter((x) => x.gpuBound).length;
  return n === t.length ? 'yes' : n === 0 ? 'no' : `${n} of ${t.length} reads`;
};
const pageName = (p) => `DPR ${p.dpr}, ${p.scene}${rounds > 1 ? `, round ${p.round}` : ''}`;
const rows = pages.map((p) =>
  p.error
    ? [pageName(p), p.error, ...head.slice(2).map(() => '')]
    : [
        pageName(p),
        sizeOf(p, (x) => x.canvas) + (p.warning ? ' (!)' : ''),
        sizeOf(p, (x) => x.scene),
        ...passNames.map((n) => range(over(p, (x) => x.gpu?.passes?.[n]))),
        range(over(p, (x) => x.gpu?.frame ?? undefined)),
        timed(p),
        range(over(p, (x) => x.webgpu?.bindGroups), 1),
        range(over(p, (x) => x.webgpu?.passes), 1),
        range(over(p, (x) => x.frame?.p50), 1),
        // the window's span over its frames, and how many reads called it the
        // GPU's (#1318): unpaced and GPU-bound, it's the GPU's ms a frame
        range(over(p, (x) => x.throughput?.ms)),
        gpuBound(p),
        ...(trace ? [p.trace ? `${p.trace.slow} (${p.trace.spells.length}, ${Math.max(0, ...p.trace.spells.map((s) => s.lasts))})` : ''] : []),
        ...(benched ? [p.scopeCost ? `${p.scopeCost.on_ns.toFixed(0)} (${p.scopeCost.off_ns.toFixed(1)}, ${stepText(p.scopeCost.clock_step_ns)})` : ''] : []),
        ...(saveTrace ? [p.savedTrace?.summary ? `${p.savedTrace.summary.zero} of ${p.savedTrace.summary.scopes} (${Math.round(p.savedTrace.summary.zeroShare * 100)}%)` : p.savedTrace?.error ?? ''] : []),
      ],
);
const first = pages.find((p) => p.env)?.env;
const adapter = pages.flatMap((p) => p.reads).find((r) => r.parsed.adapter)?.parsed.adapter;
const table = [
  `${browser === 'firefox' ? 'Firefox over WebDriver BiDi' : 'Chrome over CDP'}, ?perf=1, a ${width}x${height} device-pixel canvas, read ${reads}x from ${warmSecs} s after Begin, 10 s apart${rounds > 1 ? `, ${rounds} rounds of every scene` : ''}.`,
  first ? `${first.ua.match(/Chrome\/[\d.]+|Firefox\/[\d.]+/)?.[0] ?? first.ua}, adapter ${adapter ?? 'unread'}, timestamp-query ${first.timestampQuery ? 'on' : 'off'}, ${first.crossOriginIsolated ? '' : 'not '}cross-origin isolated.` : '',
  '',
  `| ${head.join(' | ')} |`,
  `|${head.map(() => '---').join('|')}|`,
  ...rows.map((r) => `| ${r.join(' | ')} |`),
  ...pages.filter((p) => p.warning).map((p) => `\n(!) ${p.label}: ${p.warning}.`),
  '',
].join('\n');
writeFileSync(join(outDir, 'table.md'), table);
writeFileSync(join(outDir, 'summary.json'), JSON.stringify({ base, bundle, size: [width, height], dprs, warmSecs, reads, rounds, pages }, null, 1));
console.log('\n' + table);
log(`wrote ${pages.length} pages to ${outDir}`);
server?.close();
process.exit(pages.some((p) => p.error || p.warning || p.savedTrace?.error) ? 1 : 0);
