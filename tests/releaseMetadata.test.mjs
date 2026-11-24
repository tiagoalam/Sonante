import test from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";

const read = path => readFileSync(new URL("../" + path, import.meta.url), "utf8");
const json = path => JSON.parse(read(path));

test("active product manifests and package recipe agree on Sonante 0.7.2", () => {
  assert.equal(json("package.json").version, "0.7.2");
  const npmLock = json("package-lock.json");
  assert.equal(npmLock.version, "0.7.2");
  assert.equal(npmLock.packages[""].version, "0.7.2");

  assert.ok(read("src-tauri/Cargo.toml").includes('version = "0.7.2"'));
  assert.ok(read("src-tauri/Cargo.lock").includes('name = "sonante"\nversion = "0.7.2"'));
  assert.equal(json("src-tauri/tauri.conf.json").version, "0.7.2");
  assert.ok(read("sonante-arch/PKGBUILD").includes("pkgver=0.7.2"));
  assert.ok(read("sonante-arch/PKGBUILD").includes("_source_tag="));
});

test("public release docs and CI identify 0.7.2", () => {
  const readme = read("README.md");
  const changelog = read("CHANGELOG.md");
  const releaseNotes = read("RELEASE-NOTES.md");
  const workflow = read(".github/workflows/ci.yml");
  assert.ok(readme.includes("Latest stable release: [Sonante 0.7.2]"));
  assert.ok(readme.includes("docs/releases/0.7.2/README.md"));
  assert.ok(changelog.indexOf("## 0.7.2") < changelog.indexOf("## 0.7.1"));
  assert.ok(releaseNotes.includes("# Sonante 0.7.2"));
  assert.ok(releaseNotes.includes("WavPack"));
  assert.ok(releaseNotes.includes("Subsonic/OpenSubsonic"));
  assert.ok(releaseNotes.includes("bit-perfect"));
  assert.ok(workflow.includes("sonante-0.7.2-candidate-"));
  assert.ok(!workflow.includes("sonante_0.7.1"));
  assert.ok(!workflow.includes("sonante-0.7.1"));
});
