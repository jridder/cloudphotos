# Cloud Photos for Android

The Android version of Cloud Photos. The screens are Kotlin and Jetpack
Compose. The iCloud side is the desktop app's own helper
(`../helper/photos_helper.py`), run inside the app with
[Chaquopy](https://chaquo.com/chaquopy/) and called through
`app/src/main/python/android_bridge.py`. A fix to the helper reaches both
apps.

What it does so far:

- signs in with an Apple ID and the two-factor code
- shows the library newest first, grouped by month, with video lengths and favorites
- shows albums: the smart ones (Favorites, Screenshots, Videos, Live…) and your own
- opens photos full screen and swipes between them
- plays videos, and the motion part of Live Photos
- remembers the library, so later launches only ask iCloud what changed

Videos and Live Photos stream Apple's smaller H.264 copy (`resVidMed`,
which the helper's `urls` returns with `stream`), not the original. Any
phone can decode that copy, and it uses less data. An iPhone original can
be 10-bit Dolby Vision HEVC, which some decoders can't play.

It has no editing, import, export, favoriting or deleting yet.

## Building

You need:

- JDK 17 or newer
- the Android SDK with platform 37.1 and NDK 30
- Python 3.13 on the build machine, which Chaquopy needs; `uv python install 3.13` works
- Rust with the `aarch64-linux-android` and `x86_64-linux-android` targets, plus `maturin`

```
wheels/build-pydantic-core.sh      # once: pydantic-core for Android, into wheels/dist
./gradlew assembleDebug            # app/build/outputs/apk/debug/app-debug.apk
```

The build looks for Python 3.13 in uv's folder or at `/usr/bin/python3.13`. To use
another one, set `CHAQUOPY_BUILD_PYTHON`.

### Why pydantic-core is built here

pyicloud's Photos code needs pydantic, and pydantic-core has no Android
builds on PyPI or in Chaquopy's index. `wheels/build-pydantic-core.sh`
cross-compiles it with maturin against Chaquopy's Python 3.13. It then
renames the compiled module to the `-linux-android.so` suffix that
Android's Python imports.

Chaquopy's newest cryptography is 42.0.8, but pyicloud asks for 44 or
newer. pyicloud only uses AES-GCM, elliptic-curve keys and hashing, which
42 has. So `app/build.gradle.kts` installs a pinned set of packages
(the set icloud-linux runs) with `--no-deps`, instead of letting pip
resolve them. When you upgrade pyicloud, update that list too.

## Testing without an Apple account

Debug builds use a fake library when one is in the app's files:

```
python3 ../tests/make_fake_library.py /tmp/fake 300
sed -i 's|file:///tmp/fake|file:///data/user/0/com.jridder.cloudphotos/files/fake|g' /tmp/fake/library.json
adb push /tmp/fake /data/local/tmp/ && adb shell chmod -R a+rX /data/local/tmp/fake
adb shell run-as com.jridder.cloudphotos cp -r /data/local/tmp/fake files/fake
```

In the fake library, any password works except `wrong`, and the code is
`123456`. The generator's `clip.mov` is random bytes. To test playback,
replace it with a real clip, for example from
`ffmpeg -f lavfi -i testsrc2=size=1280x720 -t 4 -c:v libx264 -pix_fmt yuv420p clip.mov`. Delete `files/fake` to go back to iCloud.

To load pyicloud and everything it needs on a device without signing in,
run this and check logcat for `selftest OK`:

```
adb shell am start -n com.jridder.cloudphotos/.MainActivity --ez selftest true
```

## Installing on a phone

Turn on USB debugging, connect the phone, and run:

```
./gradlew installDebug
```

You can also copy `app-debug.apk` to the phone and open it. The phone
will ask you to allow installs from that source.

The app talks to Apple's iCloud.com web service, as the desktop app does.
It isn't an Apple app, and Apple can change that service without warning.
