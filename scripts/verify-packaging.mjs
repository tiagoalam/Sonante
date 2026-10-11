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
// Tauri adds GTK 3 and WebKitGTK 4.1 to DEB requirements; inspect the result too.
for (const dep of ["mpd", "alsa-utils", "xdg-utils", "xdg-desktop-portal", "libasound2", "libssl3"]) assert.ok(tauri.bundle.linux.deb.depends.includes(dep), dep);
for (const dep of ["mpd", "alsa-utils", "xdg-utils", "xdg-desktop-portal", "alsa-lib", "webkit2gtk4.1", "openssl-libs"]) assert.ok(tauri.bundle.linux.rpm.depends.includes(dep), dep);
assert.ok(!tauri.bundle.linux.deb.depends.includes("wireplumber"));
assert.ok(!tauri.bundle.linux.rpm.depends.includes("wireplumber"));
assert.deepEqual(tauri.bundle.targets, ["deb", "rpm"]);
console.log(`Packaging versions and runtime contracts verified: ${pkg.version}`);
