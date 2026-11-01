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

const mixSaveProducer = readFileSync(new URL("utils/plexDiscover.ts", src), "utf8");
// Follow actual mixSave publications, including resets. Unknown values fail closed;
// a new literal key is collected and must resolve in both catalogs below.
function mixSaveErrorKeys(source) {
  const tree = ts.createSourceFile("plexDiscover.ts", source, ts.ScriptTarget.Latest, true, ts.ScriptKind.TS);
  const keys = new Set();
  let resetFound = false, saveFound = false, errors = 0;
  function finite(node, bindings = new Map()) {
    if (ts.isStringLiteral(node)) keys.add(node.text);
    else if (node.kind === ts.SyntaxKind.NullKeyword) return;
    else if (ts.isParenthesizedExpression(node)) finite(node.expression, bindings);
    else if (ts.isIdentifier(node) && bindings.has(node.text)) keys.add(bindings.get(node.text));
    else if (ts.isConditionalExpression(node)) {
      const narrowed = new Map(bindings), condition = node.condition;
      if (ts.isBinaryExpression(condition) && condition.operatorToken.kind === ts.SyntaxKind.EqualsEqualsEqualsToken
        && ts.isIdentifier(condition.left) && ts.isStringLiteral(condition.right)) {
        narrowed.set(condition.left.text, condition.right.text);
      }
      finite(node.whenTrue, narrowed);
      finite(node.whenFalse, bindings);
    } else assert.fail(`uncovered mixSave.error producer ${node.getText(tree)}`);
  }
  function errorProperties(object) {
    assert.ok(ts.isObjectLiteralExpression(object), "uncovered mixSave publication");
    for (const property of object.properties) {
      if (ts.isSpreadAssignment(property)) {
        assert.ok(["this.state.mixSave", "emptyMixSave()"].includes(property.expression.getText(tree)), "uncovered mixSave spread producer");
      } else if (property.name?.getText(tree) === "error") {
        assert.ok(ts.isPropertyAssignment(property), "uncovered mixSave.error producer");
        errors++; finite(property.initializer);
      }
    }
  }
  function visit(node) {
    if (ts.isMethodDeclaration(node) && node.name.getText(tree) === "saveMix") saveFound = true;
    if (ts.isVariableDeclaration(node) && node.name.getText(tree) === "emptyMixSave") {
      assert.ok(ts.isArrowFunction(node.initializer));
      const body = node.initializer.body;
      assert.ok(ts.isParenthesizedExpression(body));
      errorProperties(body.expression); resetFound = true;
    }
    if (ts.isPropertyAssignment(node) && node.name.getText(tree) === "mixSave") {
      if (ts.isCallExpression(node.initializer) && node.initializer.expression.getText(tree) === "emptyMixSave") {
        assert.equal(node.initializer.arguments.length, 0);
      } else errorProperties(node.initializer);
    }
    ts.forEachChild(node, visit);
  }
  visit(tree);
  assert.ok(resetFound && saveFound && errors > 0, "mixSave producer/reset must remain inspectable");
  assert.ok(keys.size > 0);
  return keys;
}

// Follow finite return values from the actual mute-label producer. Unknown
// returns remain a scanner failure rather than accepting arbitrary dynamic keys.
const muteLabelProducer = readFileSync(new URL("utils/signalPath.ts", src), "utf8");
function muteLabelKeys(source) {
  const tree = ts.createSourceFile("signalPath.ts", source, ts.ScriptTarget.Latest, true, ts.ScriptKind.TS);
  const producer = tree.statements.find(node => ts.isFunctionDeclaration(node) && node.name?.text === "muteLabel");
  assert.ok(producer?.body, "muteLabel producer must remain inspectable");
  const keys = new Set();
  function finite(node) {
    if (ts.isStringLiteral(node)) keys.add(node.text);
    else if (ts.isConditionalExpression(node)) { finite(node.whenTrue); finite(node.whenFalse); }
    else if (ts.isParenthesizedExpression(node)) finite(node.expression);
    else assert.fail(`uncovered muteLabel producer ${node.getText(tree)}`);
  }
  function visit(node) {
    if (ts.isReturnStatement(node)) { assert.ok(node.expression); finite(node.expression); }
    else ts.forEachChild(node, visit);
  }
  visit(producer.body);
  assert.ok(keys.size > 0);
  return keys;
}

const catalogErrorProducer = readFileSync(new URL("utils/localAlbumCatalog.ts", src), "utf8");
function catalogErrorKeys(source) {
  const tree = ts.createSourceFile("localAlbumCatalog.ts", source, ts.ScriptTarget.Latest, true, ts.ScriptKind.TS);
  const producer = tree.statements.find(node => ts.isFunctionDeclaration(node) && node.name?.text === "localCatalogErrorKey");
  assert.ok(producer?.body, "catalog error producer must remain inspectable");
  const keys = new Set();
  function finite(node) {
    if (ts.isStringLiteral(node)) keys.add(node.text);
    else if (node.kind === ts.SyntaxKind.NullKeyword) return;
    else if (ts.isConditionalExpression(node)) { finite(node.whenTrue); finite(node.whenFalse); }
    else if (ts.isParenthesizedExpression(node)) finite(node.expression);
    else assert.fail(`uncovered catalog error producer ${node.getText(tree)}`);
  }
  function visit(node) {
    if (ts.isReturnStatement(node)) { assert.ok(node.expression); finite(node.expression); }
    else ts.forEachChild(node, visit);
  }
  visit(producer.body);
  assert.ok(keys.size > 0);
  return keys;
}

function translationCollector(tree, file, keys, mixSaveKeys) {
  const declarations = new Map();
  function declarationsVisit(node) {
    if (ts.isVariableDeclaration(node) && ts.isIdentifier(node.name) && node.initializer) declarations.set(node.name.text, node.initializer);
    ts.forEachChild(node, declarationsVisit);
  }
  declarationsVisit(tree);
  const escape = value => value.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
  function concatenationPatterns(node) {
    if (ts.isStringLiteral(node)) return [escape(node.text)];
    if (ts.isIdentifier(node) && declarations.has(node.text)) return concatenationPatterns(declarations.get(node.text));
    if (ts.isParenthesizedExpression(node)) return concatenationPatterns(node.expression);
    if (ts.isConditionalExpression(node)) return [...concatenationPatterns(node.whenTrue), ...concatenationPatterns(node.whenFalse)];
    if (ts.isBinaryExpression(node) && node.operatorToken.kind === ts.SyntaxKind.PlusToken) {
      return concatenationPatterns(node.left).flatMap(left => concatenationPatterns(node.right).map(right => left + right));
    }
    if (file === "components/LocalSourceAvailabilityPanel.tsx" && ts.isPropertyAccessExpression(node)
      && ["row.status", "row.reason"].includes(node.getText(tree))) return [".+"];
    assert.fail(`${file}: uncovered translation concatenation ${node.getText(tree)}`);
  }
  function collect(node, guardedNullable = false) {
    if (ts.isStringLiteral(node)) keys.add(node.text);
    else if (node.kind === ts.SyntaxKind.NullKeyword && guardedNullable) return;
    else if (ts.isConditionalExpression(node)) { collect(node.whenTrue, guardedNullable); collect(node.whenFalse, guardedNullable); }
    else if (ts.isParenthesizedExpression(node)) collect(node.expression, guardedNullable);
    else if (ts.isIdentifier(node) && declarations.has(node.text)) {
      const call = node.parent, branch = call?.parent;
      const guarded = ts.isCallExpression(call) && ts.isConditionalExpression(branch)
        && branch.whenTrue === call && ts.isIdentifier(branch.condition) && branch.condition.text === node.text;
      collect(declarations.get(node.text), guardedNullable || guarded);
    }
    else if (["components/PlayerBar.tsx", "components/NowPlayingPresentation.tsx"].includes(file)
      && ts.isCallExpression(node) && node.getText(tree) === "muteLabel(status)") {
      const imported = tree.statements.some(statement => ts.isImportDeclaration(statement)
        && statement.moduleSpecifier.text === "../utils/signalPath"
        && statement.importClause?.namedBindings && ts.isNamedImports(statement.importClause.namedBindings)
        && statement.importClause.namedBindings.elements.some(item => item.name.text === "muteLabel"
          && (item.propertyName?.text ?? item.name.text) === "muteLabel"));
      assert.ok(imported, `${file}: muteLabel must bind to its inspected producer`);
      muteLabelKeys(muteLabelProducer).forEach(key => keys.add(key));
    }
    else if (["components/LocalBrowserView.tsx", "components/LocalCatalogReadError.tsx"].includes(file) && ts.isCallExpression(node)
      && node.getText(tree) === "localCatalogErrorKey(catalog.error)") {
      const imported = tree.statements.some(statement => ts.isImportDeclaration(statement)
        && statement.moduleSpecifier.text === "../utils/localAlbumCatalog"
        && statement.importClause?.namedBindings && ts.isNamedImports(statement.importClause.namedBindings)
        && statement.importClause.namedBindings.elements.some(item => item.name.text === "localCatalogErrorKey"
          && (item.propertyName?.text ?? item.name.text) === "localCatalogErrorKey"));
      assert.ok(imported, "catalog error key must bind to its inspected producer");
      assert.ok(guardedNullable, "nullable catalog error key must be guarded before translation");
      catalogErrorKeys(catalogErrorProducer).forEach(key => keys.add(key));
    }
    else if (file === "components/PlexDiscoverView.tsx" && ts.isPropertyAccessExpression(node)
      && node.getText(tree) === "state.mixSave.error") mixSaveKeys.forEach(key => keys.add(key));
    else if (ts.isBinaryExpression(node) && node.operatorToken.kind === ts.SyntaxKind.PlusToken) {
      for (const expression of concatenationPatterns(node)) {
        const pattern = new RegExp(`^${expression}$`);
        const group = Object.keys(catalogs.en).filter(key => pattern.test(key));
        assert.ok(group.length > 0, `${file}: ${node.getText(tree)}`);
        group.forEach(key => keys.add(key));
      }
    }
    else if (ts.isTemplateExpression(node)) {
      const literals = [node.head.text, ...node.templateSpans.map(span => span.literal.text)];
      const pattern = new RegExp(`^${literals.map(escape).join(".+")}$`);
      const group = Object.keys(catalogs.en).filter(key => pattern.test(key));
      assert.ok(group.length > 0, `${file}: ${node.head.text}`);
      group.forEach(key => keys.add(key));
    } else assert.fail(`${file}: uncovered translation expression ${node.getText(tree)}`);
  }
  return collect;
}

test("catalog error producer keys remain finite, guarded and bound to the real import", async () => {
  const keys = catalogErrorKeys(catalogErrorProducer);
  assert.deepEqual([...keys].sort(), ["localBrowser.catalogReadUnavailable", "localBrowser.catalogUnsupportedFormat"]);
  await assertTranslationKeys(keys);
  const invalid = catalogErrorProducer.replace('"localBrowser.catalogUnsupportedFormat";', '"localBrowser.invalidFixtureKey";');
  await assert.rejects(assertTranslationKeys(catalogErrorKeys(invalid)), /en: localBrowser.invalidFixtureKey/);
  const unknown = catalogErrorProducer.replace('return "localBrowser.catalogUnsupportedFormat";', 'return unsafe.key;');
  assert.throws(() => catalogErrorKeys(unknown), /uncovered catalog error producer/);
  const file = "components/LocalBrowserView.tsx";
  const source = 'import {localCatalogErrorKey} from "../utils/localAlbumCatalog"; const key = catalog.error ? localCatalogErrorKey(catalog.error) : null; key ? t(key) : catalog.error;';
  const tree = ts.createSourceFile(file, source, ts.ScriptTarget.Latest, true, ts.ScriptKind.TSX);
  const collected = new Set();
  translationCollector(tree, file, collected, new Set())(tree.statements[2].expression.whenTrue.arguments[0]);
  assert.deepEqual([...collected].sort(), [...keys].sort());
  const unguarded = ts.createSourceFile(file, source.replace('key ? t(key) : catalog.error;', 't(key);'), ts.ScriptTarget.Latest, true, ts.ScriptKind.TSX);
  assert.throws(() => translationCollector(unguarded, file, new Set(), new Set())(unguarded.statements[2].expression.arguments[0]), /nullable catalog error key must be guarded/);
  const wrong = ts.createSourceFile(file, source.replace('../utils/localAlbumCatalog', '../other'), ts.ScriptTarget.Latest, true, ts.ScriptKind.TSX);
  assert.throws(() => translationCollector(wrong, file, new Set(), new Set())(wrong.statements[2].expression.whenTrue.arguments[0]), /must bind to its inspected producer/);
});

test("translation scanner covers nested diagnostic templates and finite availability concatenations", () => {
  const file = "components/LibraryRecoveryPanel.tsx";
  const tree = ts.createSourceFile(file, 't(`libraryRecovery.${group}.${value}`);', ts.ScriptTarget.Latest, true, ts.ScriptKind.TSX);
  const keys = new Set();
  translationCollector(tree, file, keys, new Set())(tree.statements[0].expression.arguments[0]);
  assert.ok(keys.has("libraryRecovery.diagnostic.outcome.failed"));
  const availabilityFile = "components/LocalSourceAvailabilityPanel.tsx";
  const source = 'const key = "localBrowser.sourceAvailability."; t(key + "status." + row.status);';
  const availability = ts.createSourceFile(availabilityFile, source, ts.ScriptTarget.Latest, true, ts.ScriptKind.TSX);
  const statuses = new Set();
  translationCollector(availability, availabilityFile, statuses, new Set())(availability.statements[1].expression.arguments[0]);
  assert.deepEqual([...statuses].sort(), ["localBrowser.sourceAvailability.status.available", "localBrowser.sourceAvailability.status.identity_conflict", "localBrowser.sourceAvailability.status.unavailable"]);
  const unsafe = ts.createSourceFile(availabilityFile, source.replace('row.status', 'row.unsafe'), ts.ScriptTarget.Latest, true, ts.ScriptKind.TSX);
  assert.throws(() => translationCollector(unsafe, availabilityFile, new Set(), new Set())(unsafe.statements[1].expression.arguments[0]), /uncovered translation concatenation/);
});

async function assertTranslationKeys(keys) {
  for (const lng of ["en", "pt-BR"]) {
    const instance = i18next.createInstance();
    await instance.init({ lng, resources, fallbackLng: false });
    for (const key of keys) for (const count of [0, 1, 2]) {
      assert.equal(instance.exists(key, { count }), true, `${lng}: ${key}`);
      assert.notEqual(instance.t(key, { count }), key, `${lng}: ${key}`);
    }
  }
}

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
const technicalText = new Set(["Radio Browser", "SONANTE", "Sonante", `v${JSON.parse(readFileSync(new URL("../../package.json", import.meta.url), "utf8")).version}`, "Linux • MPD • ALSA", "Português (Brasil)", "English (US)", "PT", "EN", "hw:CARD,DEV • ALSA Direct", "PipeWire / PulseAudio / dmix", "PipeWire / PulseAudio", "4 MB", "8 MB", "16 MB", "32 MB"]);

test("frontend translation calls resolve, including conditional and dynamic key groups", async () => {
  const keys = new Set(), untranslated = [];
  const mixSaveKeys = mixSaveErrorKeys(mixSaveProducer);
  for (const file of readdirSync(src, { recursive: true }).filter(file => file.endsWith(".tsx"))) {
    const tree = ts.createSourceFile(file, readFileSync(new URL(file, src), "utf8"), ts.ScriptTarget.Latest, true, ts.ScriptKind.TSX);
    const collect = translationCollector(tree, file, keys, mixSaveKeys);
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
  await assertTranslationKeys(keys);
});

test("mix-save error keys come from the actual producer and resolve without fallback in both languages", async () => {
  const keys = mixSaveErrorKeys(mixSaveProducer);
  assert.deepEqual([...keys].sort(), ["playlists.createAndAddError", "playlists.duplicateName", "playlists.nameRequired"]);
  await assertTranslationKeys(keys);
});

test("scanner rejects an invalid producer key and unrecognized dynamic producer/consumer expressions", async () => {
  const invalid = mixSaveProducer.replace('error: "playlists.nameRequired"', 'error: "playlists.invalidFixtureKey"');
  assert.notEqual(invalid, mixSaveProducer);
  const file = "components/PlexDiscoverView.tsx";
  const consumer = ts.createSourceFile(file, "t(state.mixSave.error)", ts.ScriptTarget.Latest, true, ts.ScriptKind.TSX);
  const keys = new Set();
  translationCollector(consumer, file, keys, mixSaveErrorKeys(invalid))(consumer.statements[0].expression.arguments[0]);
  assert.ok(keys.has("playlists.invalidFixtureKey"), "new producer values must reach locale validation");
  await assert.rejects(assertTranslationKeys(keys), /en: playlists\.invalidFixtureKey/);
  const dynamic = mixSaveProducer.replace('error: "playlists.nameRequired"', 'error: unknownError.key');
  assert.throws(() => mixSaveErrorKeys(dynamic), /uncovered mixSave\.error producer unknownError\.key/);
  for (const expression of ["state.otherError", "unknownError.key", "lookupError()", "state.mixSave.error"]) {
    const file = expression === "state.mixSave.error" ? "components/OtherView.tsx" : "components/PlexDiscoverView.tsx";
    const tree = ts.createSourceFile(file, `t(${expression})`, ts.ScriptTarget.Latest, true, ts.ScriptKind.TSX);
    const collect = translationCollector(tree, file, new Set(), mixSaveErrorKeys(mixSaveProducer));
    assert.throws(() => collect(tree.statements[0].expression.arguments[0]), /uncovered translation expression/);
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

test("Settings and wizard cannot translate unconstrained error strings", () => {
  for (const [file, expression] of [
    ["components/SettingsModal.tsx", "initialSaveError"],
    ["components/WelcomeWizard.tsx", "completionError"],
  ]) {
    const tree = ts.createSourceFile(file, `t(${expression})`, ts.ScriptTarget.Latest, true, ts.ScriptKind.TSX);
    const collect = translationCollector(tree, file, new Set(), mixSaveErrorKeys(mixSaveProducer));
    assert.throws(() => collect(tree.statements[0].expression.arguments[0]), /uncovered translation expression/);
  }
});

test("mute labels follow the actual producer and reject unknown keys, returns or consumer bindings", async () => {
  const keys = muteLabelKeys(muteLabelProducer);
  assert.deepEqual([...keys].sort(), ["player.mute", "player.unmute"]);
  await assertTranslationKeys(keys);
  const invalid = muteLabelProducer.replace('"player.unmute"', '"player.invalidFixtureMuteKey"');
  assert.notEqual(invalid, muteLabelProducer);
  await assert.rejects(assertTranslationKeys(muteLabelKeys(invalid)), /en: player\.invalidFixtureMuteKey/);
  const unknown = muteLabelProducer.replace('"player.unmute"', 'unknownLabel.key');
  assert.throws(() => muteLabelKeys(unknown), /uncovered muteLabel producer unknownLabel\.key/);
  for (const expression of ["muteLabel(other)", "muteLabel()", "unknownLabel(status)"]) {
    const file = "components/PlayerBar.tsx";
    const tree = ts.createSourceFile(file, `import { muteLabel } from "../utils/signalPath"; t(${expression})`, ts.ScriptTarget.Latest, true, ts.ScriptKind.TSX);
    const collect = translationCollector(tree, file, new Set(), new Set());
    assert.throws(() => collect(tree.statements[1].expression.arguments[0]), /uncovered translation expression/);
  }
  const file = "components/PlayerBar.tsx";
  const wrongImport = ts.createSourceFile(file, 'import { muteLabel } from "../other"; t(muteLabel(status))', ts.ScriptTarget.Latest, true, ts.ScriptKind.TSX);
  assert.throws(() => translationCollector(wrongImport, file, new Set(), new Set())(wrongImport.statements[1].expression.arguments[0]), /muteLabel must bind/);
});
