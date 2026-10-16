import test from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { stripTypeScriptTypes } from "node:module";
import { createQueueClearConfirmationFlow } from "./queueClearConfirmation.ts";
import { wizardSaveErrorKey } from "./welcomeWizard.ts";

const app = readFileSync(new URL("../App.tsx", import.meta.url), "utf8");
const flush = async () => { for (let i = 0; i < 20; i++) await Promise.resolve(); };

function saveHarness(error) {
  const current = { first_run: false, local_folders: ["/music/reggae"], plex_token: "fixture" };
  const proposal = { ...current, local_folders: ["/music/reggae", "/music"] };
  const effects = { draft: null, opened: false, page: "general", notice: null, confirmed: null, finishes: 0, reads: 0, blocked: true };
  const monitor = { beginAcceptance() {}, acceptanceFailed() {}, accepted() { throw Error("must not accept"); } };
  const deps = {
    formatConfigError: value => value instanceof Error ? value.message : String(value),
    queueClearBinding: () => null, logError() {},
    configService: { async saveConfig() { throw error; }, async getConfig() { effects.reads++; return structuredClone(current); } },
    setConfig: value => { effects.confirmed = value; },
    localAlbumCatalog: { setSaveBlocked(value) { effects.blocked = value; } },
    setConfigSaveNotice: value => { effects.notice = value; },
    configSaveMonitorRef: { current: monitor }, beginConfigSave: () => true,
    configSaveGenerationRef: { current: 1 }, importPreviewRef: { current: null },
    queueClearFlowRef: { current: createQueueClearConfirmationFlow() },
    setQueueClearConfirmation() {}, setLocalSavePhase() {},
    setLocalFoldersDraft: value => { effects.draft = value; },
    setShowSettings: value => { effects.opened = value; },
    setSettingsInitialPage: value => { effects.page = value; },
    finishConfigSave() { effects.finishes++; },
  };
  const reportStart = app.indexOf("  const reportLocalFoldersSaveFailure");
  const report = app.slice(reportStart, app.indexOf("\n  useEffect", reportStart));
  const start = app.indexOf("  const startLocalFoldersSave");
  const handlers = app.slice(start, app.indexOf("\n  const cancelQueueClear", start));
  const run = new Function(...Object.keys(deps), stripTypeScriptTypes(report + handlers) + "\nreturn startLocalFoldersSave;")(...Object.values(deps));
  const click = marker => {
    const at = app.indexOf(marker);
    assert.ok(at >= 0);
    const handler = app.slice(at).match(/onClick=\{([^\n]+)\}/)[1];
    new Function(...Object.keys(deps), "configSaveInProgressRef", `return (${handler});`)(...Object.values(deps), { current: effects.busy ?? false })();
  };
  return { run, effects, proposal, current, click };
}

test("actual App Save rejection reopens Settings with an independent draft and overlap notice", async () => {
  const f = saveHarness("LOCAL_SOURCE_NEW_OVERLAP");
  assert.equal(f.run(f.proposal), true);
  await flush();
  assert.deepEqual(f.effects.draft, f.proposal);
  f.proposal.local_folders.length = 0;
  assert.deepEqual(f.effects.draft.local_folders, ["/music/reggae", "/music"]);
  assert.equal(f.effects.opened, true);
  assert.equal(f.effects.page, "local");
  assert.match(app, /initialPage=\{settingsInitialPage\}/);
  assert.equal(f.effects.notice.refusal, "overlap");
  assert.equal(f.effects.notice.detail, undefined);
  assert.deepEqual(f.effects.confirmed, f.current);
  assert.equal(f.effects.finishes, 1);
  assert.equal(f.effects.blocked, false);
  assert.match(app, /initialDraft=\{localFoldersDraft\}/);
  assert.match(app, /initialFolderOverlap=\{Boolean\(localFoldersDraft\)\}/);
});

test("unrelated Save errors keep their existing diagnostic and do not restore an overlap draft", async () => {
  const f = saveHarness("another failure"); f.run(f.proposal); await flush();
  assert.equal(f.effects.draft, null); assert.equal(f.effects.opened, false);
  assert.equal(f.effects.notice.refusal, undefined);
  assert.equal(f.effects.notice.detail, "another failure");
});

test("first setup recognizes only the exact overlap error and both locales explain correction", () => {
  assert.equal(wizardSaveErrorKey("LOCAL_SOURCE_NEW_OVERLAP"), "settings.localFolderOverlap");
  assert.equal(wizardSaveErrorKey(new Error("LOCAL_SOURCE_NEW_OVERLAP")), "settings.localFolderOverlap");
  assert.equal(wizardSaveErrorKey("prefix LOCAL_SOURCE_NEW_OVERLAP"), "wizard.saveError");
  for (const locale of ["en", "pt-BR"]) {
    const { settings } = JSON.parse(readFileSync(new URL(`../locales/${locale}.json`, import.meta.url), "utf8"));
    assert.ok(settings.localFolderOverlap.length > 50);
    assert.match(app, /refusal === "overlap" \? t\("settings.localFolderOverlap"\)/);
  }
});

test("actual Plex Connect and sidebar handlers choose each opening independently and retain busy guards", async () => {
  const f = saveHarness("LOCAL_SOURCE_NEW_OVERLAP");
  const plex = '<h2 className="text-xl font-bold text-white mb-2">{t("plex.disconnectedTitle")}';
  const normal = '<SidebarNavItem diagnosticAction="settings"';
  f.click(plex);
  assert.equal(f.effects.opened, true); assert.equal(f.effects.page, "plex");
  f.effects.opened = false;
  f.click(normal);
  assert.equal(f.effects.opened, true); assert.equal(f.effects.page, "general");
  f.run(f.proposal); await flush();
  assert.equal(f.effects.page, "local");
  f.effects.opened = false;
  f.click(normal);
  assert.equal(f.effects.opened, true); assert.equal(f.effects.page, "general");
  for (const marker of [plex, normal]) {
    f.effects.busy = true; f.effects.opened = false; f.effects.page = "audio";
    f.click(marker);
    assert.equal(f.effects.opened, false); assert.equal(f.effects.page, "audio");
  }
});
