# Cloud Photos

Browse your iCloud Photos library on GNOME: a timeline of your library,
albums, Favorites/Videos/Live Photos/Screenshots, a full-window viewer
with zoom, Live Photo and video playback, favoriting, deleting (to
Recently Deleted) and exporting originals.

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

Photos are displayed from the JPEG preview, so no HEIC decoder is needed;
Export writes the original out of the library.

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
