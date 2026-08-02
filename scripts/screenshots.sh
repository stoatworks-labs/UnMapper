#!/usr/bin/env bash
# Regenerate docs/screenshots/previz.png from a repo fixture.
#
# Everything here runs offline: `import` defaults every source to a test
# pattern, so no Resolume, no NDI sender and no network are involved. That is
# the point — the shot has to be reproducible on any machine, and a screenshot
# that needs a live rig behind it is one that quietly goes stale.
#
#   ./scripts/screenshots.sh
#
# The other image, docs/screenshots/gui-emulation.png, is a capture of the
# actual GUI window and is NOT produced here — unmapper-gui has no headless
# mode, so it is taken by hand:
#
#   cargo build --release -p unmapper-gui
#   ./target/release/unmapper-gui <a-stage>.unmapper.xml
#   # then capture the window, e.g. with macOS Screenshot (Shift-Cmd-4, Space)
#
# Re-shoot it whenever the panel layout or the side panels change.
set -euo pipefail

cd "$(dirname "$0")/.."
OUT=docs/screenshots
FIXTURE=crates/unmapper-resolume/tests/fixtures/pixel-peeker-export.xml
BIN=target/release/unmapper

mkdir -p "$OUT"
[ -x "$BIN" ] || cargo build --release -p unmapper-app

WORK="$(mktemp -d)"
trap 'rm -rf "$WORK"' EXIT

"$BIN" import "$FIXTURE" -o "$WORK/stage.unmapper.xml" >/dev/null
"$BIN" render "$WORK/stage.unmapper.xml" --previz --size 1600x900 -o "$OUT/previz.png"

echo "wrote $OUT/previz.png"
