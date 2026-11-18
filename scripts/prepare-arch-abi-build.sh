#!/usr/bin/env bash
# Install a signature-verified, reproducible Arch build snapshot that retains
# the last GLIBC 2.43 baseline. The date is intentionally fixed: moving it to a
# newer ABI is a reviewed compatibility change, never an automatic CI update.
set -euo pipefail
umask 077

if [[ "$(id -u)" -ne 0 ]]; then
  echo 'This CI-only setup must run as root inside its disposable Arch job container.' >&2
  exit 1
fi

snapshot='2026/07/26'
config="$(mktemp /tmp/sonante-pacman-snapshot.XXXXXXXX)"
trap 'rm -f -- "$config"' EXIT
cat >"$config" <<EOF
[options]
Architecture = x86_64
SigLevel = Required DatabaseOptional
LocalFileSigLevel = Optional
GPGDir = /etc/pacman.d/gnupg
DBPath = /var/lib/pacman/
CacheDir = /var/cache/pacman/pkg/
LogFile = /var/log/pacman.log

[core]
Server = https://archive.archlinux.org/repos/${snapshot}/core/os/\$arch

[extra]
Server = https://archive.archlinux.org/repos/${snapshot}/extra/os/\$arch
EOF

echo "Installing signed Arch Archive snapshot ${snapshot} in the disposable CI container."
pacman --config "$config" -Syyuu --noconfirm --needed \
  git nodejs npm rust python file binutils gtk3 webkit2gtk-4.1 \
  alsa-lib alsa-utils openssl xdg-utils xdg-desktop-portal desktop-file-utils

foreign="$(pacman --config "$config" -Qm)"
if [[ -n "$foreign" ]]; then
  printf 'Packages outside the pinned Arch snapshot remain installed:\n%s\n' "$foreign" >&2
  exit 1
fi

libc="$(getconf GNU_LIBC_VERSION)"
openssl_version="$(openssl version)"
printf 'Pinned Arch build snapshot: %s\n%s\n%s\n' \
  "$snapshot" "$(pacman -Q glibc openssl rust nodejs npm)" "$libc / $openssl_version"

[[ "$libc" == 'glibc 2.43' ]] || {
  echo "Pinned Arch snapshot does not provide the required GLIBC 2.43 build floor: $libc" >&2
  exit 1
}
[[ "$openssl_version" =~ ^OpenSSL\ 3\.5\. ]] || {
  echo "Pinned Arch snapshot must link against OpenSSL 3.5 for the current OpenSSL-3 compatibility ABI: $openssl_version" >&2
  exit 1
}

for library in /usr/lib/libssl.so.3 /usr/lib/libcrypto.so.3; do
  test -e "$library"
  readelf -d "$library" | grep -F "[$(basename "$library")]"
done
