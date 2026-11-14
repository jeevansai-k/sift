#!/usr/bin/env bash
# Build release artifacts for GitHub Releases:
#   dist/sift_<version>_<arch>.deb          Debian / Ubuntu / Pop!_OS package
#   dist/sift-<version>-linux-<arch>.tar.gz portable build with install.sh
#   dist/SHA256SUMS
# Usage: packaging/build-release.sh   (run from anywhere)
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"

VERSION="$(grep -m1 '^version' Cargo.toml | cut -d'"' -f2)"
DEB_ARCH="$(dpkg --print-architecture)"
TAR_ARCH="$(uname -m)"
DIST="$ROOT/dist"
STAGE="$ROOT/target/package"

echo "==> Building Sift $VERSION (release)"
cargo build --release
cargo test --workspace --quiet

rm -rf "$STAGE" "$DIST"
mkdir -p "$DIST"

# Lay out the files under a prefix ($1), shared by .deb and tarball.
stage_tree() {
  local root="$1" prefix="$2"
  install -Dm755 target/release/sift "$root$prefix/bin/sift"
  install -Dm755 target/release/sift-gui "$root$prefix/bin/sift-gui"
  install -Dm644 data/dev.sift.Sift.desktop "$root$prefix/share/applications/dev.sift.Sift.desktop"
  sed -i "s|^Exec=sift-gui|Exec=$prefix/bin/sift-gui|; s|^Icon=dev.sift.Sift|Icon=$prefix/share/icons/hicolor/256x256/apps/dev.sift.Sift.png|" \
    "$root$prefix/share/applications/dev.sift.Sift.desktop"
  (cd data/icons/hicolor && for f in */apps/dev.sift.Sift.*; do install -Dm644 "$f" "$root$prefix/share/icons/hicolor/$f"; done)
  install -Dm644 README.md "$root$prefix/share/doc/sift/README.md"
  install -Dm644 NOTICE "$root$prefix/share/doc/sift/NOTICE"
  install -Dm644 LICENSE "$root$prefix/share/doc/sift/LICENSE"
  for f in data/fonts/*-OFL.txt; do install -Dm644 "$f" "$root$prefix/share/doc/sift/fonts/$(basename "$f")"; done
}

# ---------- .deb
echo "==> Packaging .deb"
DEB_ROOT="$STAGE/deb"
stage_tree "$DEB_ROOT" /usr
sed "s|%h/.local/bin/sift|/usr/bin/sift|" data/sift-sampler.service \
  | install -Dm644 /dev/stdin "$DEB_ROOT/usr/lib/systemd/user/sift-sampler.service"
install -Dm644 LICENSE "$DEB_ROOT/usr/share/doc/sift/copyright"
SIZE_KB="$(du -sk "$DEB_ROOT/usr" | cut -f1)"
mkdir -p "$DEB_ROOT/DEBIAN"
cat > "$DEB_ROOT/DEBIAN/control" <<EOF
Package: sift
Version: $VERSION
Architecture: $DEB_ARCH
Maintainer: Sift contributors <https://github.com/jeevansai-k/sift>
Installed-Size: $SIZE_KB
Depends: libc6 (>= 2.35), libgtk-4-1 (>= 4.14), libadwaita-1-0 (>= 1.5), libfontconfig1
Recommends: pkexec | policykit-1
Section: utils
Priority: optional
Homepage: https://github.com/jeevansai-k/sift
Description: Linux system companion — clean, uninstall, analyze, optimize, monitor
 Sift is a Linux port of Burrow (macOS): junk and cache cleanup, project
 build-artifact purge, leftover installers, app uninstall with leftovers,
 disk treemap, duplicate finder, listening ports, health checks and a live
 dashboard with local history. Preview first, Trash by default.
EOF
cat > "$DEB_ROOT/DEBIAN/postinst" <<'EOF'
#!/bin/sh
set -e
command -v update-desktop-database >/dev/null && update-desktop-database -q /usr/share/applications || true
command -v gtk-update-icon-cache >/dev/null && gtk-update-icon-cache -q -t -f /usr/share/icons/hicolor || true
exit 0
EOF
cp "$DEB_ROOT/DEBIAN/postinst" "$DEB_ROOT/DEBIAN/postrm"
chmod 755 "$DEB_ROOT/DEBIAN/postinst" "$DEB_ROOT/DEBIAN/postrm"
dpkg-deb --root-owner-group --build "$DEB_ROOT" "$DIST/sift_${VERSION}_${DEB_ARCH}.deb" >/dev/null

# ---------- portable tarball (installs into ~/.local, no root)
echo "==> Packaging .tar.gz"
NAME="sift-${VERSION}-linux-${TAR_ARCH}"
TAR_ROOT="$STAGE/$NAME"
mkdir -p "$TAR_ROOT"
install -Dm755 target/release/sift "$TAR_ROOT/bin/sift"
install -Dm755 target/release/sift-gui "$TAR_ROOT/bin/sift-gui"
cp -r data "$TAR_ROOT/data"
rm -rf "$TAR_ROOT/data/fonts"/*.ttf
cp README.md NOTICE LICENSE "$TAR_ROOT/"
cat > "$TAR_ROOT/install.sh" <<'EOF'
#!/usr/bin/env bash
# Install Sift into ~/.local (no root). Re-run to upgrade.
set -euo pipefail
cd "$(dirname "$0")"
P="${PREFIX:-$HOME/.local}"
install -Dm755 bin/sift "$P/bin/sift"
install -Dm755 bin/sift-gui "$P/bin/sift-gui"
install -Dm644 data/dev.sift.Sift.desktop "$P/share/applications/dev.sift.Sift.desktop"
sed -i "s|^Exec=sift-gui|Exec=$P/bin/sift-gui|; s|^Icon=dev.sift.Sift|Icon=$P/share/icons/hicolor/256x256/apps/dev.sift.Sift.png|" "$P/share/applications/dev.sift.Sift.desktop"
(cd data/icons/hicolor && for f in */apps/dev.sift.Sift.*; do install -Dm644 "$f" "$P/share/icons/hicolor/$f"; done)
install -Dm644 data/sift-sampler.service "$HOME/.config/systemd/user/sift-sampler.service"
command -v update-desktop-database >/dev/null && update-desktop-database "$P/share/applications" 2>/dev/null || true
systemctl --user daemon-reload 2>/dev/null || true
echo "Sift installed. Open it from your app launcher or run: sift-gui"
EOF
chmod 755 "$TAR_ROOT/install.sh"
tar -C "$STAGE" -czf "$DIST/$NAME.tar.gz" "$NAME"

(cd "$DIST" && sha256sum ./* > SHA256SUMS)
echo "==> Done:"
ls -lh "$DIST"
