// node --test tools/web-perf/scopes.test.mjs
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { summarizeScopes, scopesLine } from './scopes.mjs';

const scope = (name, dur, frame = 1, depth = 0) => ({ ph: 'X', name, ts: 0, dur, tid: 1, args: { frame, depth } });

test('a coarsened clock: short scopes read 0 and the rest come in whole steps', () => {
  const trace = {
    traceEvents: [
      { ph: 'M', name: 'thread_name', tid: 1, args: { name: 'main' } },
      // a frame on the frames track: no depth, so not a scope
      { ph: 'X', name: 'frame', ts: 0, dur: 16667, tid: 2, args: { ms: 16.7 } },
      scope('tick', 200, 1),
      scope('physics::step', 0, 1, 1),
      scope('physics::step', 0, 2, 1),
      scope('render', 100, 2),
    ],
    otherData: { reason: 'Save trace' },
  };
  const s = summarizeScopes(trace);
  assert.equal(s.scopes, 4);
  assert.equal(s.zero, 2);
  assert.equal(s.zeroShare, 0.5);
  assert.equal(s.smallestUs, 100);
  assert.deepEqual(s.smallest, [[100, 1], [200, 1]]);
  assert.equal(s.frames, 2);
  assert.equal(s.reason, 'Save trace');
  assert.deepEqual(s.names[0], { name: 'physics::step', count: 2, zero: 2, totalUs: 0, meanUs: 0, zeroShare: 1 });
  assert.match(scopesLine(s), /^4 scopes over 2 frames, 2 read 0 \(50%\), the shortest above 0 100 µs$/);
});

test('an empty trace says so', () => {
  const s = summarizeScopes({ traceEvents: [] });
  assert.equal(s.scopes, 0);
  assert.equal(s.smallestUs, null);
  assert.match(scopesLine(s), /none above 0/);
});
