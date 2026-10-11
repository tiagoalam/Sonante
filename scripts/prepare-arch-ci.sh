#!/usr/bin/env bash
# CI only: package the checked-out commit, without fetching a future release tag.
set -euo pipefail
stage="$1"
mkdir -m 700 "$stage"
git -c safe.directory="$(pwd -P)" archive --format=tar --prefix=sonante/ HEAD > "$stage/sonante-source.tar"
archive_version="$(tar -xOf "$stage/sonante-source.tar" sonante/package.json | node -pe 'JSON.parse(require("fs").readFileSync(0,"utf8")).version')"
checkout_version="$(node -p 'require("./package.json").version')"
if [[ "$archive_version" != "$checkout_version" ]]; then
  echo 'Committed source version differs from working tree; refusing mislabeled Arch package' >&2
  exit 1
fi
cp sonante-arch/PKGBUILD sonante-arch/sonante.desktop "$stage/"
# Preserve the recipe's build/package functions, override sources in the staged
# copy only, and checksum the exact archive and desktop entry used by makepkg.
cat >> "$stage/PKGBUILD" <<'RECIPE'
source=('sonante-source.tar' 'sonante.desktop')
RECIPE
printf 'sha256sums=("%s" "%s")\n' \
  "$(sha256sum "$stage/sonante-source.tar" | cut -d ' ' -f1)" \
  "$(sha256sum "$stage/sonante.desktop" | cut -d ' ' -f1)" >> "$stage/PKGBUILD"
