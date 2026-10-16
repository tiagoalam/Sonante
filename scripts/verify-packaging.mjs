import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
const read = path => readFileSync(path, "utf8");
const pkg = JSON.parse(read("package.json"));
const lock = JSON.parse(read("package-lock.json"));
const tauri = JSON.parse(read("src-tauri/tauri.conf.json"));
const cargo = /^version = "([^"]+)"/m.exec(read("src-tauri/Cargo.toml"))?.[1];
const cargoLock = /name = "sonante"\nversion = "([^"]+)"/.exec(read("src-tauri/Cargo.lock"))?.[1];
const arch = /^pkgver=(.+)$/m.exec(read("sonante-arch/PKGBUILD"))?.[1];
for (const version of [lock.version, lock.packages[""].version, tauri.version, cargo, cargoLock, arch]) assert.equal(version, pkg.version);
if (process.env.RELEASE_TAG) assert.equal(process.env.RELEASE_TAG, `v${pkg.version}`);
assert.match(read("src/components/AboutModal.tsx"), new RegExp(`v${pkg.version.replaceAll(".", "\\.")}`));
// API/SONAME 4.1 alone does not guarantee the WebKit 2.40 APIs used by Tauri/Wry.
for (const dep of ["mpd", "alsa-utils", "xdg-utils", "xdg-desktop-portal", "libasound2", "libssl3"]) assert.ok(tauri.bundle.linux.deb.depends.includes(dep), dep);
// Tauri CLI 2.12.0 emits Dependency::any: only RPM's parenthesized rich
// dependency grammar interprets the comparison in the name field.
for (const dep of ["mpd", "alsa-utils", "xdg-utils", "xdg-desktop-portal", "alsa-lib", "(webkit2gtk4.1 >= 2.40)", "openssl-libs"]) assert.ok(tauri.bundle.linux.rpm.depends.includes(dep), dep);
assert.ok(!tauri.bundle.linux.rpm.depends.includes("webkit2gtk4.1 >= 2.40"));
assert.ok(tauri.bundle.linux.deb.depends.includes("libwebkit2gtk-4.1-0 (>= 2.40)"));
assert.ok(!tauri.bundle.linux.deb.depends.includes("wireplumber"));
assert.ok(!tauri.bundle.linux.rpm.depends.includes("wireplumber"));
assert.deepEqual(tauri.bundle.targets, ["deb", "rpm"]);
assert.equal(pkg.license, "GPL-3.0-only");
assert.equal(lock.packages[""].license, pkg.license);
assert.match(read("src-tauri/Cargo.toml"), /^license = "GPL-3.0-only"$/m);
assert.equal(tauri.bundle.license, pkg.license);
assert.equal(tauri.bundle.licenseFile, "../LICENSE");
assert.match(read("sonante-arch/PKGBUILD"), /^license=\('GPL-3\.0-only'\)$/m);
assert.ok(read("LICENSE").includes("Version 3, 29 June 2007"));
for (const fmt of ["deb", "rpm"]) {
  for (const source of Object.values(tauri.bundle.linux[fmt].files)) read(`src-tauri/${source}`);
}
console.log(`Packaging versions and runtime contracts verified: ${pkg.version}`);
