import assert from 'node:assert/strict';
import test from 'node:test';
import { readFileSync } from 'node:fs';
import React from 'react';
import * as jsx from 'react/jsx-runtime';
import ts from 'typescript';

const dictionaries = Object.fromEntries(['en', 'pt-BR'].map(language => [language,
  JSON.parse(readFileSync(new URL(`../src/locales/${language}.json`, import.meta.url), 'utf8'))]));

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

function nodes(tree, predicate) {
  return Array.isArray(tree) ? tree.flatMap(node => nodes(node, predicate))
    : !tree || typeof tree !== 'object' ? []
      : [...(predicate(tree) ? [tree] : []), ...nodes(tree.props?.children, predicate)];
}

test('the shared source header renders an accessible icon, title, description and secondary metadata', () => {
  function TestIcon() {}
  const { SourceHeader } = compile(new URL('../src/components/SourceHeader.tsx', import.meta.url), {
    'react/jsx-runtime': jsx,
  });
  const tree = SourceHeader({
    icon: TestIcon,
    title: 'Plex',
    description: 'Explore connected music libraries.',
    meta: '3 albums',
    accentClassName: 'text-orange-400',
  });
  const rendered = nodes(tree, () => true);
  assert.equal(rendered.find(node => node.type === TestIcon).props['aria-hidden'], 'true');
  assert.ok(rendered.some(node => node.type === 'h1' && node.props.children === 'Plex'));
  assert.ok(rendered.some(node => node.type === 'p' && node.props.children === 'Explore connected music libraries.'));
  assert.ok(rendered.some(node => node.props.className?.includes('flex-wrap') && node.props.children === '3 albums'));
  assert.ok(rendered.some(node => node.props.className?.includes('break-words')));
});

test('all four source roots use the shared header and preserve their source-specific controls', () => {
  const source = path => readFileSync(new URL(`../src/${path}`, import.meta.url), 'utf8');
  const local = source('components/LocalBrowserView.tsx');
  const plex = source('App.tsx');
  const subsonic = source('components/NavidromeView.tsx');
  const radio = source('components/RadioView.tsx');

  assert.match(local, /<SourceHeader icon=\{Music2\} title=\{t\("sources\.names\.local"\)\}/);
  assert.match(local, /showLocalSourceHeader/);
  assert.match(plex, /<SourceHeader icon=\{Disc3\} title=\{t\("sources\.names\.plex"\)\}/);
  assert.match(plex, /showPlexSourceHeader/);
  assert.match(subsonic, /<SourceHeader icon=\{Server\} title=\{t\('sources\.names\.navidrome'\)\}/);
  assert.match(radio, /<SourceHeader icon=\{Radio\} title=\{t\("radio\.title"\)\} description=\{t\("radio\.subtitle"\)\}/);
  assert.match(radio, /radio\.navigation/);
  assert.match(subsonic, /AlbumCatalog/);
  assert.match(local, /localBrowser\.filterPlaceholder/);

  for (const language of ['en', 'pt-BR']) {
    const dictionary = dictionaries[language];
    assert.ok(dictionary.sourceHeaders.localDescription);
    assert.ok(dictionary.sourceHeaders.plexDescription);
    assert.ok(dictionary.sourceHeaders.subsonicDescription);
    assert.equal(dictionary.sources.names.local, 'Local');
    assert.equal(dictionary.sources.names.plex, 'Plex');
    assert.equal(dictionary.sources.names.navidrome, 'Subsonic');
    assert.ok(dictionary.radio.subtitle, `${language} keeps the established radio description`);
  }
});
