#!/bin/bash
# Sign a macOS binary with the Developer ID and have Apple notarise it.
# Usage: macos-sign.sh BINARY [WAIT], WAIT being how long to wait for Apple
# (notarytool's --timeout, default 10m). Reads the MACOS_* secrets from the
# environment; docs/macos-plan.md lists them.
set -euo pipefail
bin=$1
wait=${2:-10m}

# A throwaway keychain holding only the certificate.
kc="$RUNNER_TEMP/signing.keychain-db"
kcpw=$(openssl rand -hex 16)
security create-keychain -p "$kcpw" "$kc"
trap 'security delete-keychain "$kc"; rm -f "$RUNNER_TEMP/notary.p8"' EXIT
security set-keychain-settings -lut 1800 "$kc"
security unlock-keychain -p "$kcpw" "$kc"
printf '%s' "$MACOS_CERT_P12" | base64 --decode > "$RUNNER_TEMP/cert.p12"
security import "$RUNNER_TEMP/cert.p12" -k "$kc" -P "$MACOS_CERT_PASSWORD" -T /usr/bin/codesign
rm "$RUNNER_TEMP/cert.p12"
security set-key-partition-list -S apple-tool:,apple: -s -k "$kcpw" "$kc" > /dev/null
security list-keychains -d user -s "$kc"
# Notarisation needs the hardened runtime and a secure timestamp.
codesign --force --options runtime --timestamp --keychain "$kc" --sign "Developer ID Application" "$bin"
codesign --verify --strict --verbose=2 "$bin"

printf '%s\n' "$MACOS_NOTARY_KEY" > "$RUNNER_TEMP/notary.p8"
ditto -c -k "$bin" "$RUNNER_TEMP/clipt.zip"
# Apple's queue can take far longer than a job should wait, but a bare
# binary's notarisation is looked up online when it first runs, so the
# build counts as notarised once Apple accepts it. Wait a while, then hand
# it over with the submission's id; the notary workflow can check it later.
auth=(--key "$RUNNER_TEMP/notary.p8" --key-id "$MACOS_NOTARY_KEY_ID" --issuer "$MACOS_NOTARY_ISSUER")
xcrun notarytool submit "$RUNNER_TEMP/clipt.zip" "${auth[@]}" --output-format json > "$RUNNER_TEMP/notary.json"
id=$(plutil -extract id raw "$RUNNER_TEMP/notary.json")
echo "notarisation submission: $id"
xcrun notarytool wait "$id" "${auth[@]}" --timeout "$wait" || true
xcrun notarytool info "$id" "${auth[@]}" --output-format json > "$RUNNER_TEMP/notary.json"
status=$(plutil -extract status raw "$RUNNER_TEMP/notary.json")
echo "notarisation status: $status"
case "$status" in
  Accepted) ;;
  "In Progress") echo "::warning::Apple is still notarising submission $id; the build will pass Gatekeeper once it's accepted" ;;
  *)
    xcrun notarytool log "$id" "${auth[@]}"
    exit 1
    ;;
esac
