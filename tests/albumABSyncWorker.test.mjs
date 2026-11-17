import test from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import ts from 'typescript';

function fixture() {
  const workers = [];
  class WorkerMock {
    constructor() { workers.push(this); this.terminated = false; }
    postMessage(message, transfers) { this.message = message; this.transfers = transfers; }
    terminate() { this.terminated = true; }
  }
  const exports = {};
  const output = ts.transpileModule(readFileSync(new URL('../src/services/albumABSync.ts', import.meta.url), 'utf8'), {
    fileName: 'services/albumABSync.ts',
    compilerOptions: { target: ts.ScriptTarget.ES2022, module: ts.ModuleKind.CommonJS },
  }).outputText;
  new Function('require', 'exports', 'Worker', output)(id => {
    if (id === '../workers/albumABSync.worker?worker') return WorkerMock;
    if (id === '../utils/albumAB') return { AB_RATE: 48_000 };
    assert.fail(`unexpected import ${id}`);
  }, exports, WorkerMock);
  return { startABSync: exports.startABSync, workers };
}
const sample = (seconds = 9) => ({ frames: seconds * 48_000, left: new Float32Array(seconds * 48_000), right: new Float32Array(seconds * 48_000), start: 0, sourceRate: 48_000, sourceChannels: 2 });

test('worker receives bounded transferable PCM and a valid result settles and terminates it', async () => {
  const f = fixture(), request = f.startABSync(sample(30), sample(30));
  const worker = f.workers[0]; assert.equal(worker.message.a.frames, 9 * 48_000);
  assert.equal(worker.message.b.frames, 9 * 48_000); assert.equal(worker.transfers.length, 4);
  const result = { offsetFrames: 4_800, confidence: 0.8, windowSeconds: 3 };
  worker.onmessage({ data: result });
  assert.deepEqual(await request.promise, result); assert.equal(worker.terminated, true);
});

test('worker crash, malformed result, message deserialization failure, and cancel all fail closed to manual alignment', async () => {
  for (const end of ['error', 'invalid', 'messageerror', 'cancel']) {
    const f = fixture(), request = f.startABSync(sample(), sample()), worker = f.workers[0];
    if (end === 'error') worker.onerror({ preventDefault() {} });
    else if (end === 'invalid') worker.onmessage({ data: { offsetFrames: 999_999, confidence: 0.99, windowSeconds: 3 } });
    else if (end === 'messageerror') worker.onmessageerror(new Event('messageerror'));
    else request.cancel();
    assert.equal(await request.promise, null, end);
    assert.equal(worker.terminated, true, end);
  }
});
