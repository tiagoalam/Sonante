import test from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import ts from "typescript";
import React from "react";
import * as jsx from "react/jsx-runtime";
import { renderToStaticMarkup } from "react-dom/server";
import i18next from "i18next";
import * as audioRoute from "./audioRoute.ts";
import * as audioExperience from "./audioExperience.ts";
import { equalizerActivationBlockReason } from "./equalizerAvailability.ts";
import { equalizerErrorCode, equalizerFailureMessageKey } from "./equalizerError.ts";
import * as equalizer from "./equalizer.ts";

const i18n = i18next.createInstance();
await i18n.init({
  lng: "en",
  resources: Object.fromEntries(["en", "pt-BR"].map(language => [language, {
    translation: JSON.parse(readFileSync(new URL(`../locales/${language}.json`, import.meta.url), "utf8")),
  }])),
});

function compile(file, dependencies) {
  const source = readFileSync(new URL(`../${file}`, import.meta.url), "utf8");
  const code = ts.transpileModule(source, { compilerOptions: {
    jsx: ts.JsxEmit.ReactJSX, module: ts.ModuleKind.CommonJS, target: ts.ScriptTarget.ES2020,
  } }).outputText;
  const exports = {};
  new Function("require", "exports", code)(id => {
    assert.ok(id in dependencies, `unexpected import ${id}`);
    return dependencies[id];
  }, exports);
  return exports;
}

function harness(dependencies) {
  let cursor = 0;
  const slots = [];
  const effects = [];
  const hooks = {
    ...React,
    useState(initial) {
      const key = cursor++;
      if (!(key in slots)) slots[key] = typeof initial === "function" ? initial() : initial;
      return [slots[key], value => { slots[key] = typeof value === "function" ? value(slots[key]) : value; }];
    },
    useRef(initial) { const key = cursor++; return slots[key] ??= { current: initial }; },
    useEffect(effect, deps) {
      const key = cursor++;
      const old = slots[key];
      if (!old || !deps || deps.some((value, i) => !Object.is(value, old.deps[i]))) {
        effects.push(() => { old?.cleanup?.(); slots[key] = { deps, cleanup: effect() }; });
      }
    },
  };
  const exports = compile("components/EqualizerToggle.tsx", {
    react: { ...hooks, default: hooks, __esModule: true },
    "react/jsx-runtime": jsx,
    "react-i18next": { useTranslation: () => ({ t: i18n.t.bind(i18n) }) },
    "lucide-react": { SlidersHorizontal: () => null, AlertCircle: () => null },
    "../services/equalizer": { equalizerService: dependencies.equalizerService },
    "../utils/audioExperience": audioExperience,
    "../utils/equalizerError": { equalizerErrorCode, equalizerFailureMessageKey },
    "../utils/equalizer": equalizer,
    "../utils/equalizerAvailability": { equalizerActivationBlockReason },
  });
  return {
    render(props) {
      cursor = 0;
      const tree = exports.EqualizerToggle(props);
      effects.splice(0).forEach(run => run());
      return tree;
    },
    dispose() { slots.forEach(slot => slot?.cleanup?.()); },
  };
}

function nodes(tree) {
  if (!React.isValidElement(tree)) return Array.isArray(tree) ? tree.flatMap(nodes) : [];
  return [tree, ...nodes(tree.props.children)];
}
function inputOf(tree) { return nodes(tree).find(node => node.type === "input" && node.props.role === "switch"); }

const flat = { id: "flat", name: "Flat", preamp_db: 0, bands: [] };
const offRuntime = { phase: "off", verified: false, in_path: false, revision: 1, applied: null };
const activeRuntime = { phase: "ready", verified: true, in_path: true, revision: 2, applied: flat };
const catalog = {
  built_ins: [flat],
  config: { active_preset: { kind: "built_in", id: "flat" }, user_presets: [] },
  runtime: offRuntime,
  runtime_available: true,
  pulse_audio: false,
};
const sharedOff = { destination: { kind: "system_default" }, access: "shared", processing: "off" };
const sharedEq = { ...sharedOff, processing: "equalizer" };
const transition = {
  generation: 11,
  operationId: "route:8",
  operation: "transitioning",
  route: { state: "transitioning", requested: sharedEq, confirmed: sharedOff },
  applied: sharedOff,
  evidence: [], diagnostic: null, pcmAcquired: null, exclusiveAccessConfirmed: null, legacyIssues: [],
};
const confirmed = {
  ...transition,
  generation: 12,
  operationId: "route:9",
  operation: "stable",
  route: { state: "confirmed", requested: sharedEq, confirmed: sharedEq },
  evidence: ["dsp_graph_verified"],
};

function services({ apply } = {}) {
  let reads = 0;
  let resolveAuxiliary;
  return {
    get reads() { return reads; },
    equalizerService: {
      get() {
        reads++;
        if (reads === 3) return new Promise(resolve => { resolveAuxiliary = resolve; });
        return Promise.resolve(catalog);
      },
      apply: apply ?? (() => Promise.resolve({ runtime: activeRuntime, audioRoute: confirmed })),
    },
    finishAuxiliary(value = catalog) { resolveAuxiliary?.(value); },
  };
}
const flush = async () => { for (let i = 0; i < 12; i++) await Promise.resolve(); };

test("backend confirmation clears stale route transition while catalog refresh remains pending", async () => {
  const fixture = services({ apply: async () => {
    await new Promise(resolve => setTimeout(resolve, 453));
    return { runtime: activeRuntime, audioRoute: confirmed };
  } });
  const component = harness(fixture);
  let visibleRoute = { ...transition, operation: "stable", route: { state: "confirmed", requested: sharedOff, confirmed: sharedOff } };
  let visibleRuntime = offRuntime;
  const props = () => ({ shared: true, available: true, runtime: visibleRuntime, audioRoute: visibleRoute,
    onAudioRouteConfirmed(route, runtime) { visibleRoute = route; visibleRuntime = runtime; } });

  let tree = component.render(props());
  await flush();
  tree = component.render(props());
  inputOf(tree).props.onChange({ currentTarget: { checked: true } });
  await flush();
  tree = component.render(props());
  assert.equal(inputOf(tree).props.disabled, true, "an unresolved backend transition remains protected");

  visibleRoute = transition;
  tree = component.render(props());
  assert.equal(inputOf(tree).props.disabled, true);

  await new Promise(resolve => setTimeout(resolve, 453));
  await flush();
  tree = component.render(props());
  assert.equal(fixture.reads, 3, "the auxiliary catalog refresh was started but not awaited");
  assert.equal(inputOf(tree).props.checked, true, "the backend DSP snapshot drives the toggle");
  assert.equal(inputOf(tree).props.disabled, false, "a confirmed route can be interacted with while refresh is pending");
  assert.equal(visibleRoute.operation, "stable");
  assert.equal(audioExperience.audioRouteStatusKind(visibleRoute), "confirmed");
  component.dispose();
});

test("a second EQ request cannot overlap the in-flight backend transition", async () => {
  let resolveApply;
  let applyCalls = 0;
  const fixture = services({ apply: () => {
    applyCalls++;
    return new Promise(resolve => { resolveApply = resolve; });
  } });
  const component = harness(fixture);
  const props = { shared: true, available: true, runtime: offRuntime,
    audioRoute: { ...transition, operation: "stable", route: { state: "confirmed", requested: sharedOff, confirmed: sharedOff } } };
  let tree = component.render(props);
  await flush();
  tree = component.render(props);
  inputOf(tree).props.onChange({ currentTarget: { checked: true } });
  await flush();
  tree = component.render(props);
  assert.equal(inputOf(tree).props.disabled, true);
  inputOf(tree).props.onChange({ currentTarget: { checked: true } });
  await flush();
  assert.equal(applyCalls, 1, "the pending transition owns the toggle until its backend result");
  resolveApply({ runtime: activeRuntime, audioRoute: confirmed });
  await flush();
  component.dispose();
});

test("backend failure does not confirm a route and a late response after unmount is ignored", async () => {
  let rejectApply;
  const fixture = services({ apply: () => new Promise((_, reject) => { rejectApply = reject; }) });
  const component = harness(fixture);
  let confirmations = 0;
  const props = { shared: true, available: true, runtime: offRuntime,
    audioRoute: { ...transition, operation: "stable", route: { state: "confirmed", requested: sharedOff, confirmed: sharedOff } },
    onAudioRouteConfirmed() { confirmations++; } };
  let tree = component.render(props); await flush(); tree = component.render(props);
  inputOf(tree).props.onChange({ currentTarget: { checked: true } }); await flush();
  rejectApply("EQ_SinkUnavailable"); await flush(); tree = component.render(props);
  assert.equal(confirmations, 0);
  assert.equal(inputOf(tree).props.checked, false);
  assert.equal(nodes(tree).some(node => node.props.role === "alert"), true);
  component.dispose();

  let resolveApply;
  const late = harness(services({ apply: () => new Promise(resolve => { resolveApply = resolve; }) }));
  let lateConfirmations = 0;
  const lateProps = { ...props, onAudioRouteConfirmed() { lateConfirmations++; } };
  let lateTree = late.render(lateProps); await flush(); lateTree = late.render(lateProps);
  inputOf(lateTree).props.onChange({ currentTarget: { checked: true } }); await flush();
  late.dispose();
  resolveApply({ runtime: activeRuntime, audioRoute: confirmed }); await flush();
  assert.equal(lateConfirmations, 0, "unmounted preferences cannot apply an obsolete UI response");
});
