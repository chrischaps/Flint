// node --test tools/web-perf/perfline.test.mjs   (the perf line's parser against the format
// `summary_line` writes; see perf.rs's the_copied_line_is_one_line_with_every_part)
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { parsePerfLine, range } from './perfline.mjs';

const LINE = [
  'flint perf · float · t=812s',
  'Chrome 156 · NVIDIA GeForce RTX 4070 Laptop GPU',
  '2560x1440 DPR 2',
  'scene 1920x1080 scale 0.75',
  'frame p50 16.7 p90 16.7 p99 17.0 max 18.2 ms (600 frames/10s)',
  'pace 30 fps, 1 in 4 of 120 Hz (auto)',
  'cpu 4.00 ms: update 1.00/1.0, render 3.00/3.0',
  'worst scripts: hud 2.00/2.0',
  'gpu 3.95 ms: Scene 3.10, Style mask 0.02, ASCII 0.11, SSAO 0.72',
  '212 draws 1.2M tris',
  'webgpu/frame submit 1.0 pass 14.0 bindgroup 2.0 write 12.5 (4.0 KB) buffer 0.0',
  'audio 3/1200 late mix 2.50 ms',
].join(' | ');

test('every part of the line is read', () => {
  const p = parsePerfLine(LINE);
  assert.equal(p.game, 'float');
  assert.equal(p.t, 812);
  assert.equal(p.browser, 'Chrome 156');
  assert.equal(p.adapter, 'NVIDIA GeForce RTX 4070 Laptop GPU');
  assert.deepEqual(p.canvas, [2560, 1440]);
  assert.equal(p.dpr, 2);
  assert.deepEqual(p.scene, [1920, 1080]);
  assert.equal(p.scale, 0.75);
  assert.deepEqual(p.frame, { p50: 16.7, p90: 16.7, p99: 17, max: 18.2, frames: 600, secs: 10 });
  assert.equal(p.cpu, 4);
  assert.equal(p.gpu.frame, 3.95);
  assert.deepEqual(p.gpu.passes, { Scene: 3.1, 'Style mask': 0.02, ASCII: 0.11, SSAO: 0.72 });
  assert.equal(p.draws, 212);
  assert.equal(p.tris, '1.2M');
  assert.deepEqual(p.webgpu, { submits: 1, passes: 14, bindGroups: 2, writes: 12.5, writeKB: 4, buffers: 0 });
});

// copied from a page on 2026-11-12 (Chrome 157, ascii_entities at DPR 1)
const REAL =
  'flint perf · ascii_entities · t=25s | Chrome 157 · nvidia lovelace | 1920x1080 DPR 1 | scene 1920x1080 scale 1 | ' +
  'frame p50 16.7 p90 16.7 p99 16.8 max 16.8 ms (599 frames/10s) | cpu 0.95 ms: input 0.04/0.2, scripts 0.03/0.2, ' +
  'physics 0.02/0.1, script ui 0.00/0.1, anim/fx 0.06/0.2, sync 0.14/0.3, render 0.52/0.9, hud 0.14/0.6, present 0.00/0.1 | ' +
  'worst scripts: [hot reload] 0.00/0.1, [load, on_init] 0.00/0.1, [events] 0.00/0.1, [expression] 0.00/0.1 | ' +
  'gpu 4.91 ms: Grass 0.05, Shadows 0.98, Scene 1.62, Style mask 0.04, SSAO 1.68, Bloom 0.09, Composite 0.05, ASCII 0.41, UI 0.00 | ' +
  '13 draws 663.1K tris | webgpu/frame submit 1.0 pass 26.0 bindgroup 0.0 write 23.0 (34.6 KB) buffer 0.0 | ' +
  'audio 0/579 late mix 0.37 ms | voices 0/0 tracks 0/0 one-shots 0 unheard 0 stolen';

test('a line a page wrote is read', () => {
  const p = parsePerfLine(REAL);
  assert.equal(p.adapter, 'nvidia lovelace');
  assert.deepEqual(p.canvas, [1920, 1080]);
  assert.equal(p.gpu.passes['Style mask'], 0.04);
  assert.equal(p.gpu.passes.ASCII, 0.41);
  assert.equal(Object.keys(p.gpu.passes).length, 9);
  assert.equal(p.webgpu.passes, 26);
  assert.equal(p.webgpu.bindGroups, 0);
  assert.equal(p.tris, '663.1K');
});

test('the GPU part says how it was timed (#380)', () => {
  const idle = parsePerfLine(
    'present fifo | gpu 4.91 ms under vsync at idle clocks (31% busy, reads high; FLINT_VSYNC=0 for pass costs): Scene 1.62, Style mask 0.04',
  );
  assert.equal(idle.present, 'fifo');
  assert.deepEqual(idle.gpu, { frame: 4.91, passes: { Scene: 1.62, 'Style mask': 0.04 }, vsync: true, idleClocks: 0.31 });
  assert.deepEqual(parsePerfLine('gpu 15.00 ms no vsync: Scene 15.00').gpu, { frame: 15, passes: { Scene: 15 }, vsync: false });
  assert.deepEqual(parsePerfLine('gpu 3.90 ms under vsync: SSAO 0.80').gpu, { frame: 3.9, passes: { SSAO: 0.8 }, vsync: true });
});

test('a line without GPU timing says so', () => {
  assert.deepEqual(parsePerfLine('flint perf · x · t=3s | gpu off').gpu, { frame: null, passes: {}, note: 'gpu off' });
  assert.deepEqual(parsePerfLine(undefined), {});
});

test('a range is one number when the reads agree', () => {
  assert.equal(range([0.09, 0.12, 0.1]), '0.09--0.12');
  assert.equal(range([0.02, 0.021]), '0.02');
  assert.equal(range([undefined, 3], 1), '3.0');
  assert.equal(range([]), '');
});
