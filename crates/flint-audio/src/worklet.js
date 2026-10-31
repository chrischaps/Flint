// Flint's mixer in an AudioWorklet (#136): the page's half, and the
// processor's source. `crate::worklet` (worklet.rs) drives it.
//
// The processor runs the second wasm, crates/flint-audio-worklet: kira and
// nothing else, with no imports. The page fetches its bytes before Begin,
// hands them to the processor, which compiles them on the audio thread, and
// from then on posts it each sound once (a Float32Array, transferred) and
// one batch of commands a frame (bytes, transferred). Messages posted while
// the processor is still starting wait here, in order.
//
// The processor meters itself in buffers of `meterFrames` frames: the time
// spent mixing (Date.now, the only clock a worklet has; whole milliseconds
// summed over 128-frame renders, so a light mix of a few microseconds a
// render reads as about 0), its loudest sample, the wall time
// between buffers, and how far the audio clock has fallen behind the wall
// clock since its steadiest. A dropout is a step in that lag that doesn't
// come back. Each buffer's numbers are posted to the page, whose
// BufferMeter (meter.rs) counts and logs them, as it did for the
// main-thread stream.

const PROCESSOR = `
const CREEP = 0.002; // how fast the steadiest lag follows the clocks' drift
class FlintMixer extends AudioWorkletProcessor {
  constructor(options) {
    super();
    const { wasm, meterFrames, stallMs } = options.processorOptions;
    this.x = null;
    try {
      const x = new WebAssembly.Instance(new WebAssembly.Module(wasm), {}).exports;
      if (!x.flint_init(sampleRate)) throw new Error('kira could not start');
      this.x = x;
    } catch (e) {
      this.port.postMessage({ t: 'error', msg: String(e) });
      return;
    }
    this.port.onmessage = (e) => this.message(e.data);
    this.meterFrames = meterFrames;
    this.bufferMs = (meterFrames * 1000) / sampleRate;
    this.frames = 0;
    this.mix = 0;
    this.start = 0;
    this.lag = 0;
    this.lastStart = 0;
    this.baseline = null;
    this.peak = 0;
    this.stallMs = stallMs;
    this.rendered = 0;
    this.port.postMessage({ t: 'ready', rate: sampleRate });
  }
  copyIn(bytes) {
    const ptr = this.x.flint_scratch(bytes.byteLength);
    new Uint8Array(this.x.memory.buffer, ptr, bytes.byteLength).set(bytes);
  }
  message(m) {
    if (!this.x) return;
    if (m.t === 'batch') {
      this.copyIn(m.bytes);
      this.x.flint_apply(m.bytes.byteLength);
    } else if (m.t === 'load') {
      const s = m.samples;
      this.copyIn(new Uint8Array(s.buffer, s.byteOffset, s.byteLength));
      this.x.flint_load(m.id, m.rate, m.channels, s.length);
    } else if (m.t === 'sleep') {
      // The page is hidden and its context about to be suspended (#162):
      // the clock's stop is a sleep, not sound lost.
      this.slept = true;
    }
  }
  process(inputs, outputs) {
    const out = outputs[0];
    if (!this.x || !out || out.length < 2) return true;
    const n = out[0].length;
    const t0 = Date.now();
    if (this.slept) {
      this.slept = false;
      this.frames = 0;
      this.baseline = null;
      this.lastStart = 0;
    }
    if (this.frames === 0) {
      this.start = t0;
      this.lag = t0 - currentTime * 1000;
      this.mix = 0;
    }
    const ptr = this.x.flint_render(n) >> 2;
    if (this.mem !== this.x.memory.buffer) {
      this.mem = this.x.memory.buffer;
      this.f32 = new Float32Array(this.mem);
    }
    const f = this.f32, l = out[0], r = out[1];
    let peak = this.peak;
    for (let i = 0; i < n; i++) {
      const a = (l[i] = f[ptr + i]), b = (r[i] = f[ptr + n + i]);
      if (a > peak || -a > peak) peak = Math.abs(a);
      if (b > peak || -b > peak) peak = Math.abs(b);
    }
    this.peak = peak;
    this.mix += Date.now() - t0;
    this.frames += n;
    // ?audiostall=N: miss the deadline on purpose every 2 s, to see the
    // meter catch it.
    this.rendered += n;
    if (this.stallMs && this.rendered >= 2 * sampleRate) {
      this.rendered = 0;
      const until = Date.now() + this.stallMs;
      while (Date.now() < until);
    }
    if (this.frames >= this.meterFrames) this.meter();
    return true;
  }
  meter() {
    if (this.baseline === null || this.lag < this.baseline) this.baseline = this.lag;
    const slack = this.bufferMs - (this.lag - this.baseline);
    // A loss is counted once; after it the clock is steady at its new lag.
    if (slack < 0) this.baseline = this.lag;
    else this.baseline += (this.lag - this.baseline) * CREEP;
    const wait = this.lastStart ? this.start - this.lastStart : 0;
    this.lastStart = this.start;
    this.port.postMessage({
      t: 'stats',
      v: new Float64Array([slack, this.mix, this.start, wait, this.x.flint_errors(), this.bufferMs, this.peak]),
    });
    this.frames = 0;
    this.peak = 0;
  }
}
registerProcessor('flint-mixer', FlintMixer);
`;

/** How long the processor may take to say it is ready. */
const READY_MS = 4000;

let wasmBytes = null;

/** Fetch the worklet's wasm (once), so Begin doesn't wait for it. */
export function flint_worklet_preload(url) {
  if (wasmBytes || typeof fetch !== 'function') return;
  const href = new URL(url, document.baseURI).href;
  wasmBytes = fetch(href).then((r) => {
    if (!r.ok) throw new Error(`${href}: HTTP ${r.status}`);
    return r.arrayBuffer();
  });
  wasmBytes.catch(() => {});
}

/** Whether this browser can run the worklet, and the page didn't ask for
 *  the main thread with ?mixer=main. */
export function flint_worklet_wanted() {
  const asked = new URLSearchParams(location.search).get('mixer');
  return asked !== 'main' && typeof AudioWorkletNode === 'function';
}

// iOS and iPadOS WebKit (every browser there, Chrome included) start an
// AudioContext only from inside a gesture's own handler. Chrome elsewhere
// counts any gesture the page has had, so a context opened on the frame
// after Begin plays on a desktop and stays suspended on an iPad, silently
// (Float#2). So Begin's click opens the context here, before it returns,
// and the worklet takes it a frame later. Every context Flint plays
// through is also resumed by any later tap or key: the main-thread
// fallback's, which cpal opens outside the gesture, and one iOS
// interrupted (a call, the app switcher, the screen locking).

/** The context Begin's gesture opened, until the worklet takes it. */
let unlocked = null;
/** Every context Flint plays through, for a later gesture to resume. */
const owned = new Set();

function wake() {
  for (const ctx of owned) {
    if (ctx.state !== 'running' && ctx.state !== 'closed') ctx.resume().catch(() => {});
  }
}

/** Every context Flint plays through, for the page to suspend while it is
 *  hidden (#162, page.rs). */
export function flint_audio_contexts() {
  return Array.from(owned);
}

/** Every worklet started, to tell before their context is suspended. */
const worklets = new Set();

/** The page is going to sleep (#162): each processor's meter is to take
 *  the gap that follows for a pause, not a dropout. */
export function flint_audio_sleep() {
  for (const w of worklets) w.post({ t: 'sleep' });
}

let listening = false;

/** Resume `ctx` whenever the page gets a gesture while it isn't running. */
export function flint_audio_keep_awake(ctx) {
  owned.add(ctx);
  ctx.addEventListener('statechange', () => {
    if (ctx.state === 'closed') owned.delete(ctx);
  });
  if (listening) return;
  listening = true;
  // touchend, not only click: winit cancels the click a tap on the canvas
  // would make, and WebKit counts touchend as the gesture.
  for (const type of ['touchend', 'pointerup', 'click', 'keydown']) {
    document.addEventListener(type, wake, { capture: true, passive: true });
  }
  document.addEventListener('visibilitychange', () => {
    if (document.visibilityState === 'visible') wake();
  });
}

/** Call inside Begin's gesture: open (or resume) the page's context and
 *  play one silent frame through it, which is what WebKit waits for. */
export function flint_audio_unlock(latency) {
  // Let sound play with the ringer silenced, as a video would: WebKit's
  // default "auto" session mutes Web Audio with it (iOS 17, iPadOS 17).
  try {
    const session = navigator.audioSession;
    if (session && session.type !== 'playback') session.type = 'playback';
  } catch (_) {}
  if (!unlocked && typeof AudioContext === 'function') {
    try {
      unlocked = new AudioContext({ latencyHint: latency });
    } catch (e) {
      console.warn(`[flint-audio] no AudioContext at Begin: ${e}`);
      return;
    }
    watch(unlocked);
    flint_audio_keep_awake(unlocked);
  }
  if (!unlocked) return;
  unlocked.resume().catch(() => {});
  const blip = unlocked.createBufferSource();
  blip.buffer = unlocked.createBuffer(1, 1, unlocked.sampleRate);
  blip.connect(unlocked.destination);
  blip.start(0);
  console.log(`[flint-audio] Begin opened the AudioContext (${unlocked.state})`);
}

/** The context Begin opened, or a new one when there wasn't a Begin. */
function takeContext(latency) {
  const ctx = unlocked;
  unlocked = null;
  if (ctx && ctx.state !== 'closed') return ctx;
  const fresh = new AudioContext({ latencyHint: latency });
  watch(fresh);
  flint_audio_keep_awake(fresh);
  return fresh;
}

/** The main thread mixes after all, on a context cpal opens: close the
 *  one Begin opened for the worklet, if nobody took it. */
export function flint_audio_release_unlocked() {
  if (unlocked) unlocked.close().catch(() => {});
  unlocked = null;
}

/** Resolves once `ctx` runs. A suspended context renders nothing, so the
 *  processor can't say it's ready until then. */
function running(ctx) {
  if (ctx.state === 'running') return Promise.resolve();
  console.warn(`[flint-audio] AudioContext ${ctx.state}: sound starts at the next tap or key`);
  return new Promise((resolve) => {
    const check = () => {
      if (ctx.state !== 'running') return;
      ctx.removeEventListener('statechange', check);
      resolve();
    };
    ctx.addEventListener('statechange', check);
  });
}

function watch(ctx) {
  ctx.onstatechange = () => {
    const line = `[flint-audio] AudioContext ${ctx.state} at ${ctx.currentTime.toFixed(1)} s`;
    if (ctx.state === 'running') console.log(line);
    else console.warn(line);
  };
  ctx.addEventListener('error', () => {
    console.warn(
      `[flint-audio] the audio device or WebAudio renderer failed at ` +
        `${ctx.currentTime.toFixed(1)} s (${ctx.state}); trying to resume`,
    );
    ctx.resume().catch(() => {});
  });
}

export class FlintWorklet {
  /** Take the AudioContext Begin's gesture opened (or open one now) and
   *  start the processor as soon as the wasm is in and the context runs. */
  constructor(url, meterFrames, latency, onStats) {
    this.state = 0; // 0 starting, 1 running, 2 failed
    this.queue = [];
    this.node = null;
    this.ctx = takeContext(latency);
    this.ctx.resume().catch(() => {});
    worklets.add(this);
    flint_worklet_preload(url);
    this.open(meterFrames, onStats).catch((e) => {
      this.state = 2;
      this.queue = [];
      console.warn(`[flint-audio] AudioWorklet failed (${e}); mixing on the main thread instead`);
      this.ctx.close().catch(() => {});
    });
  }

  async open(meterFrames, onStats) {
    const wasm = await wasmBytes;
    const src = URL.createObjectURL(new Blob([PROCESSOR], { type: 'text/javascript' }));
    try {
      await this.ctx.audioWorklet.addModule(src);
    } finally {
      URL.revokeObjectURL(src);
    }
    const node = new AudioWorkletNode(this.ctx, 'flint-mixer', {
      numberOfInputs: 0,
      numberOfOutputs: 1,
      outputChannelCount: [2],
      processorOptions: {
        wasm,
        meterFrames,
        stallMs: Number(new URLSearchParams(location.search).get('audiostall')) || 0,
      },
    });
    await running(this.ctx);
    await new Promise((resolve, reject) => {
      const timer = setTimeout(() => reject(new Error('the processor never said ready')), READY_MS);
      node.port.onmessage = (e) => {
        clearTimeout(timer);
        if (e.data.t === 'ready') resolve();
        else reject(new Error(e.data.msg));
      };
      node.onprocessorerror = () => {
        clearTimeout(timer);
        reject(new Error('the processor threw'));
      };
    });
    node.port.onmessage = (e) => {
      if (e.data.t === 'stats') onStats(e.data.v);
    };
    node.onprocessorerror = () =>
      console.warn('[flint-audio] the AudioWorklet processor threw; sound has stopped');
    node.connect(this.ctx.destination);
    this.node = node;
    for (const [msg, transfer] of this.queue) node.port.postMessage(msg, transfer);
    this.queue = [];
    this.state = 1;
    const latency = ((this.ctx.baseLatency || 0) + (this.ctx.outputLatency || 0)) * 1000;
    console.log(
      `[flint-audio] AudioWorklet open: ${this.ctx.sampleRate} Hz, mixing off the main ` +
        `thread (${(wasm.byteLength / 1024) | 0} KB wasm, ${latency.toFixed(0)} ms output latency)`,
    );
  }

  post(msg, transfer) {
    if (this.node) this.node.port.postMessage(msg, transfer);
    else if (this.state === 0) this.queue.push([msg, transfer]);
  }

  load(id, rate, channels, samples) {
    this.post({ t: 'load', id, rate, channels, samples }, [samples.buffer]);
  }

  batch(bytes) {
    this.post({ t: 'batch', bytes }, [bytes.buffer]);
  }

  /** Run `f` once the current task (the frame) is done. */
  kick(f) {
    queueMicrotask(f);
  }
}
