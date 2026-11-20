// Spike #963: what a browser's WebGPU adapter offers. Serves one page on
// 127.0.0.1, opens it in a throwaway profile of the browser, takes the page's
// POST of navigator.gpu's answer, writes <out>.json and closes the browser.
// usage: node webgpu_features.mjs chrome|firefox <out.json>
import { spawn } from 'node:child_process';
import { createServer } from 'node:http';
import { mkdtempSync, writeFileSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';

const [browser, out] = process.argv.slice(2);
const page = `<!doctype html><meta charset=utf-8><title>probe</title><body><script>
(async () => {
  const r = { ua: navigator.userAgent };
  try {
    if (!navigator.gpu) throw new Error('no navigator.gpu');
    const a = await navigator.gpu.requestAdapter({ powerPreference: 'high-performance' });
    if (!a) throw new Error('no adapter');
    r.features = [...a.features].sort();
    r.wgslLanguageFeatures = [...(navigator.gpu.wgslLanguageFeatures ?? [])].sort();
    const lim = {};
    for (const k in a.limits) lim[k] = a.limits[k];
    r.limits = lim;
    const i = a.info ?? {};
    r.info = { vendor: i.vendor, architecture: i.architecture, device: i.device, description: i.description,
               subgroupMinSize: i.subgroupMinSize, subgroupMaxSize: i.subgroupMaxSize, isFallbackAdapter: i.isFallbackAdapter };
    r.preferredCanvasFormat = navigator.gpu.getPreferredCanvasFormat();
    // HDR canvas: does configure() take toneMapping 'extended'?
    try {
      const d = await a.requestDevice();
      const c = document.createElement('canvas').getContext('webgpu');
      c.configure({ device: d, format: 'rgba16float', toneMapping: { mode: 'extended' } });
      r.hdrCanvas = (c.getConfiguration?.()?.toneMapping?.mode) ?? 'configured (no getConfiguration)';
    } catch (e) { r.hdrCanvas = 'refused: ' + e.message; }
    r.dynamicRange = matchMedia('(dynamic-range: high)').matches ? 'high' : 'standard';
  } catch (e) { r.error = String(e); }
  await fetch('/report', { method: 'POST', body: JSON.stringify(r) });
  document.body.textContent = 'done';
})();
</script>`;

const exe = {
  chrome: 'C:/Program Files/Google/Chrome/Application/chrome.exe',
  firefox: 'C:/Program Files/Mozilla Firefox/firefox.exe',
}[browser];
const profile = mkdtempSync(join(tmpdir(), 'spike963-'));
let child;
const server = createServer((req, res) => {
  if (req.method === 'POST') {
    let body = '';
    req.on('data', (c) => (body += c));
    req.on('end', () => {
      writeFileSync(out, JSON.stringify(JSON.parse(body), null, 2));
      res.end('ok');
      console.log('wrote', out);
      setTimeout(() => {
        child.kill();
        server.close();
        try { rmSync(profile, { recursive: true, force: true }); } catch {}
        process.exit(0);
      }, 500);
    });
    return;
  }
  res.setHeader('content-type', 'text/html');
  res.end(page);
});
server.listen(0, '127.0.0.1', () => {
  const url = `http://127.0.0.1:${server.address().port}/`;
  const args = browser === 'chrome'
    ? [`--user-data-dir=${profile}`, '--no-first-run', '--no-default-browser-check', '--new-window', url]
    : ['-no-remote', '-new-instance', '-profile', profile, url];
  child = spawn(exe, args, { stdio: 'ignore' });
});
setTimeout(() => { console.error('timed out'); child?.kill(); process.exit(1); }, 60000);
