import test from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import ts from "typescript";

const modal = readFileSync(new URL("../components/SettingsModal.tsx", import.meta.url), "utf8");
const app = readFileSync(new URL("../App.tsx", import.meta.url), "utf8");

test("only local-folder-only Save transfers ownership to App before closing", () => {
  assert.match(modal, /localFoldersOnlyChanged\(originalConfigRef\.current, config\)/);
  assert.match(modal, /if \(onStartLocalFoldersSave\(config\)\) onClose\(\)/);
  assert.match(modal, /if \(!onSaveStarted\(restartExpected\)\)/);
  assert.match(modal, /saving \? t\("settings\.savingSettings"\) : t\("settings\.save"\)/);
  assert.doesNotMatch(modal, /wizard\.starting/);
});

test("App keeps local Save pending and confirms success only after backend completion", () => {
  assert.match(app, /if \(configSaveInProgressRef\.current\) return false/);
  assert.match(app, /response\.status === "accepted"\) \{\s*queueClearFlowRef\.current\.accepted\(response\.operation\.id\);\s*monitor\.accepted\(response\.operation\);\s*\} else/);
  assert.doesNotMatch(app, /setConfig\(nextConfig\)/);
  assert.match(app, /operation\.phase === "completed"[\s\S]*?await configService\.getConfig\(\)[\s\S]*?setConfigSaveNotice\(operation\.coverage\?\.outcome === "partial" \? null : \{ kind: "success" \}\)/);
  assert.match(app, /monitorConfigSave\(configService\.getSaveOperation/);
  assert.match(app, /finally \{\s*if \(isCurrent\(\)\) finishConfigSave\(\)/);
  assert.match(app, /t\(localSavePhase === "preparing"/);
  assert.match(app, /onSaveFinished=\{finishConfigSave\}/);
  // Exercise the normal Preferences action, including its contextual destination.
  const tree = ts.createSourceFile("App.tsx", app, ts.ScriptTarget.Latest, true, ts.ScriptKind.TSX);
  const actions = [];
  const visit = node => {
    if (ts.isJsxSelfClosingElement(node) && node.tagName.getText(tree) === "SidebarNavItem"
      && node.attributes.properties.some(prop => prop.name?.getText(tree) === "diagnosticAction" && prop.initializer?.text === "settings")) {
      actions.push(node.attributes.properties.find(prop => prop.name?.getText(tree) === "onClick").initializer.expression);
    }
    ts.forEachChild(node, visit);
  };
  visit(tree);
  assert.equal(actions.length, 1, "the real sidebar Preferences action must exist exactly once");
  const js = ts.transpileModule(`const handler = ${actions[0].getText(tree)};`, {
    compilerOptions: { target: ts.ScriptTarget.ES2020 },
  }).outputText;
  for (const previousPage of ["local", "plex"]) {
    let page = previousPage, opened = false;
    const effects = [], busy = { current: true };
    const click = new Function("configSaveInProgressRef", "setSettingsInitialPage", "setShowSettings", `${js}; return handler;`)(
      busy, value => { page = value; effects.push(["page", value]); }, value => { opened = value; effects.push(["open", value]); },
    );
    click();
    assert.equal(page, previousPage); assert.equal(opened, false); assert.deepEqual(effects, [], "a pending Save fences navigation");
    busy.current = false;
    click();
    assert.equal(page, "general"); assert.equal(opened, true);
    assert.deepEqual(effects, [["page", "general"], ["open", true]], "a fresh normal opening chooses General before opening");
  }
});

test("Save status messages exist in pt-BR and en", () => {
  const expected = {
    "pt-BR": ["Atualizando biblioteca...", "Salvando configurações...", "Biblioteca atualizada.", "Falha ao atualizar a biblioteca."],
    en: ["Updating library...", "Saving settings...", "Library updated.", "Failed to update the library."],
  };
  for (const [locale, [updatingLibrary, savingSettings, libraryUpdateSucceeded, libraryUpdateFailed]] of Object.entries(expected)) {
    const { settings } = JSON.parse(readFileSync(new URL(`../locales/${locale}.json`, import.meta.url), "utf8"));
    assert.equal(settings.updatingLibrary, updatingLibrary);
    assert.equal(settings.savingSettings, savingSettings);
    assert.equal(settings.libraryUpdateSucceeded, libraryUpdateSucceeded);
    assert.equal(settings.libraryUpdateFailed, libraryUpdateFailed);
    assert.ok(settings.confirmedConfigUnavailable);
    assert.ok(settings.dismissStatus);
    for (const key of ["preparingLibrary", "validatingLibrary", "librarySaveUncertain", "librarySaveStatusUnavailable"]) assert.ok(settings[key]);
  }
});

test("modal transfers an accepted operation without reporting completion or releasing admission", () => {
  assert.match(modal, /response\.status === "accepted"[\s\S]*?transferred = true;[\s\S]*?onSaveAccepted\(response\.operation\);[\s\S]*?return;\s*\}\s*onSaved/);
  assert.match(modal, /if \(!transferred\) onSaveFinished\(\)/);
});

test("App localizes refusals, pending catalog and indexing failures without masking unrelated errors", async () => {
  assert.match(app, /reason === "LOCAL_SOURCE_FULL_RESCAN_REQUIRED" \? "fullRescan"/);
  assert.match(app, /reason === "LOCAL_SOURCE_TARGETED_PLAN_CHANGED" \? "planChanged"/);
  assert.match(app, /Boolean\(refusal\) && cleanup === "LOCAL_SOURCE_STAGING_CLEANUP_FAILED"/);
  assert.match(app, /t\("settings\.fullIndexRequired"\)/);
  assert.match(app, /t\("settings\.fullIndexNotStarted"\)/);
  assert.match(app, /t\("settings\.noChangesApplied"\)/);
  assert.match(app, /t\("settings\.stagingCleanupFailed"\)/);
  // Execute the real failure handler and notice title, as the Preferences action
  // above does, instead of pinning the spelling of the detail conditional.
  const tree = ts.createSourceFile("App.tsx", app, ts.ScriptTarget.Latest, true, ts.ScriptKind.TSX);
  let handler, title;
  const visit = node => {
    if (ts.isVariableDeclaration(node) && node.name.getText(tree) === "reportLocalFoldersSaveFailure") handler = node.initializer;
    if (ts.isJsxExpression(node) && node.expression && ts.isConditionalExpression(node.expression)
      && node.expression.condition.getText(tree) === 'configSavePhase === "local"') title = node.expression;
    ts.forEachChild(node, visit);
  };
  visit(tree);
  assert.ok(handler); assert.ok(title);
  const compile = expression => ts.transpileModule(`const value = ${expression.getText(tree)};`, {
    compilerOptions: { target: ts.ScriptTarget.ES2020 },
  }).outputText;
  for (const locale of ["pt-BR", "en"]) {
    const translations = JSON.parse(readFileSync(new URL(`../locales/${locale}.json`, import.meta.url), "utf8"));
    const { settings } = translations;
    const t = key => key.split('.').reduce((value, part) => value?.[part], translations);
    for (const key of ["fullIndexRequired", "libraryChangedBeforeSave", "noChangesApplied", "fullIndexNotStarted", "stagingCleanupFailed"]) {
      assert.ok(settings[key]);
    }
    for (const [error, refusal, detail, titleKey, uncertain = false] of [
      ["LOCAL_SOURCE_FULL_RESCAN_REQUIRED;LOCAL_SOURCE_STAGING_CLEANUP_FAILED", "fullRescan", undefined, "fullIndexRequired"],
      ["LOCAL_SOURCE_TARGETED_PLAN_CHANGED", "planChanged", undefined, "libraryChangedBeforeSave"],
      ["LOCAL_SOURCE_REMOVAL_QUEUE_CONFIRMATION_STALE", "queueChanged", undefined, "queueClearStale"],
      ["LOCAL_SOURCE_NEW_OVERLAP", "overlap", undefined, "localFolderOverlap"],
      ["QUEUE_BINDING_FIXTURE", "queueCheckRequired", undefined, "queueClearCheckRequired"],
      ["LOCAL_INDEX_COVERAGE_FAILED;private diagnostic", undefined, t("indexCoverage.failed"), "libraryUpdateFailed"],
      ["UNRELATED_FAILURE;keep diagnostic", undefined, "UNRELATED_FAILURE;keep diagnostic", "libraryUpdateFailed"],
      ["FIRST_RUN_CATALOG_PENDING: private diagnostic", undefined, undefined, "initialLibraryFailed"],
      ["FIRST_RUN_CATALOG_PENDING: private diagnostic", undefined, undefined, "initialLibraryFailed", true],
    ]) {
      let notice;
      const confirmed = { first_run: false }, confirmedWrites = [], blocked = [], logs = [];
      const report = new Function("formatConfigError", "queueClearBinding", "logError", "configService", "setConfig", "localAlbumCatalog", "setConfigSaveNotice", "t", `${compile(handler)}; return value;`)(
        String, value => value === "QUEUE_BINDING_FIXTURE", (...args) => logs.push(args), { getConfig: async () => confirmed },
        value => confirmedWrites.push(value), { setSaveBlocked: value => blocked.push(value) }, value => { notice = value; }, t,
      );
      await report(error, uncertain);
      assert.equal(notice.kind, "error"); assert.equal(notice.refusal, refusal);
      assert.equal(notice.detail, detail); assert.equal(notice.uncertain, uncertain);
      assert.equal(notice.initialPreparationFailed, titleKey === "initialLibraryFailed");
      assert.equal(notice.stagingCleanupFailed, error.endsWith("LOCAL_SOURCE_STAGING_CLEANUP_FAILED"));
      assert.equal(notice.reloadFailed, false); assert.deepEqual(confirmedWrites, [confirmed]);
      assert.deepEqual(blocked, uncertain ? [] : [false]); assert.equal(logs.length, 1);
      const renderedTitle = new Function("configSavePhase", "localSavePhase", "configSaveNotice", "t", `${compile(title)}; return value;`)(null, null, notice, t);
      assert.ok(settings[titleKey]); assert.equal(renderedTitle, settings[titleKey], locale+": "+error);
      assert.ok(!renderedTitle.includes("FIRST_RUN_") && !renderedTitle.includes("private diagnostic"));
    }
  }
});
