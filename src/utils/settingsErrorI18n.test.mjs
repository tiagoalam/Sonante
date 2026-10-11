import test from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import i18next from "i18next";

const source = readFileSync(new URL("../components/SettingsModal.tsx", import.meta.url), "utf8");
const resources = Object.fromEntries(["en", "pt-BR"].map(locale => [locale, {
  translation: JSON.parse(readFileSync(new URL(`../locales/${locale}.json`, import.meta.url), "utf8")),
}]));
const messages = {
  en: ["Could not connect to the Plex servers.", "Failed to persist the configuration.",
    "Failed to change the output/configuration.", "Step reported by the backend:"],
  "pt-BR": ["Não foi possível conectar aos servidores do Plex.", "Falha ao persistir a configuração.",
    "Falha ao alterar a saída/configuração.", "Etapa reportada pelo backend:"],
};
const keys = ["plexConnectionError", "configPersistenceError", "outputConfigChangeError", "backendReportedStep"];

test("Settings error messages resolve in both languages without language fallback", async () => {
  for (const lng of ["en", "pt-BR"]) {
    const instance = i18next.createInstance();
    await instance.init({ lng, resources, fallbackLng: false });
    keys.forEach((key, index) => {
      assert.equal(instance.exists(`settings.${key}`), true);
      assert.equal(instance.t(`settings.${key}`), messages[lng][index]);
    });
  }
  assert.match(source, /alert\(t\("settings\.plexConnectionError"\)\)/);
});

test("Save retains its exact classification and diagnostic while translating only visible labels", async () => {
  const block = source.match(/const summary = diagnostic\.startsWith\("Falha ao persistir configuração:"\)[\s\S]*?alert\([^;]+\);/)?.[0];
  assert.ok(block);
  for (const lng of ["en", "pt-BR"]) {
    const instance = i18next.createInstance();
    await instance.init({ lng, resources, fallbackLng: false });
    for (const diagnostic of [
      "Falha ao persistir configuração: CONFIG_PERSISTENCE_UNCERTAIN: fixture",
      "Falha ao iniciar nova saída: fixture. Estado anterior restaurado.",
      "OTHER_CODE: fixture",
    ]) {
      let displayed;
      new Function("diagnostic", "t", "alert", block)(diagnostic, instance.t.bind(instance), value => { displayed = value; });
      const summary = messages[lng][diagnostic.startsWith("Falha ao persistir configuração:") ? 1 : 2];
      assert.equal(displayed, `${summary}\n\n${messages[lng][3]}\n${diagnostic}`);
    }
  }
  assert.match(source, /const diagnostic = formatConfigError\(err\)/);
  assert.match(source, /diagnostic === "LOCAL_FOLDERS_MIXED_CONFIG_CHANGE"[\s\S]*?alert\(t\("settings\.separateFolderChanges"\)\)/);
});
