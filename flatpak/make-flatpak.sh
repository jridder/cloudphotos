#!/bin/sh
# Builds Cloud Photos as a Flatpak bundle into ~/cloudphotos-packages (or the
# directory given). Everything the build needs is fetched here first (Rust
# crates for the app and for glycin's heif loader, pyicloud's wheels), so
# flatpak-builder itself runs offline. Needs the GNOME 51 SDK and the
# rust-stable extension:
#   flatpak install --user flathub org.gnome.Sdk//51 org.freedesktop.Sdk.Extension.rust-stable//26.08
set -e
cd "$(dirname "$0")"
OUT=${1:-$HOME/cloudphotos-packages}
VERSION=$(sed -n 's/^version = "\(.*\)"/\1/p' ../Cargo.toml | head -1)
GLYCIN=2.2.4
PYTHON=3.14  # the GNOME 51 runtime's

work=$(mktemp -d)
trap 'rm -r -- "$work"' EXIT

# The app's crates.
(cd .. && cargo vendor --locked --versioned-dirs "$work/vendor") > "$work/vendor-config.toml"
sed -i 's|directory = ".*"|directory = "vendor"|' "$work/vendor-config.toml"
tar -C "$work" -cJf cloudphotos-vendor.tar.xz vendor vendor-config.toml
rm -r "$work/vendor"

# glycin's crates (its release tarball doesn't carry them).
curl -sSfL -o "$work/glycin.tar.xz" "https://download.gnome.org/sources/glycin/${GLYCIN%.*}/glycin-$GLYCIN.tar.xz"
tar -C "$work" -xf "$work/glycin.tar.xz"
mkdir -p "$work/glycin-vendor/.cargo"
(cd "$work/glycin-$GLYCIN" && cargo vendor --locked --versioned-dirs "$work/glycin-vendor/vendor") > "$work/glycin-vendor/.cargo/config.toml"
sed -i 's|directory = ".*"|directory = "vendor"|' "$work/glycin-vendor/.cargo/config.toml"
tar -C "$work/glycin-vendor" -cJf glycin-vendor.tar.xz .cargo vendor

# pyicloud and its dependencies, built for the runtime's Python.
mkdir "$work/wheels"
python3 -m pip download -q -d "$work/wheels" --only-binary=:all: \
  --python-version "$PYTHON" --implementation cp --abi "cp${PYTHON%.*}${PYTHON#*.}" --abi abi3 --abi none \
  --platform manylinux_2_28_x86_64 --platform manylinux_2_17_x86_64 --platform manylinux2014_x86_64 --platform any \
  -r ../helper/requirements.txt
tar -C "$work/wheels" -cf python-wheels.tar .

# rofiles-fuse needs fusermount, which isn't always allowed; it's only a
# cache optimisation.
flatpak-builder --user --force-clean --disable-rofiles-fuse --repo=repo build-dir com.jridder.CloudPhotos.yml

mkdir -p "$OUT"
BUNDLE="$OUT/CloudPhotos-$VERSION.flatpak"
flatpak build-bundle repo "$BUNDLE" com.jridder.CloudPhotos \
  --runtime-repo=https://dl.flathub.org/repo/flathub.flatpakrepo
echo "Built $BUNDLE"
echo "Install with: flatpak install --user $BUNDLE"
