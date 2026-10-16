import test from "node:test";
import assert from "node:assert/strict";
import { sharePendingRequest } from "./sharePendingRequest.ts";

test("navigation consumers share a stuck request until physical settlement, then reload fresh data", async () => {
  let resolve, calls = 0;
  const first = new Promise(done => { resolve = done; });
  const load = sharePendingRequest(() => { calls++; return calls === 1 ? first : Promise.resolve(["fresh"]); });
  const one = load();
  for (let i = 0; i < 1000; i++) assert.equal(load(), one);
  for (let i = 0; i < 50; i++) await Promise.resolve();
  assert.equal(calls, 1);
  const albums = ["preserved"];
  resolve(albums); assert.equal(await one, albums);
  assert.deepEqual(await load(), ["fresh"]); assert.equal(calls, 2);
});

test("shared failure and synchronous throws preserve identity and allow later recovery without automatic retries", async () => {
  const error = new Error("controlled failure"); let calls = 0;
  const load = sharePendingRequest(() => { calls++; if (calls === 1) throw error; return Promise.resolve(["recovered"]); });
  const one = load(), two = load(); assert.equal(one, two);
  await assert.rejects(one, e => e === error); await assert.rejects(two, e => e === error);
  assert.equal(calls, 1); assert.deepEqual(await load(), ["recovered"]);
});
