#!/usr/bin/env python3
"""
photos_helper.py — Cloud Photos' link to iCloud, the one part not in Rust.

Talks JSON lines on stdin/stdout:
  -> {"id": 1, "method": "connect", "params": {...}}
  <- {"id": 1, "result": {...}}            or {"id": 1, "error": {"code": ..., "message": ...}}
  <- {"event": "assets", "items": [...]}   (streamed while a scan runs)

It never signs in with a password. It reuses the session icloud-linux saved
(copied, so the two never write the same files); when that session has
expired it answers "needs_signin" and the app sends you to the iCloud Drive
app's Sign In Again.

Methods:
  connect  {config, cookie_dir, session_dir}   -> {apple_id}
  scan     {}                                  -> {sync_token}; emits "assets" and "albums"
  changed  {since}                             -> {changed: bool}
  urls     {ids: [...]}                        -> {id: {key: url}} with fresh download URLs
  favorite {id, value}                         -> {}
  delete   {id}                                -> {}   (moves to Recently Deleted)

`--fake DIR` serves DIR/library.json instead, for tests and screenshots.
"""
import json
import os
import shutil
import sys
import time

SMART_ALBUMS = ("Favorites", "Screenshots", "Videos", "Live", "Panoramas", "Slo-mo", "Time-lapse", "Bursts")
BATCH = 200


def emit(message):
    sys.stdout.write(json.dumps(message) + "\n")
    sys.stdout.flush()


class HelperError(Exception):
    def __init__(self, code, message):
        super().__init__(message)
        self.code = code


# ── resources ───────────────────────────────────────────────────────────────

def resource_dict(resource):
    return {
        "url": resource.url,
        "size": resource.size,
        "width": resource.width,
        "height": resource.height,
        "type": resource.type,
    }


def pick(resources, *keys):
    for key in keys:
        if key in resources and resources[key].url:
            return resources[key]
    return None


def asset_resources(asset):
    """Normalize pyicloud's per-type resource names into what the app uses."""
    r = asset.resources
    movie = asset.item_type == "movie"
    out = {}
    for name, keys in (
        ("thumb", ("thumb_image",) if movie else ("thumb",)),
        ("preview", ("medium_image", "thumb_image") if movie else ("medium", "thumb")),
        ("original", ("original",)),
        # Streamable video: the smaller rendition first.
        ("video", ("medium", "original") if movie else ()),
        ("live", () if movie else ("medium_video", "original_video")),
    ):
        res = pick(r, *keys)
        if res is not None:
            out[name] = resource_dict(res)
    return out


def is_favorite(asset):
    from pyicloud.services.photos_cloudkit.mappers import record_field_value

    return bool(record_field_value(asset.asset_record, "isFavorite"))


def asset_dict(asset):
    resources = asset_resources(asset)
    if "original" not in resources:
        return None
    taken = asset.asset_date or asset.created
    added = asset.added_date or taken
    width, height = asset.dimensions
    return {
        "id": asset.id,
        "filename": asset.filename or "",
        "kind": "video" if asset.item_type == "movie" else "photo",
        "taken": taken.timestamp(),
        "added": added.timestamp(),
        "width": width or 0,
        "height": height or 0,
        "size": resources["original"]["size"] or 0,
        "live": bool(asset.is_live_photo),
        "favorite": is_favorite(asset),
        "resources": resources,
    }


# ── iCloud ──────────────────────────────────────────────────────────────────

class ICloudLibrary:
    def __init__(self):
        self.api = None
        self.assets = {}

    def connect(self, params):
        import yaml
        from pyicloud import PyiCloudService

        config_path = os.path.expanduser(params.get("config") or "~/.config/icloud-linux/config.yaml")
        try:
            with open(config_path, encoding="utf-8") as handle:
                config = yaml.safe_load(handle) or {}
        except OSError:
            raise HelperError("not_configured", "Set up the iCloud Drive app first; Cloud Photos uses its sign-in.")
        username = config.get("username")
        if not username:
            raise HelperError("not_configured", "Set up the iCloud Drive app first; Cloud Photos uses its sign-in.")

        source = os.path.expanduser(params.get("cookie_dir") or config.get("cookie_dir") or "~/.config/icloud-linux/cookies")
        session_dir = os.path.expanduser(params["session_dir"])
        if os.path.isdir(source):
            shutil.rmtree(session_dir, ignore_errors=True)
            shutil.copytree(source, session_dir)
        os.makedirs(session_dir, exist_ok=True)

        # password=None: pyicloud can only reuse the saved session; it never
        # attempts a password sign-in on our behalf.
        api = PyiCloudService(username, None, cookie_directory=session_dir, authenticate=False)
        try:
            api.authenticate()
        except Exception as exc:
            raise HelperError("needs_signin", f"iCloud needs you to sign in again ({exc}).")
        if api.requires_2fa or api.requires_2sa:
            raise HelperError("needs_signin", "iCloud needs you to sign in again.")
        self.api = api
        apple_id = username
        if getattr(api, "data", None) and "dsInfo" in api.data:
            apple_id = api.data["dsInfo"].get("appleId", username)
        return {"apple_id": apple_id}

    def _photos(self):
        if self.api is None:
            raise HelperError("not_connected", "Not connected to iCloud.")
        return self.api.photos

    def scan(self, _params):
        photos = self._photos()
        token = photos.sync_cursor()
        batch = []
        self.assets = {}
        for asset in photos.all:
            item = asset_dict(asset)
            if item is None:
                continue
            self.assets[asset.id] = asset
            batch.append(item)
            if len(batch) >= BATCH:
                emit({"event": "assets", "items": batch})
                batch = []
        if batch:
            emit({"event": "assets", "items": batch})

        albums = []
        for album in photos.albums:
            kind = type(album).__name__
            if kind == "PhotoAlbumFolder":
                continue
            if kind == "SmartPhotoAlbum":
                if album.name not in SMART_ALBUMS:
                    continue
                albums.append({"id": album.id, "name": album.name, "kind": "smart",
                               "assets": [a.id for a in album]})
            else:
                albums.append({"id": album.id, "name": album.fullname, "kind": "user",
                               "assets": [a.id for a in album]})
        emit({"event": "albums", "albums": albums})
        return {"sync_token": token}

    def changed(self, params):
        since = params.get("since")
        if not since:
            return {"changed": True}
        for _event in self._photos().iter_changes(since=since):
            return {"changed": True}
        return {"changed": False}

    def _asset(self, asset_id):
        asset = self.assets.get(asset_id)
        if asset is None:
            asset = self._photos().all.get(asset_id)
            if asset is None:
                raise HelperError("not_found", "That photo is no longer in your library.")
            self.assets[asset_id] = asset
        return asset

    def urls(self, params):
        out = {}
        for asset_id in params.get("ids", []):
            # Fetch again: download URLs are short-lived.
            asset = self._photos().all.get(asset_id)
            if asset is None:
                continue
            self.assets[asset_id] = asset
            out[asset_id] = {k: v["url"] for k, v in asset_resources(asset).items()}
        return out

    def favorite(self, params):
        if not self._asset(params["id"]).set_favorite(bool(params["value"])):
            raise HelperError("failed", "iCloud didn't accept the change.")
        return {}

    def delete(self, params):
        if not self._asset(params["id"]).delete():
            raise HelperError("failed", "iCloud didn't accept the change.")
        self.assets.pop(params["id"], None)
        return {}


# ── fake library (tests) ────────────────────────────────────────────────────

class FakeLibrary:
    """Serves DIR/library.json; resource URLs are file:// paths under DIR."""

    def __init__(self, directory):
        self.directory = directory
        self.log = os.path.join(directory, "calls.log")

    def _record(self, method, params):
        with open(self.log, "a", encoding="utf-8") as handle:
            handle.write(json.dumps({"method": method, "params": params}) + "\n")

    def _library(self):
        with open(os.path.join(self.directory, "library.json"), encoding="utf-8") as handle:
            return json.load(handle)

    def connect(self, params):
        self._record("connect", params)
        if os.path.exists(os.path.join(self.directory, "needs_signin")):
            raise HelperError("needs_signin", "iCloud needs you to sign in again.")
        return {"apple_id": "someone@example.com"}

    def scan(self, params):
        self._record("scan", params)
        library = self._library()
        items = library["assets"]
        delay = library.get("delay", 0)
        for start in range(0, len(items), BATCH):
            time.sleep(delay)
            emit({"event": "assets", "items": items[start:start + BATCH]})
        emit({"event": "albums", "albums": library.get("albums", [])})
        return {"sync_token": library.get("sync_token", "fake-1")}

    def changed(self, params):
        self._record("changed", params)
        return {"changed": params.get("since") != self._library().get("sync_token", "fake-1")}

    def urls(self, params):
        self._record("urls", params)
        by_id = {a["id"]: a for a in self._library()["assets"]}
        return {i: {k: v["url"] for k, v in by_id[i]["resources"].items()} for i in params.get("ids", []) if i in by_id}

    def favorite(self, params):
        self._record("favorite", params)
        return {}

    def delete(self, params):
        self._record("delete", params)
        return {}


def main():
    fake = sys.argv[sys.argv.index("--fake") + 1] if "--fake" in sys.argv else None
    library = FakeLibrary(fake) if fake else ICloudLibrary()
    methods = {name: getattr(library, name) for name in ("connect", "scan", "changed", "urls", "favorite", "delete")}
    for line in sys.stdin:
        try:
            request = json.loads(line)
        except ValueError:
            continue
        request_id = request.get("id")
        method = methods.get(request.get("method"))
        try:
            if method is None:
                raise HelperError("bad_method", f"Unknown method {request.get('method')!r}")
            emit({"id": request_id, "result": method(request.get("params") or {})})
        except HelperError as exc:
            emit({"id": request_id, "error": {"code": exc.code, "message": str(exc)}})
        except Exception as exc:
            emit({"id": request_id, "error": {"code": "failed", "message": f"{type(exc).__name__}: {exc}"}})


if __name__ == "__main__":
    main()
