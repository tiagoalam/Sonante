import test from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import ts from 'typescript';
import React from 'react';
import * as jsx from 'react/jsx-runtime';
import { formatEqNumber, parseEqNumber } from '../src/components/eqNumbers.ts';
import * as equalizer from '../src/utils/equalizer.ts';
import { equalizerGraphPath, equalizerGraphRange, sampleEqualizerGraph } from '../src/utils/equalizerGraph.ts';

const nodes = tree => React.isValidElement(tree)
  ? [tree, ...nodes(tree.props.children)]
  : Array.isArray(tree) ? tree.flatMap(nodes) : [];
const inputOf = tree => nodes(tree).find(node => node.type === 'input');
const errorOf = tree => nodes(tree).find(node => node.props.role === 'alert');

function editor(initial, { digits = 2, locale = 'en', min = -15, max = 15, step = .1 } = {}) {
  let value = initial, invalid = false, states = [], refs = [], effects = [], dependencies = [], si = 0, ri = 0, ei = 0;
  const writes = [];
  const hooks = {
    useState(v) { const i = si++; if (!(i in states)) states[i] = typeof v === 'function' ? v() : v; return [states[i], next => states[i] = next]; },
    useRef(v) { return refs[ri++] ??= { current: v }; },
    useEffect(fn, deps) { const i = ei++; if (!dependencies[i] || deps.some((v, k) => !Object.is(v, dependencies[i][k]))) { effects.push(fn); dependencies[i] = deps; } },
  };
  const code = ts.transpileModule(readFileSync(new URL('../src/components/EqNumberInput.tsx', import.meta.url), 'utf8'), { compilerOptions: { jsx: ts.JsxEmit.ReactJSX, module: ts.ModuleKind.CommonJS, target: ts.ScriptTarget.ES2022 } }).outputText;
  const ex = {};
  new Function('require', 'exports', code)(name => ({ react: hooks, 'react/jsx-runtime': jsx, './eqNumbers': { formatEqNumber, parseEqNumber } })[name], ex);
  const message = locale === 'pt-BR' ? 'Ganho permitido: -15 a +15 dB.' : 'Allowed gain: -15 to +15 dB.';
  const errorId = 'eq-band-fixture-gain-error';
  const render = () => {
    si = ri = ei = 0; effects = [];
    const makeInput = () => ex.EqNumberInput({ value, onValueChange: n => { value = n; writes.push(n); }, onValidityChange: next => { invalid = next; }, 'aria-describedby': invalid ? errorId : undefined, locale, digits, min, max, step, 'aria-label': 'numeric fixture' });
    let control = makeInput();
    if (effects.length) { effects.forEach(fn => fn()); si = ri = ei = 0; effects = []; control = makeInput(); }
    const tree = React.createElement(React.Fragment, null,
      React.createElement('div', { 'data-eq-band-fields': true }, React.createElement('label', null, 'Gain', control), React.createElement('label', null, 'Q', React.createElement('input', { 'data-eq-q-field': true }))),
      invalid ? React.createElement('div', { className: 'eq-band-number-errors' }, React.createElement('p', { id: errorId, role: 'alert', 'data-eq-field-error': true }, message)) : null);
    return tree;
  };
  return {
    render, writes, value: () => value, locale: next => locale = next, external: next => value = next,
    key(key) { const input = inputOf(render()); input.props.onKeyDown({ key, preventDefault() {}, stopPropagation() {}, currentTarget: { blur: () => input.props.onBlur({ currentTarget: { value: input.props.value } }) } }); },
  };
}

function assertVisibleCurve(gain) {
  const preset = equalizer.flatPreset();
  preset.bands[0].enabled = true; preset.bands[0].gain_db = gain;
  assert.equal(equalizer.validDraft(preset), true);
  const sampled = sampleEqualizerGraph(preset, null, false);
  const range = equalizerGraphRange([...sampled.individual, sampled.requested]);
  const path = equalizerGraphPath(sampled.requested, 48, 904, 28, 268, range);
  assert.ok(path.length > 0);
  assert.doesNotMatch(path, /NaN|Infinity/);
  assert.ok(path.split(/[ML]/).filter(Boolean).every(point => point.split(',').length === 2 && point.split(',').every(value => Number.isFinite(Number(value)))));
}

test('formatted values and locale changes never rewrite the original number', () => {
  for (const [initial, digits] of [[1199.999973, 0], [4.24999992, 2], [1.23456789, 2], [-.123456789, 2]]) {
    const h = editor(initial, { digits, min: -10000, max: 20000 });
    let control = inputOf(h.render()); assert.equal(control.props.value, formatEqNumber(initial, digits));
    control.props.onFocus(); inputOf(h.render()).props.onBlur({ currentTarget: { value: String(initial) } });
    h.locale('pt-BR'); h.render(); h.locale('en'); h.render();
    assert.equal(h.value(), initial); assert.deepEqual(h.writes, []);
  }
});

test('per-filter gains accept -15, -12, 0, +12 and +15 dB', () => {
  for (const [text, gain] of [['-15', -15], ['-12', -12], ['0', 0], ['+12', 12], ['+15', 15]]) {
    const h = editor(0); inputOf(h.render()).props.onFocus();
    inputOf(h.render()).props.onChange({ target: { value: text } });
    assert.equal(h.value(), gain); assert.equal(equalizer.clampGain(gain), gain); assertVisibleCurve(gain);
    const control = inputOf(h.render()); assert.equal(control.props['aria-invalid'], undefined); assert.equal(errorOf(h.render()), undefined);
  }
  assert.equal(equalizer.clampGain(100), 15); assert.equal(equalizer.clampGain(-100), -15);
  assert.equal(equalizer.validDraft({ ...equalizer.flatPreset(), preamp_db: 12 }), true);
  assert.equal(equalizer.validDraft({ ...equalizer.flatPreset(), preamp_db: -24 }), true);
});

test('invalid and incomplete entries keep the last valid value and graph visible, show an error, and recover on correction', () => {
  for (const text of ['+16', '-16', '', '-', '.', '1e-', 'not a number']) {
    const h = editor(3); inputOf(h.render()).props.onFocus();
    inputOf(h.render()).props.onChange({ target: { value: text } });
    const tree = h.render(), control = inputOf(tree), error = errorOf(tree);
    assert.equal(h.value(), 3, `${JSON.stringify(text)} must not publish to the preset`);
    assert.deepEqual(h.writes, []);
    assert.equal(control.props.value, text); assert.equal(control.props['aria-invalid'], true);
    assert.equal(error.props.children, 'Allowed gain: -15 to +15 dB.');
    assert.equal(control.props['aria-describedby'], error.props.id);
    const row = nodes(tree).find(node => node.props['data-eq-band-fields'] !== undefined);
    assert.equal(nodes(row).find(node => node.props['data-eq-q-field'] !== undefined).props['data-eq-q-field'], true);
    assert.equal(row.props.children.length, 2, 'the feedback message is outside the fixed-height Gain/Q row');
    assertVisibleCurve(h.value());
    control.props.onBlur({ currentTarget: { value: text } });
    assert.equal(inputOf(h.render()).props.value, text, 'the invalid text remains available for correction');
    inputOf(h.render()).props.onChange({ target: { value: '+15' } });
    assert.equal(h.value(), 15); assert.equal(errorOf(h.render()), undefined); assertVisibleCurve(h.value());
  }
});

test('localized decimals and Escape preserve correction behavior without committing incomplete decimals', () => {
  const h = editor(4.24999992); inputOf(h.render()).props.onFocus();
  inputOf(h.render()).props.onChange({ target: { value: '-' } }); assert.equal(h.value(), 4.24999992);
  assert.equal(inputOf(h.render()).props['aria-invalid'], true);
  inputOf(h.render()).props.onChange({ target: { value: '-4,' } }); assert.equal(h.value(), 4.24999992);
  inputOf(h.render()).props.onChange({ target: { value: '-4,25' } }); assert.equal(h.value(), -4.25);
  h.locale('pt-BR'); assert.equal(inputOf(h.render()).props.value, '-4,25');
  inputOf(h.render()).props.onBlur({ currentTarget: { value: '-4,25' } }); assert.equal(inputOf(h.render()).props.value, '-4,25');
  inputOf(h.render()).props.onFocus(); inputOf(h.render()).props.onChange({ target: { value: '16' } }); h.key('Escape');
  assert.equal(h.value(), -4.25); assert.equal(inputOf(h.render()).props.value, '-4,25'); assert.equal(errorOf(h.render()), undefined);
});

test('keyboard steps use the unrounded model and external updates replace only displayed text', () => {
  const raw = 4.24999992, h = editor(raw); inputOf(h.render()).props.onFocus(); h.key('ArrowUp');
  assert.equal(h.value(), raw + .1); assert.equal(inputOf(h.render()).props.value, '4.35'); h.key('Escape'); assert.equal(h.value(), raw);
  h.external(2.000000001); assert.equal(inputOf(h.render()).props.value, '2'); assert.equal(h.value(), 2.000000001);
  h.locale('pt-BR'); h.external(4.25); assert.equal(inputOf(h.render()).props.value, '4,25'); assert.equal(h.value(), 4.25);
});

test('parsing rejects nonnumeric, empty, incomplete and out-of-range representations without coercing to zero', () => {
  for (const [text, value] of [['1200', 1200], ['4,25', 4.25], ['.25', .25], ['-0.25', -.25], ['1e3', 1000]]) assert.equal(parseEqNumber(text), value);
  for (const text of ['', '-', '.', ',', '1e', '1e-', '1,2.3', '4,', '  ']) assert.ok(Number.isNaN(parseEqNumber(text)), text);
});
