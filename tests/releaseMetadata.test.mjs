import test from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";

const read = path => readFileSync(new URL(`../${path}`, import.meta.url), "utf8");
const json = path => JSON.parse(read(path));

test("active product manifests and package recipe agree on Sonante 0.7.0", () => {
  assert.equal(json("package.json").version, "0.7.0");
  const npmLock = json("package-lock.json");
  assert.equal(npmLock.version, "0.7.0");
  assert.equal(npmLock.packages[""].version, "0.7.0");

  assert.match(read("src-tauri/Cargo.toml"), /^version = "0\.7\.0"$/m);
  assert.match(read("src-tauri/Cargo.lock"), /\[\[package\]\]\nname = "sonante"\nversion = "0\.7\.0"/);
  assert.equal(json("src-tauri/tauri.conf.json").version, "0.7.0");
  assert.match(read("sonante-arch/PKGBUILD"), /^pkgver=0\.7\.0$/m);
  assert.match(read("sonante-arch/PKGBUILD"), /_source_tag="v\$\{pkgver\}"/);
});

test("release docs describe an unpublished candidate and the migration limits", () => {
  const readme = read("README.md");
  const changelog = read("CHANGELOG.md");
  const releaseNotes = read("RELEASE-NOTES.md");
  for (const document of [readme, changelog, releaseNotes]) {
    assert.match(document, /0\.7\.0/);
    assert.match(document, /not published|not a public release|not yet published/i);
  }
  for (const document of [readme, releaseNotes]) {
    assert.match(document, /manual(?:ly)? import/i);
    assert.match(document, /queue is not automatically|old queue is not automatically/i);
    assert.match(document, /favorites?.{0,100}(exact|safe)|exact safe match/i);
  }
  assert.match(releaseNotes, /DSD64 mono\/stereo/i);
  assert.match(releaseNotes, /(?:converted to PCM|conversion to PCM)/i);
  assert.match(releaseNotes, /bit-perfect/i);
});
