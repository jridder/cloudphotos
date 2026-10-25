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
  scan     {}                                  -> {sync_token, recently_deleted: [{id, expires}]};
                                                  emits "assets" and "albums"
  changes  {since}                             -> {sync_token, assets, trashed: [{id, expires}], removed: [id],
                                                  albums_changed} since the token, or {full_scan: true}
  urls     {ids: [...], masters: {id: master}} -> {id: {key: url}} with fresh download URLs
  favorite {id, master, value}                 -> {}
  delete   {id, master}                        -> {}   (moves to Recently Deleted)
  recover  {id, master}                        -> {}   (back from Recently Deleted)
  purge    {id, master}                        -> {}   (removed from Recently Deleted for good)
  upload   {path}                              -> {asset}  (a new photo, e.g. an edited copy)

`--fake DIR` serves DIR/library.json instead, for tests and screenshots.
"""
import json
import os
import shutil
import sys
import time

SMART_ALBUMS = ("Favorites", "Screenshots", "Videos", "Live", "Panoramas", "Slo-mo", "Time-lapse", "Bursts")
DELETED_ALBUM = "Recently Deleted"
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
        # Apple's fingerprint of the file; changes when the photo is edited.
        "checksum": resource.checksum,
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
    # What the local library keeps: JPEG thumb and preview for display, and
    # the originals (a video's original is the movie; a Live Photo's motion
    # part is its original video).
    for name, keys in (
        ("thumb", ("thumb_image",) if movie else ("thumb",)),
        ("preview", ("medium_image", "thumb_image") if movie else ("medium", "thumb")),
        ("original", ("original",)),
        ("live", () if movie else ("original_video",)),
    ):
        res = pick(r, *keys)
        if res is not None:
            out[name] = resource_dict(res)
    return out


def is_favorite(asset):
    from pyicloud.services.photos_cloudkit.mappers import record_field_value

    return bool(record_field_value(asset.asset_record, "isFavorite"))


def asset_details(asset):
    """What Apple's records say beyond the basics: caption, keywords,
    location, time zone, video length, HDR/burst. Best effort: a field that
    can't be decoded is simply left out."""
    from pyicloud.services.photos_cloudkit.mappers import record_field_value

    record = asset.asset_record
    out = {}
    try:
        from pyicloud.services.photos_cloudkit.materialize import build_xmp_metadata

        meta = build_xmp_metadata(record)
    except Exception:
        meta = None
    if meta is not None:
        for key, value in (
            ("caption", meta.title),
            ("description", meta.description),
            ("keywords", meta.keywords),
            ("latitude", meta.gps_latitude),
            ("longitude", meta.gps_longitude),
            ("altitude", meta.gps_altitude),
            ("speed", meta.gps_speed),
        ):
            if value not in (None, "", []):
                out[key] = value
    for key, field_name in (
        ("timezone_offset", "timeZoneOffset"),
        ("duration", "duration"),
        ("hdr", "assetHDRType"),
        ("burst", "burstId"),
        ("edited", "adjustmentRenderType"),
        ("hidden", "isHidden"),
    ):
        try:
            value = record_field_value(record, field_name)
        except Exception:
            value = None
        if value not in (None, "", 0):
            out[key] = value
    return out


def asset_dict(asset):
    resources = asset_resources(asset)
    if "original" not in resources:
        return None
    taken = asset.asset_date or asset.created
    added = asset.added_date or taken
    width, height = asset.dimensions
    return {
        "id": asset.id,
        "master": asset.master_id,
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
        "details": asset_details(asset),
    }


def expiry(asset):
    """When iCloud removes an item in Recently Deleted for good (Unix
    seconds): its dateExpunged, which iCloud sets 30 days after the delete.
    (The record's expirationTime is a later, internal cleanup date.)"""
    value = record_value(getattr(asset, "_asset_record", None), "dateExpunged")
    if value is None:
        return None
    if hasattr(value, "timestamp"):
        return value.timestamp()
    value = float(value)
    return value / 1000 if value > 1e11 else value


def record_value(record, field):
    """A field's value from a CKRecord or a raw record dict."""
    if isinstance(record, dict):
        return (record.get("fields", {}).get(field) or {}).get("value")
    try:
        return record.fields.get_value(field)
    except (AttributeError, KeyError):
        return None


# ── iCloud ──────────────────────────────────────────────────────────────────

class ICloudLibrary:
    def __init__(self):
        self.api = None

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
        for asset in photos.all:
            item = asset_dict(asset)
            if item is None:
                continue
            batch.append(item)
            if len(batch) >= BATCH:
                emit({"event": "assets", "items": batch})
                batch = []
        if batch:
            emit({"event": "assets", "items": batch})

        albums = []
        recently_deleted = []
        for album in photos.albums:
            kind = type(album).__name__
            if kind == "PhotoAlbumFolder":
                continue
            if kind == "SmartPhotoAlbum" and album.name == DELETED_ALBUM:
                # Kept locally until Apple purges them; mirrored away after.
                recently_deleted = [{"id": a.id, "expires": expiry(a)} for a in album]
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
        return {"sync_token": token, "recently_deleted": recently_deleted}

    def changes(self, params):
        """What changed in iCloud since `since` (a sync token): items added
        or edited, moved to or out of Recently Deleted, or gone for good.
        Album changes are only flagged; the app rescans for those."""
        since = params.get("since")
        if not since:
            return {"full_scan": True}
        changed, gone, albums_changed = [], set(), False
        try:
            records, token = self._zone_changes(since)
        except Exception as exc:
            # An expired token, say: start over with a full listing.
            return {"full_scan": True, "reason": str(exc)}
        for record in records:
            name, kind = record.get("recordName"), record.get("recordType")
            if not name or record.get("serverErrorCode"):
                continue
            if record.get("deleted"):
                gone.add(name)  # a tombstone: removed for good
            elif kind == "CPLAsset":
                if name not in changed:
                    changed.append(name)
            elif kind in ("CPLAlbum", "CPLContainerRelation"):
                albums_changed = True

        assets, trashed = [], []
        for start in range(0, len(changed), BATCH):
            ids = changed[start:start + BATCH]
            found = self._lookup(ids, {})
            for asset_id in ids:
                asset = found.get(asset_id)
                record = getattr(asset, "_asset_record", None)
                if asset is None or record_value(record, "isHidden"):
                    gone.add(asset_id)  # hidden items aren't part of the library
                elif record_value(record, "isDeleted"):
                    trashed.append({"id": asset_id, "expires": expiry(asset)})
                else:
                    item = asset_dict(asset)
                    if item is not None:
                        assets.append(item)
        return {"sync_token": token, "assets": assets, "trashed": trashed,
                "removed": sorted(gone), "albums_changed": albums_changed}

    def _zone_changes(self, since):
        """The photo library's records changed since `since` (raw CloudKit
        JSON: name, type, deleted) and the new sync token. pyicloud's
        iter_changes validates the reply against its models, which some real
        libraries' replies fail, so this reads the JSON directly."""
        from pyicloud.services.photos_cloudkit.constants import PRIMARY_ZONE

        http = self._photos().private_client._client._http
        token, records = since, []
        while True:
            data = http.post("/changes/zone", {"zones": [{"zoneID": dict(PRIMARY_ZONE), "syncToken": token, "desiredKeys": ["isDeleted"]}]})
            zones = data.get("zones") or []
            if not zones:
                break
            zone = zones[0]
            if zone.get("serverErrorCode"):
                raise RuntimeError(f"{zone['serverErrorCode']}: {zone.get('reason', '')}")
            records.extend(zone.get("records") or [])
            token = zone.get("syncToken") or token
            if not zone.get("moreComing"):
                break
        return records, token

    def _asset(self, asset_id, master=None):
        """The current record for one item, for changing it: looked up fresh
        so its change tag is up to date (a stale one makes iCloud refuse)."""
        asset = self._lookup([asset_id], {asset_id: master} if master else {}).get(asset_id)
        if asset is None:
            raise HelperError("not_found", "That photo is no longer in your library.")
        return asset

    def _lookup(self, ids, masters):
        """PhotoAssets for several items in one CloudKit request.

        Looks the asset and master records up by name (records/lookup), which
        answers in well under a second; pyicloud's per-photo get() runs an
        index query that took ~55 s per item on a real library.
        """
        from pyicloud.common.cloudkit.models import CKRecord, CKZoneIDReq
        from pyicloud.services.photos_cloudkit.constants import PRIMARY_ZONE
        from pyicloud.services.photos_cloudkit.service import PhotoAsset

        photos = self._photos()
        ids = list(ids)
        masters = dict(masters or {})
        lookup = photos.private_client.lookup

        def fetch(names):
            response = lookup(record_names=names, zone_id=CKZoneIDReq(**PRIMARY_ZONE))
            return {r.recordName: r for r in response.records if isinstance(r, CKRecord)}

        records = fetch(ids + [masters[i] for i in ids if masters.get(i)])
        missing = [i for i in ids if not masters.get(i) and i in records]
        for i in missing:
            ref = records[i].fields.get_value("masterRef")
            name = ref.get("recordName") if isinstance(ref, dict) else getattr(ref, "recordName", None)
            if name:
                masters[i] = name
        if missing:
            records.update(fetch([masters[i] for i in missing if masters.get(i)]))

        out = {}
        for i in ids:
            asset_record, master_record = records.get(i), records.get(masters.get(i, ""))
            if asset_record is None or master_record is None:
                continue
            out[i] = PhotoAsset(photos, master_record=master_record, asset_record=asset_record)
        return out

    def urls(self, params):
        """Fresh download URLs for several items in one CloudKit request."""
        assets = self._lookup(params.get("ids", []), params.get("masters"))
        return {i: {k: v["url"] for k, v in asset_resources(asset).items()} for i, asset in assets.items()}

    def favorite(self, params):
        if not self._asset(params["id"], params.get("master")).set_favorite(bool(params["value"])):
            raise HelperError("failed", "iCloud didn't accept the change.")
        return {}

    def delete(self, params):
        if not self._asset(params["id"], params.get("master")).delete():
            raise HelperError("failed", "iCloud didn't accept the change.")
        return {}

    def recover(self, params):
        asset = self._asset(params["id"], params.get("master"))
        self._set_flags(asset._asset_record, {"isDeleted": 0})
        # iCloud marks the master deleted along with the asset; if it
        # doesn't bring it back by itself, do so too.
        fresh = self._asset(params["id"], params.get("master"))
        if record_value(fresh._master_record, "isDeleted"):
            self._set_flags(fresh._master_record, {"isDeleted": 0})
        return {}

    def purge(self, params):
        asset = self._asset(params["id"], params.get("master"))
        self._set_flags(asset._asset_record, {"isExpunged": 1})
        return {}

    def _set_flags(self, record, values):
        """Set INT64 fields on one record. CloudKit's refusals raise."""
        from pyicloud.common.cloudkit import CKModifyOperation, CKWriteRecord, CKZoneID, CKZoneIDReq
        from pyicloud.services.photos_cloudkit import service as svc

        zone = svc.record_zone(record) or svc.PRIMARY_ZONE
        op = CKModifyOperation(
            operationType="update",
            record=CKWriteRecord(
                recordName=svc.record_name(record),
                recordType=svc.record_record_type(record),
                recordChangeTag=svc._record_change_tag(record),
                fields={k: {"type": "INT64", "value": v} for k, v in values.items()},
                zoneID=CKZoneID(**zone),
            ),
        )
        zone_id = CKZoneIDReq(zoneName=zone["zoneName"], ownerRecordName=zone.get("ownerRecordName"), zoneType=zone.get("zoneType"))
        self._photos().private_client.modify(operations=[op], zone_id=zone_id, atomic=True)

    def upload(self, params):
        asset = self._photos().upload(params["path"])
        if asset is None:
            raise HelperError("failed", "iCloud didn't accept the upload.")
        return {"asset": asset_dict(asset)}


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

    scans = 0

    def scan(self, params):
        self._record("scan", params)
        # "{scan}" in URLs becomes the scan number, so tests can expire old links.
        FakeLibrary.scans += 1
        with open(os.path.join(self.directory, "library.json"), encoding="utf-8") as handle:
            library = json.loads(handle.read().replace("{scan}", str(FakeLibrary.scans)))
        items = library["assets"]
        delay = library.get("delay", 0)
        for start in range(0, len(items), BATCH):
            time.sleep(delay)
            emit({"event": "assets", "items": items[start:start + BATCH]})
        emit({"event": "albums", "albums": library.get("albums", [])})
        return {"sync_token": library.get("sync_token", "fake-1"),
                "recently_deleted": library.get("recently_deleted", [])}

    def changes(self, params):
        """library.json's "changes" (assets/trashed/removed/albums_changed)
        once its sync_token differs from `since`; nothing otherwise."""
        self._record("changes", params)
        library = self._library()
        token = library.get("sync_token", "fake-1")
        if params.get("since") == token:
            return {"sync_token": token, "assets": [], "trashed": [], "removed": [], "albums_changed": False}
        if "changes" not in library:
            return {"full_scan": True}
        return {"sync_token": token, "assets": [], "trashed": [], "removed": [], "albums_changed": False,
                **library["changes"]}

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

    def recover(self, params):
        self._record("recover", params)
        return {}

    def purge(self, params):
        self._record("purge", params)
        return {}

    def upload(self, params):
        self._record("upload", params)
        path = params["path"]
        url = "file://" + path
        size = os.path.getsize(path)
        res = {"url": url, "size": size, "width": 1, "height": 1, "type": "public.jpeg", "checksum": f"up-{size}"}
        return {"asset": {
            "id": f"UPLOAD-{int(time.time() * 1000)}", "master": None, "filename": os.path.basename(path),
            "kind": "photo", "taken": time.time(), "added": time.time(), "width": 1, "height": 1, "size": size,
            "live": False, "favorite": False, "details": {},
            "resources": {"thumb": dict(res), "preview": dict(res), "original": dict(res)},
        }}


def main():
    fake = sys.argv[sys.argv.index("--fake") + 1] if "--fake" in sys.argv else None
    library = FakeLibrary(fake) if fake else ICloudLibrary()
    methods = {name: getattr(library, name) for name in ("connect", "scan", "changes", "urls", "favorite", "delete", "recover", "purge", "upload")}
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
