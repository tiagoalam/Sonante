import test from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";

const rust = readFileSync(new URL("../src-tauri/src/lib.rs", import.meta.url), "utf8");
const wizard = readFileSync(new URL("../src/components/WelcomeWizard.tsx", import.meta.url), "utf8");
const settings = readFileSync(new URL("../src/components/SettingsModal.tsx", import.meta.url), "utf8");

test("directory picker uses the async rfd API and preserves path/null IPC contract", () => {
  assert.match(rust, /async fn pick_directory\(\) -> Option<String> \{\s*rfd::AsyncFileDialog::new\(\)/);
  assert.match(rust, /\.pick_folder\(\)\s*\.await\s*\.map\(\|p\| p\.path\(\)\.to_string_lossy\(\)\.to_string\(\)\)/);
});

for (const [name, source] of [["WelcomeWizard", wizard], ["SettingsModal", settings]]) {
  test(`${name} blocks duplicate pickers and restores its button on every outcome`, () => {
    assert.match(source, /if \([^\n]*pickingFolderRef\.current\) return;/);
    assert.match(source, /pickingFolderRef\.current = true;\s*setIsPickingFolder\(true\);/);
    assert.match(source, /const selected = await audioService\.pickDirectory\(\);\s*if \(selected\)/);
    assert.match(source, /catch \(err\) \{\s*logError\(/);
    assert.match(source, /finally \{\s*pickingFolderRef\.current = false;\s*setIsPickingFolder\(false\);/);
    assert.match(source, /onClick=\{handleAddFolder\}\s*disabled=\{isPickingFolder\}/);
    assert.match(source, /prev\.local_folders\.includes\(selected\) \? prev/);
  });
}
