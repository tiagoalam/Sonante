import test from "node:test";
import assert from "node:assert/strict";
import { readFileSync, readdirSync } from "node:fs";
import ts from "typescript";
import i18next from "i18next";

const src = new URL("../", import.meta.url);
const resources = Object.fromEntries(["en", "pt-BR"].map(locale => [locale, {
  translation: JSON.parse(readFileSync(new URL(`locales/${locale}.json`, src), "utf8")),
}]));
function flatten(value, prefix = "", result = {}) {
  for (const [name, entry] of Object.entries(value)) {
    const key = prefix ? `${prefix}.${name}` : name;
    if (typeof entry === "string") result[key] = entry;
    else flatten(entry, key, result);
  }
  return result;
}
const catalogs = Object.fromEntries(Object.entries(resources).map(([lng, data]) => [lng, flatten(data.translation)]));
const parameters = value => [...new Set([...value.matchAll(/{{-?\s*(\w+)(?:\s*,[^}]+)?\s*}}/g)].map(match => match[1]))].sort();

test("About shows version-only licensing and preserves third-party ownership", async () => {
  const component = readFileSync(new URL("components/AboutModal.tsx", src), "utf8");
  assert.match(component, /t\("about\.license", \{ license: "GPL-3\.0-only" \}\)/);
  for (const lng of ["en", "pt-BR"]) {
    const instance = i18next.createInstance();
    await instance.init({ lng, resources, fallbackLng: false });
    const message = instance.t("about.license", { license: "GPL-3.0-only" });
    assert.ok(message.includes("GPL-3.0-only"));
    assert.ok(message.includes("2026 Tiago Alam"));
    assert.doesNotMatch(message, /{{|or later|ou posterior/);
  }
});

test("locale keys, interpolation variables and plural families match", () => {
  assert.deepEqual(Object.keys(catalogs.en).sort(), Object.keys(catalogs["pt-BR"]).sort());
  for (const [key, value] of Object.entries(catalogs.en)) {
    assert.ok(value.trim(), key);
    assert.ok(catalogs["pt-BR"][key].trim(), key);
    assert.deepEqual(parameters(value), parameters(catalogs["pt-BR"][key]), key);
    if (key.endsWith("_one")) assert.ok(catalogs.en[key.replace(/_one$/, "_other")], key);
  }
});

test("every message resolves and interpolates independently in each language", async () => {
  for (const lng of ["en", "pt-BR"]) {
    const instance = i18next.createInstance();
    await instance.init({ lng, resources, fallbackLng: false });
    for (const [key, value] of Object.entries(catalogs[lng])) {
      const variables = Object.fromEntries(parameters(value).map(name => [name, name === "count" ? 2 : `fixture_${name}`]));
      assert.equal(instance.exists(key, variables), true, `${lng}: ${key}`);
      const rendered = instance.t(key, variables);
      assert.notEqual(rendered, key);
      assert.doesNotMatch(rendered, /{{[^}]+}}/, `${lng}: ${key}`);
      for (const item of Object.values(variables)) assert.ok(rendered.includes(String(item)), `${lng}: ${key}`);
    }
  }
});

// Brands, format names, units, language autonyms and address examples stay literal.
const technicalText = new Set(["SONANTE", "Sonante", `v${JSON.parse(readFileSync(new URL("../../package.json", import.meta.url), "utf8")).version}`, "Linux • MPD • ALSA", "Português (Brasil)", "English (US)", "PT", "EN", "hw:CARD,DEV • ALSA Direct", "PipeWire / PulseAudio / dmix", "PipeWire / PulseAudio", "4 MB", "8 MB", "16 MB", "32 MB"]);

test("frontend translation calls resolve, including conditional and dynamic key groups", async () => {
  const keys = new Set(), untranslated = [];
  for (const file of readdirSync(src, { recursive: true }).filter(file => file.endsWith(".tsx"))) {
    const tree = ts.createSourceFile(file, readFileSync(new URL(file, src), "utf8"), ts.ScriptTarget.Latest, true, ts.ScriptKind.TSX);
    const declarations = new Map();
    function declarationsVisit(node) {
      if (ts.isVariableDeclaration(node) && ts.isIdentifier(node.name) && node.initializer) declarations.set(node.name.text, node.initializer);
      ts.forEachChild(node, declarationsVisit);
    }
    declarationsVisit(tree);
    function collect(node) {
      if (ts.isStringLiteral(node)) keys.add(node.text);
      else if (ts.isConditionalExpression(node)) { collect(node.whenTrue); collect(node.whenFalse); }
      else if (ts.isParenthesizedExpression(node)) collect(node.expression);
      else if (ts.isIdentifier(node) && declarations.has(node.text)) collect(declarations.get(node.text));
      else if (ts.isTemplateExpression(node)) {
        assert.equal(node.templateSpans.length, 1, file);
        assert.equal(node.templateSpans[0].literal.text, "", file);
        const group = Object.keys(catalogs.en).filter(key => key.startsWith(node.head.text));
        assert.ok(group.length > 0, `${file}: ${node.head.text}`);
        group.forEach(key => keys.add(key));
      } else assert.fail(`${file}: uncovered translation expression ${node.getText(tree)}`);
    }
    function visible(node, text) {
      text = text.trim().replace(/\s+/g, " ");
      if (/[A-Za-zÀ-ž]/.test(text) && !technicalText.has(text) && !/^https?:\/\//.test(text)) untranslated.push(`${file}: ${text}`);
    }
    function visit(node) {
      if (ts.isCallExpression(node) && node.expression.getText(tree) === "t") collect(node.arguments[0]);
      if (ts.isJsxText(node)) visible(node, node.text);
      if (ts.isJsxAttribute(node) && ["title", "placeholder", "aria-label", "alt"].includes(node.name.getText(tree)) && node.initializer && ts.isStringLiteral(node.initializer)) visible(node, node.initializer.text);
      if (ts.isCallExpression(node) && ["alert", "window.alert"].includes(node.expression.getText(tree)) && ts.isStringLiteral(node.arguments[0])) visible(node, node.arguments[0].text);
      ts.forEachChild(node, visit);
    }
    visit(tree);
  }
  assert.deepEqual(untranslated, []);
  for (const lng of ["en", "pt-BR"]) {
    const instance = i18next.createInstance();
    await instance.init({ lng, resources, fallbackLng: false });
    for (const key of keys) for (const count of [0, 1, 2]) assert.equal(instance.exists(key, { count }), true, `${lng}: ${key}`);
  }
});

test("track, folder and artist counts use singular, plural and explicit zero forms", async () => {
  for (const [lng, expected] of [
    ["en", { "common.trackCount": ["0 tracks", "1 track", "2 tracks"], "wizard.foldersCount": ["0 folders", "1 folder", "2 folders"], "artistView.catalogedTracks": ["0 cataloged tracks", "1 cataloged track", "2 cataloged tracks"], "artistView.albumsCount": ["0 albums", "1 album", "2 albums"] }],
    ["pt-BR", { "common.trackCount": ["0 faixas", "1 faixa", "2 faixas"], "wizard.foldersCount": ["0 pastas", "1 pasta", "2 pastas"], "artistView.catalogedTracks": ["0 faixas catalogadas", "1 faixa catalogada", "2 faixas catalogadas"], "artistView.albumsCount": ["0 álbuns", "1 álbum", "2 álbuns"] }],
  ]) {
    const instance = i18next.createInstance();
    await instance.init({ lng, resources, fallbackLng: false });
    for (const [key, forms] of Object.entries(expected)) forms.forEach((text, count) => assert.equal(instance.t(key, { count }), text));
    assert.equal(instance.t("plex.decadeShort", { decade: 1990 }), lng === "en" ? "1990s" : "1990");
    assert.equal(instance.t("localBrowser.decadeShort", { decade: 1990 }), lng === "en" ? "1990s" : "1990");
    assert.equal(instance.t("nowPlaying.analyzerReason.unknown", { defaultValue: instance.t("nowPlaying.vuUnavailable") }), lng === "en" ? "Inactive" : "Inativo");
  }
});
