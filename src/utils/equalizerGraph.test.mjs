import test from "node:test";
import assert from "node:assert/strict";
import { flatPreset, responseDb } from "./equalizer.ts";
import {
  EQ_GRAPH_MAX_RANGE_DB,
  EQ_GRAPH_MIN_RANGE_DB,
  equalizerGraphFrequencies,
  equalizerGraphFrequencyLabels,
  equalizerGraphGain,
  equalizerGraphPath,
  equalizerGraphRange,
  equalizerGraphTicks,
  equalizerGraphY,
  sampleEqualizerGraph,
} from "./equalizerGraph.ts";

test("graph sampling follows the logarithmic frequency axis and uses the actual filter response", () => {
  const frequencies = equalizerGraphFrequencies();
  assert.equal(frequencies.length, 481);
  assert.equal(frequencies[0], 20);
  assert.equal(frequencies.at(-1), 20_000);
  const ratios = frequencies.slice(1).map((frequency, index) => frequency / frequencies[index]);
  assert.ok(ratios.every(ratio => Math.abs(ratio - ratios[0]) < 1e-12));

  const preset = flatPreset();
  preset.bands[0] = { ...preset.bands[0], enabled: true, filter_type: "peak", frequency_hz: 997, gain_db: 3, q: 1 };
  const curves = sampleEqualizerGraph(preset, -23, false);
  const atCenter = curves.frequencies.reduce((best, frequency, index) => Math.abs(frequency - 997) < Math.abs(curves.frequencies[best] - 997) ? index : best, 0);
  const sampledFrequency = curves.frequencies[atCenter];
  assert.ok(Math.abs(curves.individual[0][atCenter] - 3) < 0.1);
  assert.equal(curves.individual[0][atCenter], responseDb({ ...preset, preamp_db: 0, bands: [preset.bands[0]] }, sampledFrequency));
  assert.equal(curves.combined, null, "the protected curve is not sampled while its display is off");
  assert.ok(curves.requested.every(Number.isFinite));
  assert.ok(curves.individual[0].every(Number.isFinite));
  const withProtected = sampleEqualizerGraph(preset, -23, true);
  assert.ok(withProtected.combined.every(Number.isFinite));
  assert.equal(withProtected.combined[atCenter], responseDb({ ...preset, preamp_db: -23 }, sampledFrequency));
});

test("adaptive dB ranges fit ordinary curves and remain within readable bounds", () => {
  assert.equal(equalizerGraphRange([]), EQ_GRAPH_MIN_RANGE_DB);
  assert.equal(equalizerGraphRange([[3, Number.NaN, Number.POSITIVE_INFINITY]]), EQ_GRAPH_MIN_RANGE_DB);
  assert.equal(equalizerGraphRange([[6]]), 9, "leaves margin around a 6 dB curve");
  assert.equal(equalizerGraphRange([[12]]), 15);
  assert.equal(equalizerGraphRange([[1_000]]), EQ_GRAPH_MAX_RANGE_DB);
  assert.equal(equalizerGraphRange([[1_000_000]]), EQ_GRAPH_MAX_RANGE_DB);

  const extreme = flatPreset();
  extreme.bands = extreme.bands.map(band => ({ ...band, enabled: true, filter_type: "low_shelf", frequency_hz: 20_000, gain_db: 12, q: 30 }));
  const sampled = sampleEqualizerGraph(extreme, -72, true);
  const scale = equalizerGraphRange([...sampled.individual, sampled.requested, sampled.combined]);
  assert.equal(scale, 480, "valid high-Q shelves fit the bounded scale without changing their response");
  assert.ok(Math.max(...sampled.requested.map(Math.abs)) < scale);
});

test("graph labels use clean dB increments and reduce frequency text on narrow plots", () => {
  assert.deepEqual(equalizerGraphTicks(4), [-4, -2.7, -1.3, 0, 1.3, 2.7, 4]);
  assert.deepEqual(equalizerGraphTicks(9), [-9, -6, -3, 0, 3, 6, 9]);
  assert.deepEqual(equalizerGraphTicks(Number.NaN), [-4, -2.7, -1.3, 0, 1.3, 2.7, 4]);
  assert.deepEqual(equalizerGraphFrequencyLabels(224), [20, 100, 500, 2_000, 5_000, 20_000]);
  assert.equal(equalizerGraphFrequencyLabels(504).length, 10, "wide plots keep each log-frequency label");
});

test("dB pointer transforms share bounded plotting coordinates and curves skip non-finite samples", () => {
  const top = 28, height = 268, range = 15;
  assert.equal(equalizerGraphY(0, top, height, range), top + height / 2);
  assert.equal(equalizerGraphY(15, top, height, range), top);
  assert.ok(Math.abs(equalizerGraphGain(equalizerGraphY(12, top, height, range), top, height, range) - 12) < 1e-12);
  assert.equal(equalizerGraphGain(-10_000, top, height, range), 15, "dragging beyond the plot retains the ±15 dB filter gain limit");
  assert.equal(equalizerGraphGain(0, top, 0, range), 0);
  assert.equal(equalizerGraphY(1_000, top, height, Number.NaN), top + height / 2 - height * 1_000 / (2 * EQ_GRAPH_MIN_RANGE_DB));
  assert.equal(equalizerGraphPath([0, Number.NaN, 6], 48, 100, 20, 100, 6), "M48,70 M148,20");
});
