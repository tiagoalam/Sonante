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

snapshot='2026/07/24'
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
XferCommand = /usr/bin/curl --fail --show-error --location --retry 4 --retry-connrefused --retry-delay 5 --retry-max-time 1200 --connect-timeout 45 --max-time 900 --output %o %u

[core]
Server = https://archive.archlinux.org/repos/${snapshot}/core/os/\$arch

[extra]
Server = https://archive.archlinux.org/repos/${snapshot}/extra/os/\$arch
EOF

echo "Installing signed Arch Archive snapshot ${snapshot} in the disposable CI container."
echo 'Package downloads use bounded connection/transfer timeouts and up to four retries. Pacman signature checks remain required.'

# The moving base-devel image may carry a parallel openssl-3 package. The
# pinned snapshot's openssl package owns the same .so.3 files, so remove only
# that known package before the signed snapshot transaction. In the current
# base image pacman/curl use OpenSSL 4 and remain runnable during the transition.
if pacman -Q openssl-3 >/dev/null 2>&1; then
  for library in /usr/lib/libssl.so.3 /usr/lib/libcrypto.so.3; do
    if ! owner="$(pacman -Qqo "$library" 2>/dev/null)"; then
      printf 'Cannot identify the owner of the conflicting OpenSSL 3 library: %s\n' "$library" >&2
      exit 1
    fi
    if [[ "$owner" != 'openssl-3' ]]; then
      printf 'Refusing to remove openssl-3 because %s is owned by %s.\n' "$library" "$owner" >&2
      exit 1
    fi
  done

  echo 'Removing only the conflicting openssl-3 package from this disposable container before snapshot synchronization.'
  pacman -Rdd --noconfirm openssl-3
  if pacman -Q openssl-3 >/dev/null 2>&1; then
    echo 'The conflicting openssl-3 package remains installed after removal.' >&2
    exit 1
  fi
  for library in /usr/lib/libssl.so.3 /usr/lib/libcrypto.so.3; do
    if [[ -e "$library" || -L "$library" ]]; then
      printf 'The conflicting OpenSSL 3 library remains after removing openssl-3: %s\n' "$library" >&2
      exit 1
    fi
  done
  pacman --version >/dev/null
  curl --version >/dev/null
fi

pacman --config "$config" -Syyuu --noconfirm --needed --ignore=archlinux-keyring \
  curl git nodejs npm rust python file binutils gtk3 webkit2gtk-4.1 \
  alsa-lib alsa-utils openssl xdg-utils xdg-desktop-portal desktop-file-utils

if foreign="$(pacman --config "$config" -Qm 2>&1)"; then
  :
else
  foreign_status=$?
  # pacman 7.1 may represent an empty -Qm result as status 1 with no output;
  # other releases print the explicit "no foreign packages found" message.
  if ((foreign_status == 1)) && { [[ -z "$foreign" ]] || [[ "$foreign" == *'no foreign packages found'* ]]; }; then
    foreign=''
  else
    printf 'Could not verify packages outside the pinned Arch snapshot (pacman status %s):\n%s\n' \
      "$foreign_status" "$foreign" >&2
    exit "$foreign_status"
  fi
fi
if [[ -n "$foreign" ]]; then
  printf 'Packages outside the pinned Arch snapshot remain installed:\n%s\n' "$foreign" >&2
  exit 1
fi

libc="$(getconf GNU_LIBC_VERSION)"
openssl_version="$(openssl version)"
printf 'Pinned Arch build snapshot: %s\n%s\n%s\n' \
  "$snapshot" "$(pacman -Q glibc openssl rust nodejs npm archlinux-keyring)" "$libc / $openssl_version"

[[ "$libc" == 'glibc 2.43' ]] || {
  echo "Pinned Arch snapshot does not provide the required GLIBC 2.43 build floor: $libc" >&2
  exit 1
}
[[ "$openssl_version" =~ ^OpenSSL\ 3\.6\. ]] || {
  echo "Pinned Arch snapshot must use the pinned OpenSSL 3.6 build line: $openssl_version" >&2
  exit 1
}

for library in /usr/lib/libssl.so.3 /usr/lib/libcrypto.so.3; do
  test -e "$library"
  readelf -d "$library" | grep -F "[$(basename "$library")]"
done
