import test from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";

const read = path => readFileSync(new URL(`../${path}`, import.meta.url), "utf8");
const json = path => JSON.parse(read(path));

test("active product manifests and package recipe agree on Sonante 0.7.1", () => {
  assert.equal(json("package.json").version, "0.7.1");
  const npmLock = json("package-lock.json");
  assert.equal(npmLock.version, "0.7.1");
  assert.equal(npmLock.packages[""].version, "0.7.1");

  assert.match(read("src-tauri/Cargo.toml"), /^version = "0\.7\.1"$/m);
  assert.match(read("src-tauri/Cargo.lock"), /\[\[package\]\]\nname = "sonante"\nversion = "0\.7\.1"/);
  assert.equal(json("src-tauri/tauri.conf.json").version, "0.7.1");
  assert.match(read("sonante-arch/PKGBUILD"), /^pkgver=0\.7\.1$/m);
  assert.match(read("sonante-arch/PKGBUILD"), /_source_tag="v\$\{pkgver\}"/);
});

test("release docs identify 0.7.1 as the stable public release", () => {
  const readme = read("README.md");
  const changelog = read("CHANGELOG.md");
  const releaseNotes = read("RELEASE-NOTES.md");
  assert.match(readme, /Latest stable release: \[Sonante 0\.7\.1\]/);
  assert.match(changelog, /## 0\.7\.1/);
  assert.match(releaseNotes, /# Sonante 0\.7\.1/);
  assert.match(releaseNotes, /manual(?:ly)? import/i);
  assert.match(releaseNotes, /old queue is not automatically/i);
  assert.match(releaseNotes, /DSD64 mono\/stereo/i);
  assert.match(releaseNotes, /converted to PCM/i);
  assert.match(releaseNotes, /bit-perfect/i);
  assert.match(releaseNotes, /Physical audio and hardware validation was not performed/);
});
