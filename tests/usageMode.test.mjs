import test from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import ts from 'typescript';
import React from 'react';
import * as jsx from 'react/jsx-runtime';
import { renderToStaticMarkup } from 'react-dom/server';
import { fullModeEnabled, isAdvancedSection } from '../src/utils/usageMode.ts';
import { experimentalFeaturesEnabled } from '../src/utils/experimentalFeatures.ts';
import { audioRestartExpected } from '../src/utils/audioConfigChange.ts';
import { createMainSectionPreference } from '../src/utils/appBootstrap.ts';

test('legacy profiles keep Full; Simple suspends beta without losing the saved opt-in', () => {
  for (const config of [undefined, null, {}, { usage_mode: 'full' }]) assert.equal(fullModeEnabled(config), true);
  const saved = { usage_mode: 'simple', experimental_features: true };
  assert.equal(fullModeEnabled(saved), false); assert.equal(experimentalFeaturesEnabled(saved), false);
  assert.equal(experimentalFeaturesEnabled({ ...saved, usage_mode: 'full' }), true);
  assert.equal(saved.experimental_features, true);
});
test('Simple leaves basic routes available and refuses advanced sections, including restored Equalizer', () => {
  for (const section of ['local','plex','navidrome','radio','search','favorites','history','playlists']) assert.equal(isAdvancedSection(section), false);
  for (const section of ['explore','equalizer','sessions','editions']) assert.equal(isAdvancedSection(section), true);
  const calls = [], writes = [];
  const controller = createMainSectionPreference(value => calls.push(value), () => ({ getItem: () => 'equalizer', setItem: (...args) => writes.push(args) }));
  controller.restore({ first_run: false, plex_token: '', usage_mode: 'simple' });
  assert.equal(calls.at(-1), 'local'); assert.deepEqual(writes, []);
  controller.restore({ first_run: false, plex_token: '', usage_mode: 'full' }); assert.equal(calls.at(-1), 'equalizer');
});
test('mode-only transitions never request an audio restart in either route', () => {
  for (const audio_output_type of ['alsa','pipewire']) for (const usage_mode of ['full','simple']) {
    const config = { audio_output_type, alsa_device: audio_output_type === 'alsa' ? 'hw:Fixture,0' : 'default', dop_enabled: true, replay_gain: 'album', audio_buffer_size_kb: 8192 };
    assert.equal(audioRestartExpected(config, { ...config, usage_mode }), false);
  }
});
test('mode choice renders labelled radio options, preserves Full by default, and supports both languages', () => {
  for (const language of ['pt-BR','en']) {
    const dictionary = JSON.parse(readFileSync(new URL(`../src/locales/${language}.json`, import.meta.url), 'utf8'));
    const t = key => key.split('.').reduce((value, part) => value?.[part], dictionary);
    const exports = {}, deps = { react: React, 'react/jsx-runtime': jsx, 'react-i18next': { useTranslation: () => ({ t }) } };
    const js = ts.transpileModule(readFileSync(new URL('../src/components/UsageModeSetting.tsx', import.meta.url),'utf8'), { compilerOptions: { jsx: ts.JsxEmit.ReactJSX, module: ts.ModuleKind.CommonJS } }).outputText;
    new Function('require','exports',js)(name => deps[name], exports);
    for (const mode of [undefined, 'full', 'simple']) {
      const html = renderToStaticMarkup(React.createElement(exports.UsageModeSetting, { value: mode, onChange() {} }));
      assert.equal((html.match(/type="radio"/g) ?? []).length, 2);
      assert.match(html, new RegExp(`checked=""[^>]*value="${mode ?? 'full'}"`));
      assert.ok(html.includes(t('usageMode.help'))); assert.match(html, /aria-describedby=/);
    }
  }
});
