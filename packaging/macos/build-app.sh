#!/bin/bash
# Wrap a macOS clipt binary in clipt.app.
# Usage: [CLIPT_SIGN_IDENTITY=NAME] packaging/macos/build-app.sh BINARY OUTDIR,
# run from the repository root on a Mac; writes OUTDIR/clipt.app, ad-hoc
# signed unless CLIPT_SIGN_IDENTITY names a certificate
# (.github/scripts/macos-sign.sh re-signs it with the Developer ID).
#
# The bundle gives clipt a fixed identity: macOS files its Screen Recording
# permission under the bundle ID rather than under whichever terminal or
# launcher ran it, and it can sit in Applications, Spotlight and Login
# Items. LSUIElement keeps it out of the Dock. Opened from Finder it starts
# the editor, as `clipt` does.
set -euo pipefail
bin=$1
out=$2
here=$(dirname "$0")
version=$("$bin" --version | awk '{print $2}')
app="$out/clipt.app"

rm -rf "$app"
mkdir -p "$app/Contents/MacOS" "$app/Contents/Resources"
cp "$bin" "$app/Contents/MacOS/clipt"
sed "s/@VERSION@/$version/g" "$here/Info.plist" > "$app/Contents/Info.plist"
plutil -lint "$app/Contents/Info.plist"
# Apple's iconutil writes the .icns, from PNGs clipt draws.
iconset=$(mktemp -d)/clipt.iconset
"$bin" debug-icon -o "$iconset"
iconutil -c icns -o "$app/Contents/Resources/clipt.icns" "$iconset"
rm -rf "$(dirname "$iconset")"
# Ad-hoc by default. macOS ties the Screen Recording permission to the
# signature, and an ad-hoc one changes with every build, so a rebuilt app
# is asked again; set CLIPT_SIGN_IDENTITY to a certificate in your keychain
# (an "Apple Development" one from Xcode will do) to keep the permission.
codesign --force --sign "${CLIPT_SIGN_IDENTITY:--}" "$app"
echo "built $app ($version)"
