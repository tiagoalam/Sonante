import assert from 'node:assert/strict';
import test from 'node:test';
import { readFileSync } from 'node:fs';
import React from 'react';
import * as jsx from 'react/jsx-runtime';
import ts from 'typescript';

const dictionaries = Object.fromEntries(['en', 'pt-BR'].map(language => [language,
  JSON.parse(readFileSync(new URL(`../src/locales/${language}.json`, import.meta.url), 'utf8'))]));
const translate = (key, language = 'en', values) => {
  const message = key.split('.').reduce((value, part) => value?.[part], dictionaries[language]) ?? key;
  if (typeof message !== 'string' || !values) return message;
  return Object.entries(values).reduce((text, [name, value]) => text.replaceAll(`{{${name}}}`, String(value)), message);
};
const same = (left, right) => left?.length === right?.length && left.every((value, index) => Object.is(value, right[index]));
let current;
const hooks = {
  ...React,
  useState(initial) {
    const host = current, index = host.cursor++;
    host.slots[index] ??= { value: typeof initial === 'function' ? initial() : initial };
    return [host.slots[index].value, next => { host.slots[index].value = typeof next === 'function' ? next(host.slots[index].value) : next; }];
  },
  useRef(value) { return current.slots[current.cursor++] ??= { current: value }; },
  useEffect(effect, dependencies) {
    const host = current, index = host.cursor++, previous = host.slots[index];
    if (!same(previous?.dependencies, dependencies)) host.effects.push(() => {
      previous?.cleanup?.();
      const slot = { dependencies, cleanup: effect() };
      host.slots[index] = slot;
    });
  },
};
function compile(path, dependencies) {
  const code = ts.transpileModule(readFileSync(path, 'utf8'), {
    compilerOptions: { module: ts.ModuleKind.CommonJS, jsx: ts.JsxEmit.ReactJSX, target: ts.ScriptTarget.ES2022 },
  }).outputText;
  const exports = {};
  new Function('require', 'exports', code)(name => {
    assert.ok(name in dependencies, `unexpected import ${name}`);
    return dependencies[name];
  }, exports);
  return exports;
}
const service = compile(new URL('../src/services/navidrome.ts', import.meta.url), {
  '@tauri-apps/api/core': { invoke: async () => undefined },
  './config': { ensureDirectOutputChoice: async () => undefined },
  '../utils/localSourceReadQueue': { localSourceReadQueue: { run: async operation => operation() } },
  '../utils/playbackIntent': { playbackIntentToken: () => 1 },
});
const component = compile(new URL('../src/components/NavidromeConnectionSetting.tsx', import.meta.url), {
  react: { __esModule: true, default: React, ...hooks },
  'react/jsx-runtime': jsx,
  'react-i18next': { useTranslation: () => ({ t: translate }) },
  '../services/navidrome': service,
  '../utils/platform': { isWindowsPreview: () => false },
}).NavidromeConnectionSetting;
const nodes = (tree, predicate) => Array.isArray(tree) ? tree.flatMap(node => nodes(node, predicate))
  : !tree || typeof tree !== 'object' ? []
    : [...(predicate(tree) ? [tree] : []), ...nodes(tree.props?.children, predicate)];
async function flush() { for (let i = 0; i < 30; i++) await Promise.resolve(); }
function fixture(initialStatus, windowsPreview = false, language = 'en') {
  const calls = [], configured = [], busy = [];
  let status = initialStatus;
  const api = {
    status: async () => status,
    connect: async (...args) => {
      calls.push(args);
      status = { connection_id: 'session', server: args[0], username: args[1] || null, auth_method: args[3] };
      return status;
    },
    disconnect: async () => ({ connection_id: null, server: null, username: null, auth_method: null }),
  };
  const loaded = compile(new URL('../src/components/NavidromeConnectionSetting.tsx', import.meta.url), {
    react: { __esModule: true, default: React, ...hooks },
    'react/jsx-runtime': jsx,
    'react-i18next': { useTranslation: () => ({ t: (key, values) => translate(key, language, values) }) },
    '../services/navidrome': { ...service, navidromeService: api },
    '../utils/platform': { isWindowsPreview: () => windowsPreview },
  }).NavidromeConnectionSetting;
  const host = {
    slots: [], cursor: 0, effects: [], tree: null,
    render() {
      this.cursor = 0; this.effects = []; current = this;
      this.tree = loaded({ onConfiguredChange: value => configured.push(value), onBusyChange: value => busy.push(value) });
      current = null; this.effects.splice(0).forEach(effect => effect());
      return this.tree;
    },
  };
  return { host, calls, configured, busy };
}
const field = (tree, type) => nodes(tree, node => node.type === 'input' && node.props.type === type)[0];

test('saved API-key connections remain configured and HTTP displays the transport warning', async () => {
  const f = fixture({ connection_id: null, server: 'http://music.example/subsonic/', username: null, auth_method: 'api_key' });
  f.host.render(); await flush(); const tree = f.host.render();
  assert.equal(nodes(tree, node => node.type === 'select')[0].props.value, 'api_key');
  assert.ok(nodes(tree, node => node.props.role === 'alert').some(node => JSON.stringify(node.props.children).includes(translate('navidrome.httpWarning'))));
  assert.deepEqual(f.configured, [true]);
  assert.equal(field(tree, 'password').props.maxLength, 2047);
  assert.equal(translate('sources.names.navidrome'), 'Subsonic');
});

test('settings and wizard explain the Subsonic API compatibility scope in English and Brazilian Portuguese', async () => {
  for (const language of ['en', 'pt-BR']) {
    const f = fixture({ connection_id: null, server: null, username: null, auth_method: null }, false, language);
    f.host.render(); await flush();
    const tree = f.host.render();
    const paragraphs = nodes(tree, node => node.type === 'p').map(node => node.props.children).filter(value => typeof value === 'string');
    assert.ok(paragraphs.includes(translate('navidrome.connectionHint', language)));

    for (const key of ['wizard.navidromeDesc', 'navidrome.connectionHint']) {
      const message = translate(key, language);
      for (const server of ['Navidrome', 'Airsonic', 'Gonic', 'Ampache']) assert.ok(message.includes(server), `${language} ${key} names ${server}`);
      assert.match(message, /Subsonic\/OpenSubsonic/);
    }
    assert.match(translate('wizard.navidromeDesc', language), language === 'en' ? /vary by server/i : /variam conforme o servidor/i);
    assert.match(translate('navidrome.connectionHint', language), language === 'en' ? /vary by server/i : /variam conforme o servidor/i);
    assert.equal(translate('sources.names.navidrome', language), 'Subsonic');
  }

  const wizard = readFileSync(new URL('../src/components/WelcomeWizard.tsx', import.meta.url), 'utf8');
  assert.ok(wizard.includes('t("wizard.navidromeDesc")'), 'the connection explanation remains visible in the wizard');
});

test('API-key mode permits an optional account label and passes only the entered connection values', async () => {
  const f = fixture({ connection_id: null, server: null, username: null, auth_method: null });
  f.host.render(); await flush(); let tree = f.host.render();
  const server = field(tree, 'url'); server.props.onChange({ target: { value: 'https://music.example/base/' } });
  tree = f.host.render();
  const auth = nodes(tree, node => node.type === 'select')[0];
  auth.props.onChange({ target: { value: 'api_key' } }); tree = f.host.render();
  const inputs = nodes(tree, node => node.type === 'input');
  const label = inputs.find(input => input.props.type === 'text');
  label.props.onChange({ target: { value: '' } }); tree = f.host.render();
  field(tree, 'password').props.onChange({ target: { value: 'fixture-api-key' } }); tree = f.host.render();
  const connect = nodes(tree, node => node.type === 'button' && node.props.children === translate('navidrome.connect'))[0];
  assert.equal(connect.props.disabled, false);
  await connect.props.onClick(); await flush(); tree = f.host.render();
  assert.deepEqual(f.calls, [['https://music.example/base/', '', 'fixture-api-key', 'api_key']]);
  assert.deepEqual(f.configured, [false, true]);
  assert.equal(nodes(tree, node => node.type === 'select')[0].props.value, 'api_key');
});

test('older saved connection status without auth_method defaults to password authentication', async () => {
  const f = fixture({ connection_id: null, server: 'https://legacy.example/', username: 'alice' });
  f.host.render(); await flush(); const tree = f.host.render();
  assert.equal(nodes(tree, node => node.type === 'select')[0].props.value, 'password');
  assert.equal(field(tree, 'text').props.value, 'alice');
  assert.equal(field(tree, 'password').props.maxLength, 4096);
  assert.deepEqual(f.configured, [true]);
});

test('server details show only capabilities actually returned by the Subsonic server', async () => {
  const f = fixture({
    connection_id: 'session',
    server: 'https://fixture.invalid/subsonic/',
    username: 'fixture-user',
    auth_method: 'password',
    server_type: 'Gonic',
    server_version: '0.20.0',
    api_version: '1.16.1',
    open_subsonic: true,
    extensions: [{ name: 'transcoding', versions: [1, 2] }, { name: 'formPost', versions: [] }],
  });
  f.host.render(); await flush();
  const tree = f.host.render();
  const info = nodes(tree, node => node.props['data-subsonic-server-info'])[0];
  const visible = JSON.stringify(info.props.children);
  assert.match(visible, /Gonic/);
  assert.match(visible, /0\.20\.0/);
  assert.match(visible, /1\.16\.1/);
  assert.match(visible, /OpenSubsonic/);
  assert.match(visible, /transcoding v1\/2/);
  assert.match(visible, /formPost/);
  assert.doesNotMatch(visible, /fixture\.invalid|fixture-user/);
});

test('server details remain neutral when a legacy server omits optional identification fields', async () => {
  const f = fixture({ connection_id: 'session', server: 'https://fixture.invalid/', username: 'fixture-user' });
  f.host.render(); await flush();
  const tree = f.host.render();
  assert.equal(nodes(tree, node => node.props['data-subsonic-server-info']).length, 0);
});

test('Windows preview keeps the existing password-only Navidrome backend contract', async () => {
  const f = fixture({ connection_id: null, server: null, username: null, auth_method: null }, true);
  f.host.render(); await flush(); const tree = f.host.render();
  const options = nodes(tree, node => node.type === 'option').map(node => node.props.value);
  assert.deepEqual(options, ['password']);
  assert.equal(nodes(tree, node => node.type === 'select')[0].props.value, 'password');
});

test('new protocol errors map to translated permission and capability messages', () => {
  assert.equal(service.navidromeError('NAVIDROME_PERMISSION'), 'permission');
  assert.equal(service.navidromeError('NAVIDROME_API_KEY_UNSUPPORTED'), 'apiKeyUnsupported');
  assert.equal(service.navidromeError('NAVIDROME_AUTH_METHOD_UNSUPPORTED'), 'authMethodUnsupported');
  for (const code of ['permission', 'apiKeyUnsupported', 'authMethodUnsupported']) {
    assert.notEqual(translate(`navidrome.errors.${code}`), `navidrome.errors.${code}`);
  }
});
