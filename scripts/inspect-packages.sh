#!/usr/bin/env bash
set -euo pipefail
version="$1"
shopt -s nullglob
packages=(src-tauri/target/release/bundle/deb/*.deb src-tauri/target/release/bundle/rpm/*.rpm)
test "${#packages[@]}" = 2
for package in "${packages[@]}"; do
  case "$package" in
    *.deb)
      dpkg-deb -I "$package"
      dpkg-deb -c "$package"
      test "$(dpkg-deb -f "$package" Version)" = "$version"
      test "$(dpkg-deb -f "$package" Architecture)" = amd64
      contents="$(dpkg-deb --fsys-tarfile "$package" | tar -tf -)"
      depends="$(dpkg-deb -f "$package" Depends)"
      for dep in mpd alsa-utils xdg-utils xdg-desktop-portal libasound2 libwebkit2gtk-4.1-0 libssl3; do
        [[ "$depends" == *"$dep"* ]]
      done
      ;;
    *.rpm)
      rpm -K "$package"
      rpm -qip "$package"
      rpm -qlp "$package"
      test "$(rpm -qp --queryformat '%{VERSION}' "$package")" = "$version"
      test "$(rpm -qp --queryformat '%{ARCH}' "$package")" = x86_64
      contents="$(rpm -qlp "$package")"
      depends="$(rpm -qp --requires "$package")"
      for dep in mpd alsa-utils xdg-utils xdg-desktop-portal alsa-lib webkit2gtk4.1 openssl-libs 'libssl.so.3()(64bit)'; do
        [[ "$depends" == *"$dep"* ]]
      done
      ;;
  esac
  [[ "$contents" == *usr/bin/sonante* ]]
  [[ "$contents" == *share/applications/*.desktop* ]]
  [[ "$contents" == *share/icons/hicolor* ]]
  # No personal state, system MPD configuration or service shipped.
  if [[ "$contents" == *etc/mpd* || "$contents" == *queue_cache.json* || "$contents" == *systemd/system/mpd* ]]; then
    echo 'Unexpected state/service in package' >&2; exit 1
  fi
done
