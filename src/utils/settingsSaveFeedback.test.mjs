import test from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";

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
  assert.match(app, /operation\.phase === "completed"[\s\S]*?await configService\.getConfig\(\)[\s\S]*?setConfigSaveNotice\(\{ kind: "success" \}\)/);
  assert.match(app, /monitorConfigSave\(configService\.getSaveOperation/);
  assert.match(app, /setConfigSaveNotice\(\{[\s\S]*?kind: "error",[\s\S]*?detail: refusal \? undefined : detail,[\s\S]*?reloadFailed,[\s\S]*?refusal,/);
  assert.match(app, /finally \{\s*if \(isCurrent\(\)\) finishConfigSave\(\)/);
  assert.match(app, /t\(localSavePhase === "preparing"/);
  assert.match(app, /onSaveFinished=\{finishConfigSave\}/);
  assert.match(app, /onClick=\{\(\) => \{ if \(!configSaveInProgressRef\.current\) setShowSettings\(true\); \}\}/);
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

test("App localizes full-rescan refusal and staging cleanup failure without masking unrelated errors", () => {
  assert.match(app, /reason === "LOCAL_SOURCE_FULL_RESCAN_REQUIRED" \? "fullRescan"/);
  assert.match(app, /reason === "LOCAL_SOURCE_TARGETED_PLAN_CHANGED" \? "planChanged"/);
  assert.match(app, /Boolean\(refusal\) && cleanup === "LOCAL_SOURCE_STAGING_CLEANUP_FAILED"/);
  assert.match(app, /detail: refusal \? undefined : detail/);
  assert.match(app, /t\("settings\.fullIndexRequired"\)/);
  assert.match(app, /t\("settings\.fullIndexNotStarted"\)/);
  assert.match(app, /t\("settings\.noChangesApplied"\)/);
  assert.match(app, /t\("settings\.stagingCleanupFailed"\)/);
  for (const locale of ["pt-BR", "en"]) {
    const { settings } = JSON.parse(readFileSync(new URL(`../locales/${locale}.json`, import.meta.url), "utf8"));
    for (const key of ["fullIndexRequired", "libraryChangedBeforeSave", "noChangesApplied", "fullIndexNotStarted", "stagingCleanupFailed"]) {
      assert.ok(settings[key]);
    }
  }
});
