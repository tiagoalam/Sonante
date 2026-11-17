import test from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import ts from "typescript";

const source = readFileSync(new URL("./instrumentMotion.ts", import.meta.url), "utf8");
const js = ts.transpileModule(source, { compilerOptions: { module: ts.ModuleKind.CommonJS, target: ts.ScriptTarget.ES2020 } }).outputText;
const exported = {};
new Function("exports", js)(exported);
const { InstrumentMotion, levelDbfs, meterPosition } = exported;

const frame = (leftRms, rightRms, leftPeak, rightPeak, spectrum = []) => ({ leftRms, rightRms, leftPeak, rightPeak, spectrum });

test("meters keep left and right RMS and peak evidence independent", () => {
  const model = new InstrumentMotion();
  model.setTarget(frame(.25, .5, .4, .7, [.2, .6]), true);
  model.step(16.67);
  assert.equal(model.frame.rms.length, 2);
  assert.ok(model.frame.rms[0] > 0 && model.frame.rms[0] < .25);
  assert.ok(model.frame.rms[1] > model.frame.rms[0]);
  assert.equal(model.frame.peak[0], .4);
  assert.equal(model.frame.peak[1], .7);
  assert.ok(model.frame.spectrum[1] > model.frame.spectrum[0]);
  assert.equal(levelDbfs(0), -90);
  assert.equal(levelDbfs(1), 0);
  assert.equal(meterPosition(1), 1);
});

test("silence clears meter levels without synthetic activity and peak marks decay", () => {
  const model = new InstrumentMotion();
  model.setTarget(frame(.6, .4, .95, .7, Array(48).fill(.8)), true);
  model.step(16.67);
  const held = model.frame.peak[0];
  model.setTarget(frame(0, 0, 0, 0, []), false);
  for (let index = 0; index < 60; index++) model.step(16.67);
  assert.equal(model.frame.rms[0], 0);
  assert.equal(model.frame.rms[1], 0);
  assert.equal(model.frame.spectrum[0], 0);
  assert.equal(model.frame.spectrumPeaks[0], 0);
  assert.ok(model.frame.peak[0] < held);
});

test("meter peak marks hold briefly after a transient, then decay at a readable pace", () => {
  const model = new InstrumentMotion();
  model.setTarget(frame(.3, .2, .95, .6), true);
  model.step(16.67);
  const held = model.frame.peak[0];
  model.setTarget(frame(.2, .2, .2, .2), true);
  for (let index = 0; index < 30; index++) model.step(16);
  assert.equal(model.frame.peak[0], held, "the marker remains visible during its hold window");
  for (let index = 0; index < 40; index++) model.step(16);
  assert.ok(model.frame.peak[0] < held, "the marker releases after the hold window");
});
