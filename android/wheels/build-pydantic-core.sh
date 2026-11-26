#!/bin/bash
# Cross-compile pydantic-core for Android (Chaquopy, Python 3.13), since no
# Android wheels exist on PyPI or Chaquopy's index.
#   ./build-pydantic-core.sh            -> dist/*.whl for arm64-v8a and x86_64
set -euo pipefail
cd "$(dirname "$0")"
HERE=$PWD
VERSION=2.50.2
API=24
NDK=${ANDROID_NDK_HOME:-$HOME/Android/Sdk/ndk/30.0.16248370}
TC=$NDK/toolchains/llvm/prebuilt/linux-x86_64/bin
export PATH=$HOME/.cargo/bin:$HOME/.local/bin:$PATH

if [ ! -d src/pydantic_core-$VERSION ]; then
    mkdir -p src
    curl -fsSL "$(cat sdist.url)" | tar -xz -C src
fi

mkdir -p dist
for abi in arm64-v8a x86_64; do
    case $abi in
        arm64-v8a) triple=aarch64-linux-android ;;
        x86_64)    triple=x86_64-linux-android ;;
    esac
    T=$(echo "$triple" | tr 'a-z-' 'A-Z_')
    cat > "$HERE/target/pyo3-$abi.txt" <<CFG
implementation=CPython
version=3.13
shared=true
abi3=false
lib_name=python3.13
lib_dir=$HERE/target/$abi/jniLibs/$abi
pointer_width=64
build_flags=
suppress_build_script_link_lines=false
CFG
    export PYO3_CONFIG_FILE=$HERE/target/pyo3-$abi.txt
    export CARGO_TARGET_${T}_LINKER=$TC/${triple}${API}-clang
    export CC_${triple//-/_}=$TC/${triple}${API}-clang
    export AR_${triple//-/_}=$TC/llvm-ar
    export CFLAGS_${triple//-/_}="-I$HERE/target/$abi/include/python3.13"
    (cd src/pydantic_core-$VERSION &&
        maturin build --release --target "$triple" -o "$HERE/dist/raw-$abi" --compatibility off)
done
mv dist/raw-*/*.whl dist/ && rmdir dist/raw-*
python3 fix-wheel-suffix.py dist/pydantic_core-*.whl
