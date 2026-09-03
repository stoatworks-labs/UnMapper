#!/usr/bin/env bash
# Build UnMapper's release artefacts on this Mac.
#
# There is no CI for this repo yet, so this is the release. It builds a
# universal macOS binary pair and wraps the GUI in a double-clickable .app.
#
#   scripts/release-local.sh            # universal macOS build + UnMapper.app
#   scripts/release-local.sh --fast     # this machine's arch only, for a quick check
#
# Deliberately macOS-only for now. Windows and Linux builds are possible with
# cargo-xwin / cargo-zigbuild (see openstage's release-local.sh), but nothing
# here has ever been *run* on either, so shipping binaries for them would imply
# a confidence that does not exist.
set -euo pipefail

cd "$(dirname "$0")/.."
VERSION=$(grep -m1 '^version' Cargo.toml | cut -d'"' -f2)
OUT=dist-release
FAST=${1:-}

echo "UnMapper $VERSION"
rm -rf "$OUT" && mkdir -p "$OUT"

cargo test --release --quiet
cargo clippy --release --all-targets -- -D warnings

if [ "$FAST" = "--fast" ]; then
  cargo build --release
  BIN_GUI=target/release/unmapper-gui
  BIN_CLI=target/release/unmapper
else
  for T in aarch64-apple-darwin x86_64-apple-darwin; do
    echo "building $T"
    cargo build --release --target "$T"
  done
  mkdir -p "$OUT/bin"
  # A universal binary rather than two downloads. `lipo -info` is the only
  # honest check that both slices are really in there — the build log will
  # happily claim success having produced one.
  for B in unmapper-gui unmapper; do
    lipo -create \
      "target/aarch64-apple-darwin/release/$B" \
      "target/x86_64-apple-darwin/release/$B" \
      -output "$OUT/bin/$B"
    lipo -info "$OUT/bin/$B"
  done
  BIN_GUI="$OUT/bin/unmapper-gui"
  BIN_CLI="$OUT/bin/unmapper"
fi

APP="$OUT/UnMapper.app"
mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Resources"
# NOT "UnMapper" and "unmapper" side by side: macOS filesystems are
# case-INSENSITIVE by default, so those are one file and the second copy
# silently replaces the first. The bundle then launches the CLI, which prints
# usage to a log nobody reads and exits.
cp "$BIN_GUI" "$APP/Contents/MacOS/UnMapper"
cp "$BIN_CLI" "$APP/Contents/MacOS/unmapper-cli"

cat > "$APP/Contents/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>CFBundleName</key><string>UnMapper</string>
  <key>CFBundleDisplayName</key><string>UnMapper</string>
  <key>CFBundleIdentifier</key><string>com.stoatworks.unmapper</string>
  <key>CFBundleVersion</key><string>$VERSION</string>
  <key>CFBundleShortVersionString</key><string>$VERSION</string>
  <key>CFBundlePackageType</key><string>APPL</string>
  <key>CFBundleExecutable</key><string>UnMapper</string>
  <key>LSMinimumSystemVersion</key><string>11.0</string>
  <key>NSHighResolutionCapable</key><true/>
  <key>CFBundleDocumentTypes</key>
  <array><dict>
    <key>CFBundleTypeName</key><string>UnMapper stage</string>
    <key>CFBundleTypeRole</key><string>Editor</string>
    <key>LSItemContentTypes</key><array><string>public.xml</string></array>
  </dict></array>
</dict>
</plist>
PLIST

# NDI is loaded at run time and never bundled — see docs. If an operator has
# installed the runtime, the app finds it; if not, it says so with the URL.
cp README.md LICENSE "$APP/Contents/Resources/"

echo
# Prove the bundle launches the GUI and not the CLI — the case-insensitivity
# trap above produced exactly that, and only checking the file catches it.
#
# Under an alarm, because this check is the one thing here that runs a GUI
# binary: a version that treats `--help` as a filename opens a window and never
# returns, and this script then hangs for ever having printed nothing at all.
# It has done exactly that. A timeout is a failure, not a pass — a check that
# cannot complete has not succeeded.
if ! HELP=$(perl -e 'alarm 20; exec @ARGV' "$APP/Contents/MacOS/UnMapper" --help 2>&1); then
  echo "ERROR: Contents/MacOS/UnMapper did not answer --help and exit" >&2
  echo "       (a GUI that opens a window here hangs the build)" >&2
  exit 1
fi
if printf '%s\n' "$HELP" | grep -q "Usage: UnMapper <COMMAND>"; then
  echo "ERROR: Contents/MacOS/UnMapper is the CLI, not the GUI" >&2
  exit 1
fi

echo "wrote $APP"
du -sh "$APP"
echo
echo "Unsigned: macOS will quarantine it on first open. Either right-click →"
echo "Open, or:  xattr -dr com.apple.quarantine $APP"
