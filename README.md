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

Photos are shown from Apple's JPEG renditions, so HEIC support isn't
needed to browse; Export saves the original file.

## Development

```bash
cargo run
```

Outside the Flatpak the helper runs with icloud-linux's virtualenv
(`~/.local/share/icloud-linux/.venv`). Test without an Apple account:

```bash
python3 tests/make_fake_library.py /tmp/fakelib 300
CLOUDPHOTOS_FAKE=/tmp/fakelib CLOUDPHOTOS_PYTHON=python3 \
CLOUDPHOTOS_DATA_DIR=/tmp/cp-data CLOUDPHOTOS_CACHE_DIR=/tmp/cp-cache cargo run
```

`CLOUDPHOTOS_SCRIPT="wait 3; shot /tmp/a.png; open 2; key Right; click Info; quit"`
drives the window for headless screenshots.
