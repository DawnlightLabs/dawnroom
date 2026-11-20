#!/usr/bin/env bash
# Inside the linux.Dockerfile container: copy the read-only source (/src) to /work without Windows
# build output, build the engine, then Studio's .deb and AppImage, and copy them to /out.
# Updater artefacts (.sig) are signed when TAURI_SIGNING_PRIVATE_KEY is passed in (docker run -e; the
# key is never part of the image or /out). NEXT=0.1.1 also builds that version's engine tarball into
# /out/next, so `dawnroom update` can be tested against a real newer binary.
set -euo pipefail
# linuxdeploy (used for the AppImage) is an AppImage itself; containers have no FUSE to mount it.
export APPIMAGE_EXTRACT_AND_RUN=1
mkdir -p /work /out
cd /src
tar --exclude='./engine/target' --exclude='./studio/src-tauri/target' --exclude='./studio/src-tauri/binaries' \
    --exclude='./node_modules' --exclude='./studio/node_modules' --exclude='./site' --exclude='./.data' \
    --exclude='./.models' --exclude='./.shots' --exclude='./spike' --exclude='./.ref' --exclude='./.git' --exclude='./.keys' -cf - . | tar -xf - -C /work

cd /work/engine
cargo build --release --bin dawnroom
mkdir -p /work/studio/src-tauri/binaries
cp target/release/dawnroom "/work/studio/src-tauri/binaries/dawnroom-$(rustc -vV | sed -n 's/^host: //p')"

cd /work/studio
npm ci
npx tauri build --bundles deb,appimage
cp src-tauri/target/release/bundle/deb/*.deb src-tauri/target/release/bundle/appimage/*.AppImage /out/
cp src-tauri/target/release/bundle/deb/*.sig src-tauri/target/release/bundle/appimage/*.sig /out/ 2>/dev/null || echo "no updater signatures (TAURI_SIGNING_PRIVATE_KEY not set)"

# The --cli download (install.sh): the engine and the model list it reads beside itself.
VERSION="$(sed -n 's/.*"version": *"\([^"]*\)".*/\1/p' src-tauri/tauri.conf.json | head -n 1)"
mkdir -p /work/cli/registry/runtimes
cp /work/engine/target/release/dawnroom /work/cli/
cp /work/registry/*.toml /work/cli/registry/ && cp /work/registry/runtimes/*.toml /work/cli/registry/runtimes/
tar -czf "/out/dawnroom-$VERSION-linux-x64.tar.gz" -C /work/cli .
cd /out && sha256sum *.deb *.AppImage *.tar.gz > SHA256SUMS.linux

if [ -n "${NEXT:-}" ]; then
  cd /work/engine
  sed -i "0,/^version = \".*\"/s//version = \"$NEXT\"/" Cargo.toml
  cargo build --release --bin dawnroom
  rm -rf /work/next && mkdir -p /work/next/registry/runtimes /out/next
  cp target/release/dawnroom /work/next/ && cp /work/registry/*.toml /work/next/registry/ && cp /work/registry/runtimes/*.toml /work/next/registry/runtimes/
  tar -czf "/out/next/dawnroom-$NEXT-linux-x64.tar.gz" -C /work/next .
fi
ls -la /out /out/next 2>/dev/null
