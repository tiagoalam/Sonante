import test from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import ts from 'typescript';

function compile(file) {
  const exports = {};
  new Function('require', 'exports', ts.transpileModule(readFileSync(new URL(`../src/${file}`, import.meta.url), 'utf8'), {
    fileName: file,
    compilerOptions: { target: ts.ScriptTarget.ES2022, module: ts.ModuleKind.CommonJS },
  }).outputText)(id => {
    if (id === './albumAB') return { AB_RATE: 48_000 };
    assert.fail(`unexpected import ${id}`);
  }, exports);
  return exports;
}

const { estimateABSync } = compile('utils/albumABSync.ts');
const { alignABSamples, cropABSamples } = compile('utils/albumAB.ts');
const RATE = 48_000, HOP = RATE / 100, SECONDS = 9;
function random(seed) { let state = seed >>> 0; return () => ((state = (state * 1664525 + 1013904223) >>> 0) / 0x1_0000_0000); }
function makeEdition({ delay = 0, seed = 1, gain = 1, eq = false, noise = 0, fade = false, intro = false } = {}) {
  const frames = RATE * SECONDS, left = new Float32Array(frames), right = new Float32Array(frames);
  const envelope = Array.from({ length: SECONDS * 100 + 10 }, random(seed));
  const noiseL = random(seed + 91), noiseR = random(seed + 177);
  let lowL = 0, lowR = 0; const alpha = 1 - Math.exp(-2 * Math.PI * 900 / RATE);
  for (let i = 0; i < frames; i++) {
    const sourceTime = i / RATE - delay;
    if (sourceTime < 0) {
      if (intro) { left[i] = Math.sin(2 * Math.PI * 511 * i / RATE) * 0.2; right[i] = Math.sin(2 * Math.PI * 733 * i / RATE) * 0.18; }
      continue;
    }
    const hop = Math.floor(sourceTime * 100), env = 0.18 + envelope[hop] * 0.82;
    const time = sourceTime;
    const lo = Math.sin(2 * Math.PI * 220 * time) * env;
    const hi = Math.sin(2 * Math.PI * 3100 * time + 0.2) * env * 0.42;
    let l = (lo + hi) * gain, r = (Math.sin(2 * Math.PI * 223 * time) * env + Math.sin(2 * Math.PI * 3170 * time + 0.4) * env * 0.38) * gain;
    if (eq) { lowL += alpha * (l - lowL); lowR += alpha * (r - lowR); l = lowL + (l - lowL) * 0.45; r = lowR + (r - lowR) * 0.5; }
    if (fade) { const f = Math.min(1, sourceTime / 0.45); l *= f; r *= f; }
    l += (noiseL() - 0.5) * noise; r += (noiseR() - 0.5) * noise;
    left[i] = l; right[i] = r;
  }
  return { left, right, frames, sourceRate: RATE, sourceChannels: 2, start: 0 };
}

test('content correlation estimates known 100 ms to 2 s delays to within 20 ms despite leading silence, gain and moderate EQ', () => {
  for (const delay of [0.1, 0.5, 1, 2]) {
    const result = estimateABSync(makeEdition(), makeEdition({ delay, gain: 0.37, eq: true, noise: 0.0002, fade: true }));
    assert.ok(result, `expected a confident match at ${delay}s`);
    assert.ok(Math.abs(result.offsetFrames / RATE - delay) <= 0.02, `${delay}s estimated as ${result.offsetFrames / RATE}s`);
    assert.ok(result.confidence >= 0.62);
  }
});

test('negative relative offset is signed consistently and manual start remains available as an independent adjustment', () => {
  const delayedA = makeEdition({ delay: 0.5 }), plainB = makeEdition({ seed: 1 });
  const result = estimateABSync(delayedA, plainB);
  assert.ok(result);
  assert.ok(Math.abs(result.offsetFrames / RATE + 0.5) <= 0.02);
  const aligned = alignABSamples(delayedA, plainB, RATE * 5, result.offsetFrames);
  assert.ok(aligned); assert.equal(aligned[0].start, 0.5); assert.equal(aligned[1].start, 0);
});

test('a short, different musical introduction is not mistaken for silence or used as the alignment point', () => {
  const result = estimateABSync(makeEdition(), makeEdition({ delay: 0.5, intro: true, gain: 0.6 }));
  assert.ok(result);
  assert.ok(Math.abs(result.offsetFrames / RATE - 0.5) <= 0.02);
});

test('unrelated content, flat tones, and near-silence do not cause automatic trimming', () => {
  const unrelated = estimateABSync(makeEdition({ seed: 1 }), makeEdition({ seed: 700 }));
  assert.equal(unrelated, null);
  const frames = RATE * SECONDS;
  const flat = { left: new Float32Array(frames).fill(0.1), right: new Float32Array(frames).fill(0.1), frames };
  assert.equal(estimateABSync(flat, flat), null);
  const silence = { left: new Float32Array(frames), right: new Float32Array(frames), frames };
  assert.equal(estimateABSync(silence, silence), null);
});

test('automatic offset crops a shared-duration timeline without cumulative shifts', () => {
  const a = makeEdition(), b = makeEdition({ delay: 0.5 });
  const result = estimateABSync(a, b); assert.ok(result);
  const first = alignABSamples(a, b, RATE * 5, result.offsetFrames); assert.ok(first);
  assert.equal(first[0].frames, RATE * 5); assert.equal(first[1].frames, RATE * 5);
  assert.ok(Math.abs((first[1].start - first[0].start) - result.offsetFrames / RATE) <= 0.02);
  const second = alignABSamples(a, b, RATE * 5, result.offsetFrames); assert.ok(second);
  assert.equal(second[0].start, first[0].start); assert.equal(second[1].start, first[1].start);
  assert.equal(cropABSamples(a, b, RATE * 5)[0].start, 0);
});

test('ambiguous or out-of-context offsets keep the manual, unshifted crop', () => {
  const sample = makeEdition();
  assert.ok(alignABSamples(sample, sample, RATE * 5, RATE * 3 + 1) === null, 'offset outside supported context');
  const shortB = { ...sample, left: sample.left.slice(0, RATE * 5), right: sample.right.slice(0, RATE * 5), frames: RATE * 5 };
  assert.ok(alignABSamples(sample, shortB, RATE * 5, RATE / 2) === null, 'B lacks tail context for positive offset');
  const fallback = cropABSamples(sample, sample, RATE * 5); assert.ok(fallback);
  assert.equal(fallback[0].start, 0); assert.equal(fallback[1].start, 0);
});
