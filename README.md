# Cloud Photos

Browse and edit your iCloud Photos library on GNOME: a timeline of your
library, albums, Favorites/Videos/Live Photos/Screenshots, a full-window
viewer with zoom, Live Photo and video playback, an Info panel (camera,
exposure, location, keywords), favoriting, deleting (to Recently Deleted)
and exporting.

Editing is non-destructive: crop, rotate and flip, Light and Color
adjustments (exposure, contrast, highlights, shadows, saturation,
vibrance, warmth, tint, sepia) and one-click Looks (Vivid, Warm, Cool,
Dramatic, Mono, Noir, Silvertone, Sepia…). Edits are rendered from the
full-resolution original, keep its EXIF and color profile, can be undone
or reverted, and can be saved back to iCloud as a copy.

## Install

Download `CloudPhotos-<version>.flatpak` from the
[releases](https://github.com/jridder/cloudphotos/releases) and run

```bash
flatpak install --user CloudPhotos-0.1.0.flatpak
```

It needs [icloud-linux](https://github.com/jridder/icloud-linux) (the
iCloud Drive app) to be signed in. The Flatpak can read only its settings
and session (`~/.config/icloud-linux`, read only) and the library folder
`~/Pictures/iCloud Photos`.

To build the bundle yourself: `flatpak/make-flatpak.sh` (GNOME 51 SDK and
the rust-stable extension; output goes to `~/cloudphotos-packages`).

The app is Rust (GTK 4 + libadwaita). Talking to Apple goes through a
small Python helper (`helper/photos_helper.py`) built on pyicloud, and it
reuses the sign-in of [icloud-linux](https://github.com/jridder/icloud-linux)
instead of signing in on its own: it never sends your password. If that
session expires, sign in again in the iCloud Drive app.

## The library

Everything lives in one SQLite file, `~/Pictures/iCloud Photos/Photos Library.sqlite`:
the metadata and albums, and the files themselves (originals, Live Photo
videos, and the JPEG thumbnail and preview used for display) as blobs,
streamed in and out with SQLite's incremental blob I/O.

After each check with iCloud, the app downloads whatever is missing:
thumbnails for everything first, then previews, then originals, newest
first. Download links are fetched 100 items at a time right before use
(they expire within minutes), each file's size is checked before it's
stored, and edited photos (new checksum) download again. Photos deleted
in iCloud stay hidden-but-kept while in Recently Deleted and are removed
locally once iCloud purges them.

The grid and viewer show Apple's JPEG renditions. Editing decodes the
original (HEIC included) through [glycin](https://gitlab.gnome.org/GNOME/glycin);
the Flatpak ships its own HEIC loader because the GNOME runtime's has no
HEVC decoder. Export writes the original or the edited version.

## Development

```bash
cargo run
```

Outside the Flatpak the helper runs with icloud-linux's virtualenv
(`~/.local/share/icloud-linux/.venv`). Test without an Apple account:

```bash
python3 tests/make_fake_library.py /tmp/fakelib 300
CLOUDPHOTOS_FAKE=/tmp/fakelib CLOUDPHOTOS_PYTHON=python3 \
CLOUDPHOTOS_LIBRARY=/tmp/cp/Library.sqlite CLOUDPHOTOS_DATA_DIR=/tmp/cp/data \
CLOUDPHOTOS_CACHE_DIR=/tmp/cp/cache cargo run
```

`CLOUDPHOTOS_SCRIPT="wait 3; shot /tmp/a.png; open 2; key Right; click Info; quit"`
drives the window for headless screenshots.

## Windows

The same app runs on Windows, built with [MSYS2](https://www.msys2.org)'s
UCRT64 environment (GTK 4, libadwaita, GStreamer, libheif, Rust, Python).
What differs from Linux:

- **Sign-in**: there's no icloud-linux to share a sign-in with, so the app
  signs in itself (Apple ID, password, then the two-factor code). The
  password goes to Apple through pyicloud and isn't kept; the session
  cookies and Apple ID are, in `%LOCALAPPDATA%\cloudphotos`.
- **The library** is `Pictures\Cloud Photos\Photos Library.sqlite`
  (iCloud for Windows has used `Pictures\iCloud Photos` itself).
- **Decoding originals** goes through libheif (HEIC) and the image crate
  (JPEG, PNG, TIFF) instead of glycin, which is Linux-only.
- **Video** plays through GStreamer's `gtk4paintablesink` (`src/ui/player.rs`):
  GTK's own GStreamer media hands over D3D12 textures that come out blank.

Set up once (installs MSYS2 packages and the helper's venv):

```powershell
winget install MSYS2.MSYS2
C:\msys64\usr\bin\bash.exe -lc "pacman -Syu --noconfirm"
C:\msys64\usr\bin\bash.exe -lc "pacman -S --needed --noconfirm mingw-w64-ucrt-x86_64-{toolchain,pkgconf,rust,gtk4,libadwaita,libheif,gstreamer,gst-plugins-base,gst-plugins-good,gst-plugins-bad,gst-plugins-rs,gst-libav,python,python-pip,python-gobject,ntldd,nsis,adwaita-icon-theme}"
windows\setup-dev.ps1
```

Run from the source tree:

```powershell
. windows\env.ps1   # MSYS2 on PATH, builds in target\ucrt64
cargo run
```

A folder that runs without MSYS2 (`dist\CloudPhotos\bin\cloudphotos.exe`,
about 280 MB with its Python), from an MSYS2 UCRT64 shell:

```bash
windows/make-bundle.sh
```

And the installer, `dist\CloudPhotos-<version>-setup.exe` (needs
`mingw-w64-ucrt-x86_64-nsis`): it builds the bundle, then packs it.

```bash
windows/make-installer.sh
```

It installs per user (no administrator rights) into
`%LOCALAPPDATA%\Programs\Cloud Photos`, with a Start menu shortcut and an
uninstaller in Settings › Apps; `/S` installs silently. Uninstalling leaves
the photo library and the sign-in. The executable's icon and version come
from `windows/cloudphotos.rc` (embedded by `build.rs`).

The fake library works the same way (`tests/make_fake_library.py` needs
`python-gobject`); with `CLOUDPHOTOS_FAKE` the helper's sign-in takes any
password but `wrong`, and the code `123456`.
