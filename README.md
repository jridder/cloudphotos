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
