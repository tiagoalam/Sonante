Qualified private installers: explicit profiles, external pinned inputs

Profiles: arch-20261020.1, ubuntu-20261020.2, fedora-20261020.3.
These identify the existing validated artifacts, not new package revisions.
The app/motors are NOT rebuilt by this tooling. This directory stores only text
recipes, descriptors, source/payload inventories and hashes. Binary/runtime/source
archives remain external. Toolchain and Cargo vendor inputs are not stored here.

The historical compiled source base is 26d25d5 (416 files), version0.5.4.
Ubuntu/Arch use that exact source and app7baab05a. Fedora uses the same base plus
one explicit private-runtimes.json override and appa7d6ce3c. Its two original
compatibility ABIs and system Nettle>=3.10 must not be transplanted into the
Ubuntu/Arch descriptor. Compiled descriptor bytes, runtime resources and app
identity are checked together. Current packaging-only commits do not rebuild
or relabel an old ELF as a new build.

From the repository root (replace every input path with its actual location):
python3 scripts/qualified-package.py verify --profile ubuntu-20261020.2 \
 --package /inputs/sonante-private_0.5.4-20261020.2_amd64.deb \
 --build-inputs /inputs/sonante-private-build-inputs-20261020.2.tar.zst \
 --source-sidecar /inputs/runtime-source-materials-consumer-clock.tar.zst

For Arch/Fedora use the corresponding profile and archive filenames/hashes in
profiles.json; Fedora additionally requires --compat-sources pointing to the
fedora-compat-source-materials-20261020.3.tar.zst archive.
Dependencies for inspection: Python3 standard library, GNU readelf, zstd,
dpkg-deb (DEB), rpm and bsdtar/libarchive (RPM). No app is executed or installed.
RPM queries use actual header requirements/flags, Provides and scriptlets.

Prepare exact recipe inputs into a NEW directory using the same --profile,
--build-inputs and --source-sidecar/--compat-sources arguments, replacing verify
and --package with prepare and --output /owned-build/new-inputs. Preparation
verifies inputs and the small app-source snapshot, then copies selected nested
archives; it does not expand engine source trees. Put the required sidecar(s)
beside the prepared recipe before packaging. No filesystem paths from the old
task directories are required by these committed recipes or the gate.

Arch: in prepared inputs run makepkg using the frozen PKGBUILD/local checksums.
Ubuntu: python3 repackage-deb.py original.deb OUTPUT.deb (metadata-only .2).
Fedora: python3 rebuild-payload.py reconstructs the174-file payload; in an owned
Fedora build root bind prepared inputs at /build and run rpmbuild --define
'_topdir /build/rpmbuild' -bb /build/sonante-private.spec.
No stripping, ABI symlinks, global private SONAME Provides or service scripts.
These recipes reproduce metadata/payload identities. Full package-container
byte reproduction is established for the DEB repack only, not asserted for
makepkg/rpmbuild headers, compression timestamps or native engine/toolchain builds.
A differently encoded package needs a new reviewed profile; do not edit hashes
to force an old qualification onto it.

A compilation artifact is not a qualified installer. The ordinary Tauri/CI
bundles currently omit pinned runtimes. inspect-packages.sh and existing CI
checks only establish compilation/legacy metadata contracts. They do not earn
qualified_content=true. The new verify command requires the actual complete
payload, approved profile, source materials, correct dependencies and build inputs.
No old CI artifact has become complete through this change. CI now discovers the new small qualification tests alongside existing packaging
tests; build/assembly/release behavior is unchanged and not triggered here.
CI/release still lack the explicit external-runtime
assembly plus this qualification gate before public distribution. Public release
must wait for that integration and its separate authorization; no new secret or
implicit download is assumed. Do not publish ordinary CI bundles as installers.

Validation scope: content/metadata/source/ABI gate. Prior isolated installation
and native playback proofs remain separate evidence. Arch initial operation and
Fedora43 VM acceptance are representative; automated Fedora qualification used44.
Ubuntu.2 installation is accepted; sustained playback on the slow VM is inconclusive.
Ubuntu physical test remains pending and does not block packaging consolidation.
No hardware/DAC certification, bit-perfect, audio migration or profile reset claim.
