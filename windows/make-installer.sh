#!/bin/bash
# Build the bundle and its installer, dist/CloudPhotos-<version>-setup.exe.
# Run in an MSYS2 UCRT64 shell (needs mingw-w64-ucrt-x86_64-nsis):
#   windows/make-installer.sh
set -euo pipefail
cd "$(dirname "$0")/.."
windows/make-bundle.sh
version=$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -1)
makensis -V2 -DVERSION="$version" windows/installer.nsi
ls -lh "dist/CloudPhotos-$version-setup.exe"
