#!/bin/bash
# Build a self-contained Windows folder, dist/CloudPhotos: run
# bin\cloudphotos.exe from it, no MSYS2 needed. Run in an MSYS2 UCRT64 shell
# after windows/setup-dev.ps1 (the helper's packages come from its venv):
#   windows/make-bundle.sh
set -euo pipefail
cd "$(dirname "$0")/.."
prefix=/ucrt64
out=dist/CloudPhotos
venv_python="$(cygpath -u "$LOCALAPPDATA")/cloudphotos/venv/bin/python.exe"

export CARGO_TARGET_DIR=target/ucrt64
cargo build --release

rm -rf "$out"
mkdir -p "$out"/{bin,lib,share,libexec/cloudphotos}
cp "$CARGO_TARGET_DIR/release/cloudphotos.exe" "$out/bin/"
# GApplication's single-instance check talks D-Bus through gdbus.exe.
cp "$prefix/bin/gdbus.exe" "$out/bin/"
cp helper/photos_helper.py "$out/libexec/cloudphotos/"

# GStreamer: what playing iPhone videos (MOV, H.264/HEVC, AAC) needs.
mkdir -p "$out/lib/gstreamer-1.0" "$out/libexec/gstreamer-1.0"
for plugin in coreelements playback typefindfunctions isomp4 libav videoconvertscale videofilter \
    audioconvert audioresample autodetect wasapi wasapi2 directsound volume gtk4 videoparsersbad \
    audioparsers app deinterlace; do
  cp "$prefix/lib/gstreamer-1.0/libgst$plugin.dll" "$out/lib/gstreamer-1.0/"
done
cp "$prefix/libexec/gstreamer-1.0/gst-plugin-scanner.exe" "$out/libexec/gstreamer-1.0/"

# gdk-pixbuf loaders (SVG icons), cached by relative path: gdk-pixbuf
# resolves those against the bundle wherever it's moved.
loaders=lib/gdk-pixbuf-2.0/2.10.0/loaders
mkdir -p "$out/$loaders"
cp "$prefix/$loaders"/{libpixbufloader-png.dll,libpixbufloader-jpeg.dll,pixbufloader_svg.dll} "$out/$loaders/"
gdk-pixbuf-query-loaders "$out/$loaders"/*.dll | grep -v '^# LoaderDir' \
  | sed "s|\"$(cygpath -m "$PWD/$out")/|\"|" > "$out/lib/gdk-pixbuf-2.0/2.10.0/loaders.cache"

# Settings schemas and icons.
mkdir -p "$out/share/glib-2.0/schemas" "$out/share/icons"
cp "$prefix/share/glib-2.0/schemas/gschemas.compiled" "$out/share/glib-2.0/schemas/"
cp -r "$prefix/share/icons/Adwaita" "$prefix/share/icons/hicolor" "$out/share/icons/"
cp -r data/icons/hicolor/* "$out/share/icons/hicolor/"
gtk4-update-icon-cache -q -t -f "$out/share/icons/hicolor"

# Python for the helper: the standard library, and pyicloud with what it needs.
cp "$prefix/bin/python.exe" "$out/bin/"
pyver=$(python -c 'import sys; print("python%d.%d" % sys.version_info[:2])')
mkdir -p "$out/lib/$pyver"
tar -C "$prefix/lib/$pyver" -cf - --exclude=./site-packages --exclude=./test --exclude=./idlelib \
    --exclude=./tkinter --exclude=./turtledemo --exclude=./ensurepip --exclude=__pycache__ . \
  | tar -C "$out/lib/$pyver" -xf -
"$venv_python" windows/bundle_python.py "$out/lib/$pyver/site-packages"
# OpenSSL's providers (the app points OPENSSL_MODULES here for the helper).
cp -r "$prefix/lib/ossl-modules" "$out/lib/"

# Every MSYS2 DLL the programs, plugins and Python modules link to (ntldd,
# because ldd won't read .pyd files).
find "$out" \( -name '*.exe' -o -name '*.dll' -o -name '*.pyd' \) -print0 \
  | xargs -0 -n 20 ntldd -R 2>/dev/null \
  | awk '{print $3}' | tr '\\' '/' | grep -i "^$(cygpath -m "$prefix")/bin/" | sort -u \
  | while read -r dll; do [ -e "$out/bin/${dll##*/}" ] || cp "$dll" "$out/bin/"; done

echo "$(du -sh "$out" | cut -f1) in $out"
