import test from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { equalizerActivationBlockReason } from "./equalizerAvailability.ts";

const ready = {
  shared: true,
  available: true,
  catalogLoading: false,
  runtimeAvailable: true,
  pulseAudio: false,
  pending: false,
};

test("EQ activation reports each blocking condition without promoting unknown state", () => {
  const cases = [
    [{ pending: true }, "operation_pending"],
    [{ shared: false }, "shared_route_required"],
    [{ available: null }, "availability_checking"],
    [{ available: false }, "engine_unavailable"],
    [{ catalogLoading: true }, "catalog_loading"],
    [{ pulseAudio: true }, "native_pulseaudio"],
    [{ runtimeAvailable: null }, "runtime_unvalidated"],
    [{ runtimeAvailable: false }, "runtime_unavailable"],
  ];
  for (const [change, expected] of cases) {
    assert.equal(equalizerActivationBlockReason({ ...ready, ...change }), expected);
  }
  assert.equal(equalizerActivationBlockReason(ready), null);
});

test("settings and full Equalizer use one block reason for activation controls", () => {
  const sources = [
    readFileSync(new URL("../components/EqualizerToggle.tsx", import.meta.url), "utf8"),
    readFileSync(new URL("../components/EqualizerView.tsx", import.meta.url), "utf8"),
  ];
  assert.match(sources[0], /equalizerActivationBlockReason\(/);
  assert.match(sources[0], /available = null/);
  assert.match(sources[0], /cannotEnable/);
  assert.match(sources[0], /nextEnabled && cannotEnable/);
  assert.match(sources[1], /equalizerActivationBlockReason\(/);
  assert.match(sources[1], /activationBlockReason!==null/);
  const view = sources[1];
  assert.match(view, /available=null/);
  assert.match(view, /disabled=\{pending\|\|refreshingAvailability\|\|\(!routed&&activationBlockReason!==null\)\}/);
  assert.match(view, /disabled=\{activationBlockReason!==null\|\|!budget\}/);
  assert.match(readFileSync(new URL("../components/SettingsModal.tsx", import.meta.url), "utf8"), /equalizerAvailable = null/);
});
