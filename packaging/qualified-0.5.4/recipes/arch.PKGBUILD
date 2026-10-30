# Local private inputs only; generated checksums identify the exact snapshot.
# No public Git tag/branch download. Generate inputs with prepare-private-arch.py.
pkgname=sonante-private
pkgver=0.5.4
pkgrel=20261020.1
pkgdesc='PRIVATE TEST 20261020.1 - Sonante 0.5.4 with pinned runtimes'
arch=('x86_64')
license=('GPL-3.0-only' 'GPL-2.0-or-later' 'MPL-2.0')
url='https://github.com/tiagoalam/Sonante'
provides=('sonante=0.5.4')
conflicts=('sonante' 'sonante-bin')
options=('!debug' '!strip') # Runtime identities must not change during packaging.
depends=(alsa-lib alsa-utils aom audiofile avahi blas brotli bzip2 cairo chromaprint dbus expat faad2 fluidsynth fontconfig freetype2 fribidi 'gcc-libs>=12' glib2 'glibc>=2.35' gnutls gsm gtk3 krb5 lame lapack libao libass libbs2b libcdio-paranoia libdrm libgcc libgcrypt libgme libgomp libgpg-error libid3tag libidn2 libjack.so=0-64 libmad libmikmod libmms libmodplug libmpcdec libmpdclient libmysofa libnghttp2 libogg libopenmpt libpipewire-0.3.so=0-64 libpsl libpulse librsvg libsamplerate libshout libsndfile libsoxr libssh2 libstdc++ libva libvdpau libvorbis libwebp libx11 lilv mpg123 ocl-icd openal openjpeg2 openssl opus pcre2 pipewire snappy sndio speex sqlite systemd-libs twolame wavpack 'webkit2gtk-4.1>=2.40' wildmidi wireplumber xdg-desktop-portal xdg-utils xvidcore xz zeromq zimg zlib zstd zvbi zziplib)
makedepends=('python' 'binutils')
source=('sonante-private-source.tar' 'sonante-private-runtimes.tar' 'runtime-source-materials-consumer-clock.tar.zst' 'sonante-private-application' 'delivery-gates.py')
noextract=('runtime-source-materials-consumer-clock.tar.zst' 'sonante-private-application') # Corresponding sources travel alongside; never install this archive.
sha256sums=('a68c11341fe8c9bfda5f3187b206a899016eb490dc09915eeb3ce00cf620252a' '699cfa0d11cc15e0f7a865b3a2db96ef81147e8b4bf866a2c7d7ed0337fcfed6' '0b0f8a91f965711398a36b912b7ae8d193c1230622fd14d67f8c3a8c5e503af6' '7baab05a17d88f364068afbcbef3d0463297e603d0dac925cc4f48a4497bae6a' 'fe7d49592dd72b55cac1d96d7da9fb13472c35386da6bacfd42895cdac1e8887')

# Application is the exact Ubuntu 22.04 release/custom-protocol build supplied
# alongside the exact source snapshot. Do not rebuild on the Manjaro host:
# that would silently raise the application's GLIBC requirement.

package() {
    cd "$srcdir/sonante-private-source"
    test ! -e "$srcdir/sonante-private-runtimes/DISTRIBUTION_BLOCKED"
    python3 scripts/verify-private-runtime-stage.py "$srcdir/sonante-private-runtimes"
    install -Dm755 "$srcdir/sonante-private-application" "$pkgdir/usr/bin/sonante"
    python3 - "$pkgdir/usr/bin/sonante" <<'PYABI'
import re,subprocess,sys
text=subprocess.check_output(['readelf','--version-info',sys.argv[1]],text=True)
versions=re.findall(r'\bGLIBC_(\d+(?:\.\d+)+)',text)
if max(tuple(map(int,v.split('.'))) for v in versions)>(2,35):raise SystemExit('Application exceeds GLIBC 2.35')
PYABI
    install -Dm644 sonante-arch/sonante.desktop "$pkgdir/usr/share/applications/sonante.desktop"
    sed -i 's/^Name=Sonante$/Name=Sonante (Private test 20261020.1)/' "$pkgdir/usr/share/applications/sonante.desktop"
    for size in 32 128; do
        install -Dm644 "src-tauri/icons/${size}x${size}.png" "$pkgdir/usr/share/icons/hicolor/${size}x${size}/apps/sonante.png"
    done
    install -Dm644 src-tauri/icons/128x128@2x.png "$pkgdir/usr/share/icons/hicolor/256x256/apps/sonante.png"
    install -Dm644 src-tauri/icons/icon.png "$pkgdir/usr/share/icons/hicolor/512x512/apps/sonante.png"
    install -Dm644 LICENSE "$pkgdir/usr/share/licenses/sonante/LICENSE"
    install -dm755 "$pkgdir/usr/share/doc/sonante"
    install -Dm644 AUTHORS "$pkgdir/usr/share/doc/sonante/AUTHORS"
    install -Dm644 THIRD_PARTY_NOTICES "$pkgdir/usr/share/doc/sonante/THIRD_PARTY_NOTICES"
    cp -a --no-preserve=ownership "$srcdir/sonante-private-runtimes/usr/." "$pkgdir/usr/"
    python3 "$srcdir/delivery-gates.py" --app-only "$pkgdir/usr/bin/sonante"
    install -Dm644 "$srcdir/sonante-private-runtimes/usr/share/doc/sonante/PRIVATE_RUNTIME_SOURCES.txt" "$pkgdir/usr/share/doc/sonante/PRIVATE_RUNTIME_SOURCES.txt"
    printf '%s\n' 'PRIVATE TEST 20261020.1 - NOT A PUBLIC RELEASE' 'Source commit: 26d25d5ed7cfce59b234aaedc8070e2659e179f8; application SHA256: 7baab05a17d88f364068afbcbef3d0463297e603d0dac925cc4f48a4497bae6a' > "$pkgdir/usr/share/doc/sonante/PRIVATE_TEST_BUILD.txt"
}
