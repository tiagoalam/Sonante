import test from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";

const read = path => readFileSync(new URL(`../src/components/${path}`, import.meta.url), "utf8");

test("all Settings tabs use shared section spacing and title hierarchy", () => {
  const pages = read("SettingsPages.tsx");
  const styles = read("settings.css");
  const modal = read("SettingsModal.tsx");

  assert.match(pages, /className="settings-page-content"/);
  assert.match(styles, /\.settings-section\s*\{[^}]*gap:\s*12px;[^}]*padding-block:\s*16px;[^}]*border-top:/s);
  assert.match(styles, /\.settings-section > legend,\s*\.settings-section-title\s*\{[^}]*font-size:\s*11px;[^}]*font-weight:\s*700;[^}]*text-transform:\s*uppercase/s);
  assert.match(styles, /@media\s*\(max-width:\s*520px\)/);
  assert.match(styles, /\.settings-page-content > \.settings-section:first-child/);
  for (const id of ["language", "audio-output", "audio-processing", "audio-performance", "local-libraries", "plex-connection"]) {
    assert.ok(modal.includes(`data-settings-section="${id}"`), `SettingsModal contains ${id}`);
  }
});

test("conditional network and experimental sections share the fieldset title pattern", () => {
  for (const [file, id] of [
    ["UsageModeSetting.tsx", "usage-mode"], ["AppearanceSetting.tsx", "appearance"],
    ["ExperimentalFeatures.tsx", "experimental"], ["UpnpSetting.tsx", "upnp"],
    ["RemoteControlSetting.tsx", "remote"], ["ScrobblingSetting.tsx", "scrobbling"],
    ["SqliteSourcesSettings.tsx", "sqlite-sources"],
  ]) {
    const source = read(file);
    assert.ok(source.includes(`data-settings-section="${id}"`), `${file} identifies ${id}`);
    assert.match(source, /className="settings-section"/);
    assert.match(source, /className="settings-section-title"/);
    assert.doesNotMatch(source, /border-t border-\[color:var\(--theme-border-soft,[^\]]+\)\] pt-4/);
  }
  assert.match(read("ScrobblingSetting.tsx"), /className="settings-subsection-title"/);
  assert.match(read("SourcesSettings.tsx"), /className="settings-subsection-title mb-3"/);
});

test("settings categories classify every existing group without a General page", () => {
  const pages = read("SettingsPages.tsx");
  const modal = read("SettingsModal.tsx");
  const ids = [...pages.matchAll(/id: "([a-z]+)" as const/g)].map(match => match[1]);
  assert.deepEqual(ids, ["interface", "appearance", "audio", "sources", "network", "experimental"]);
  assert.doesNotMatch(pages, /"general"|settings\.pages\.general/);

  for (const prop of ["interfacePage", "appearance", "audio", "sources", "network", "experimental"]) {
    assert.equal([...modal.matchAll(new RegExp(`^ {10}${prop}=\\{`, "gm"))].length, 1, `${prop} content is assigned once`);
    assert.match(pages, new RegExp(`content: ${prop}[, }]`), `${prop} has one matching page`);
  }
  assert.match(modal, /data-settings-section="language"[\s\S]*?<UsageModeSetting/);
  assert.match(modal, /appearance={<AppearanceSetting \/>}/);
  assert.match(modal, /experimental=\{fullModeEnabled\(config\)/);
  assert.match(modal, /network=\{<>[\s\S]*?<UpnpSetting[\s\S]*?<RemoteControlSetting[\s\S]*?<ScrobblingSetting/);
  assert.match(modal, /data-settings-section="audio-output"[\s\S]*?data-settings-section="audio-processing"[\s\S]*?data-settings-section="audio-performance"/);
  assert.match(modal, /sources={<SourcesSettings/);
  assert.match(pages, /overflow-x-auto sm:overflow-x-visible sm:w-52/);
  assert.match(pages, /onKeyDown=\{event =>/);

  for (const language of ["pt-BR", "en"]) {
    const locale = JSON.parse(readFileSync(new URL(`../src/locales/${language}.json`, import.meta.url), "utf8"));
    const labels = locale.settings.pages;
    assert.deepEqual(Object.keys(labels).filter(key => ["interface", "appearance", "audio", "sources", "network", "experimental"].includes(key)), ids);
    assert.equal(Object.hasOwn(labels, "general"), false);
    assert.ok(ids.every(id => typeof labels[id] === "string" && labels[id].length > 0), `${language} labels every category`);
  }
});
