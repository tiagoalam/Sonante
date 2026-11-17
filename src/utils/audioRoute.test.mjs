import test from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { interpretLegacyAudioRoute, requestedRouteSnapshot, transitioningRouteSnapshot,
  confirmedRouteSnapshot, uncertainRouteSnapshot, validateAudioRoute, validateAudioRouteSnapshot,
  acceptsAudioRouteGeneration } from "./audioRoute.ts";

const fixture = JSON.parse(readFileSync(new URL("../../tests/fixtures/audio-route-contract.json", import.meta.url), "utf8"));
const projectionFixture = JSON.parse(readFileSync(new URL("../../tests/fixtures/audio-route-projection.json", import.meta.url), "utf8"));

test("frontend legacy route semantics match the shared Rust fixture", () => {
  for (const row of fixture.cases) {
    const interpretation = interpretLegacyAudioRoute(row.outputType, row.device);
    assert.deepEqual(interpretation, row.expected, row.name);
    assert.equal(validateAudioRoute(interpretation.route) === null,
      !interpretation.issues.includes("invalid_device_identifier"), `${row.name} validity`);
  }
});

test("saved configuration is a request, not evidence of a confirmed or active route", () => {
  const savedConfig = { audio_output_type: "pipewire", alsa_device: "default" };
  const interpreted = interpretLegacyAudioRoute(savedConfig.audio_output_type, savedConfig.alsa_device);
  assert.equal(interpreted.route.processing, "off");
  const snapshot = requestedRouteSnapshot(interpreted.route);
  assert.equal(snapshot.state, "requested");
  assert.equal(snapshot.confirmed, null);
});

test("requested, transitioning, confirmed and uncertain route states remain distinct", () => {
  const shared = { destination: { kind: "device", deviceId: "pipewire:sink-fixture" }, access: "shared", processing: "off" };
  const eq = { ...shared, processing: "equalizer" };
  assert.equal(requestedRouteSnapshot(shared).state, "requested");
  const transition = transitioningRouteSnapshot(eq, shared);
  assert.equal(transition.state, "transitioning");
  assert.deepEqual(transition.confirmed, shared);
  assert.equal(confirmedRouteSnapshot(eq, eq).state, "confirmed");
  assert.equal(uncertainRouteSnapshot(eq).state, "uncertain");
  assert.equal(validateAudioRouteSnapshot({ state: "confirmed", requested: eq, confirmed: shared }), "invalid_snapshot_state");
  assert.throws(() => confirmedRouteSnapshot(eq, shared), /requested_and_observed_route_differ/);
});

test("invalid and inconsistent modality combinations are rejected", () => {
  assert.equal(validateAudioRoute({ destination: { kind: "device", deviceId: "" }, access: "exclusive", processing: "off" }), "invalid_device_identifier");
  assert.equal(validateAudioRoute({ destination: { kind: "device", deviceId: "default" }, access: "shared", processing: "off" }), "invalid_device_identifier");
  assert.equal(validateAudioRoute({ destination: { kind: "system_default" }, access: "exclusive", processing: "off" }), "exclusive_system_default");
  assert.equal(validateAudioRoute({ destination: { kind: "device", deviceId: "hw:Fixture,0" }, access: "exclusive", processing: "equalizer" }), "equalizer_requires_shared_access");
});

test("route snapshot generation rejects a response older than the latest operation snapshot", () => {
  assert.equal(acceptsAudioRouteGeneration(12, 11), true);
  assert.equal(acceptsAudioRouteGeneration(11, 12), false);
  assert.equal(acceptsAudioRouteGeneration(Number.NaN, 0), false);
  assert.equal(acceptsAudioRouteGeneration(Number.MAX_SAFE_INTEGER + 1, 0), false);
});

test("frontend DTO preserves the backend projection semantics without inferring PCM or WASAPI exclusivity", () => {
  assert.equal(projectionFixture.route.state, "confirmed");
  assert.equal(projectionFixture.route.requested.processing, "equalizer");
  assert.equal(projectionFixture.applied.processing, "off");
  assert.ok(projectionFixture.evidence.includes("dsp_graph_verified"));
  assert.equal(projectionFixture.pcmAcquired, null);
  assert.equal(projectionFixture.exclusiveAccessConfirmed, null);
  assert.equal(acceptsAudioRouteGeneration(projectionFixture.generation, 8), true);
});
