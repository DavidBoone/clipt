#!/bin/bash
# Regenerates docs/screenshot.png, the README's picture (needs Docker).
set -e
cd "$(dirname "$0")/../.."
cargo build --release --bin clipt
docker build -q -t clipt-gnome tests/gnome >/dev/null
docker build -q -t clipt-screenshot tests/screenshot >/dev/null
out=$(mktemp -d); chmod 777 "$out"
docker run --rm --shm-size=512m -v $PWD/target/release/clipt:/clipt:ro -v $PWD/data/clipt.desktop:/clipt.desktop:ro \
  -v $PWD/tests/screenshot:/t:ro -v $PWD/tests/gnome/rdinput.py:/rdinput.py:ro -v "$out":/out clipt-screenshot /t/session.sh >/dev/null 2>&1
cp "$out/screenshot.png" docs/screenshot.png
rm -rf "$out"
echo "wrote docs/screenshot.png"
