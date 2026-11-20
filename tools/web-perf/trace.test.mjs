// node --test tools/web-perf/trace.test.mjs   (run.mjs --trace's summary, #962)
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { summarizeTrace, traceLine } from './trace.mjs';

// frame times from gaps, as the page's recorder keeps them (ms since start)
const framesOf = (gaps) => gaps.reduce((f, g) => [...f, Math.round((f.at(-1) + g) * 10) / 10], [0]);
const vsync = (n) => Array(n).fill(16.7);

test('an even 60 Hz trace has no slow frames', () => {
  const t = summarizeTrace({ frames: framesOf(vsync(600)) });
  assert.equal(t.frames, 601);
  assert.equal(t.vsync, 16.7);
  assert.equal(t.slow, 0);
  assert.deepEqual(t.spells, []);
  assert.equal(t.seconds, 10);
  assert.equal(t.fps.length, 10);
  assert.ok(t.fps.every((n) => n === 60 || n === 59 || n === 61), t.fps.join(' '));
});

test('frames a second show a half-rate start as 30s, then 60s', () => {
  const t = summarizeTrace({ frames: framesOf([...Array(90).fill(33.3), ...vsync(180)]) });
  assert.deepEqual(t.fps.slice(0, 2), [31, 30]);
  assert.ok(t.fps.slice(3).every((n) => n >= 59), t.fps.join(' '));
  assert.equal(t.slowSeconds, 3);
  assert.match(traceLine(t), /3 of \d+ s at 45 fps or less$/);
});

test('a stretch at half rate is one spell, however its on-time frames fall', () => {
  // 2 s at 60, then about 1 s at 30 with an on-time frame now and then, then 60
  const half = [33.3, 33.4, 16.7, 33.3, 33.3, 33.4, 16.7, 33.3, 33.3, 33.4, 33.3, 16.7, 33.3, 33.3];
  const t = summarizeTrace({ frames: framesOf([...vsync(120), ...half, ...vsync(120)]) });
  assert.equal(t.slow, 11);
  assert.equal(t.spells.length, 1);
  const [s] = t.spells;
  assert.equal(s.at, 2);
  assert.equal(s.slow, 11);
  assert.equal(s.worst, 33.4);
  assert.equal(s.lasts, 0.4);
  assert.match(traceLine(t), /11 slow in 1 spells \(longest 0.4 s\)/);
});

test('a page at half rate most of its run still has a 60 Hz vsync', () => {
  // 3 s at 60, then 40 s at 30, as Firefox's WebGPU clear did in one run
  const t = summarizeTrace({ frames: framesOf([...vsync(180), ...Array(1200).fill(33.3)]) });
  assert.equal(t.vsync, 16.7);
  assert.equal(t.slow, 1200);
  assert.equal(t.slowSeconds, 39); // second 3 holds both rates
});

test('slow frames far apart are spells of their own', () => {
  const t = summarizeTrace({ frames: framesOf([...vsync(60), 50, ...vsync(60), 33.4, ...vsync(60)]) });
  assert.equal(t.spells.length, 2);
  assert.deepEqual(t.spells.map((s) => s.worst), [50, 33.4]);
  assert.equal(t.max, 50);
});

test('a trace too short to have an interval says so and does not throw', () => {
  const t = summarizeTrace({ frames: [0] });
  assert.equal(t.slow, 0);
  assert.deepEqual(t.spells, []);
});
