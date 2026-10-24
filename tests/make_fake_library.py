#!/usr/bin/env python3
"""Build a fake iCloud library for `photos_helper.py --fake DIR`.

Usage: make_fake_library.py DIR [COUNT]

Each asset gets solid-colour thumb/preview/original JPEGs so screenshots
show something; videos and Live Photos reuse a tiny placeholder .mov.
"""
import colorsys
import datetime
import json
import os
import random
import sys

import gi

gi.require_version("GdkPixbuf", "2.0")
from gi.repository import GdkPixbuf


def jpeg(path, width, height, hue):
    if os.path.exists(path):
        return
    r, g, b = (int(c * 255) for c in colorsys.hsv_to_rgb(hue, 0.55, 0.85))
    pixbuf = GdkPixbuf.Pixbuf.new(GdkPixbuf.Colorspace.RGB, False, 8, width, height)
    pixbuf.fill((r << 24) | (g << 16) | (b << 8) | 0xFF)
    # A darker band so orientation and cropping are visible.
    band = GdkPixbuf.Pixbuf.new(GdkPixbuf.Colorspace.RGB, False, 8, width, max(1, height // 5))
    band.fill(((r // 2) << 24) | ((g // 2) << 16) | ((b // 2) << 8) | 0xFF)
    band.copy_area(0, 0, width, band.get_height(), pixbuf, 0, height - band.get_height())
    pixbuf.savev(path, "jpeg", ["quality"], ["85"])


def main():
    out = os.path.abspath(sys.argv[1])
    count = int(sys.argv[2]) if len(sys.argv) > 2 else 300
    media = os.path.join(out, "media")
    os.makedirs(media, exist_ok=True)
    movie = os.path.join(media, "clip.mov")
    if not os.path.exists(movie):
        with open(movie, "wb") as handle:
            handle.write(os.urandom(4096))

    rng = random.Random(42)
    start = datetime.datetime(2022, 3, 1).timestamp()
    end = datetime.datetime(2026, 10, 20).timestamp()
    assets = []
    for i in range(count):
        taken = rng.uniform(start, end)
        portrait = rng.random() < 0.35
        width, height = (3024, 4032) if portrait else (4032, 3024)
        kind = "video" if rng.random() < 0.08 else "photo"
        live = kind == "photo" and rng.random() < 0.3
        hue = (i * 0.137) % 1.0
        base = f"asset{i:04d}"
        tw, th = (270, 360) if portrait else (360, 270)
        pw, ph = (900, 1200) if portrait else (1200, 900)
        jpeg(os.path.join(media, f"{base}-thumb.jpg"), tw, th, hue)
        jpeg(os.path.join(media, f"{base}-preview.jpg"), pw, ph, hue)
        jpeg(os.path.join(media, f"{base}-original.jpg"), pw * 2, ph * 2, hue)
        url = lambda name: "file://" + os.path.join(media, name)  # noqa: E731
        resources = {
            "thumb": {"url": url(f"{base}-thumb.jpg"), "size": 20000, "width": tw, "height": th, "type": "public.jpeg"},
            "preview": {"url": url(f"{base}-preview.jpg"), "size": 200000, "width": pw, "height": ph, "type": "public.jpeg"},
            "original": {"url": url(f"{base}-original.jpg"), "size": 3_000_000, "width": width, "height": height,
                         "type": "public.jpeg"},
        }
        if kind == "video":
            resources["video"] = {"url": "file://" + movie, "size": 4096, "width": 1920, "height": 1080,
                                  "type": "com.apple.quicktime-movie"}
        if live:
            resources["live"] = {"url": "file://" + movie, "size": 4096, "width": 1440, "height": 1080,
                                 "type": "com.apple.quicktime-movie"}
        assets.append({
            "id": f"FAKE-{i:04d}",
            "filename": f"IMG_{1000 + i:04d}.{'MOV' if kind == 'video' else 'HEIC'}",
            "kind": kind,
            "taken": taken,
            "added": taken + 60,
            "width": width,
            "height": height,
            "size": 3_000_000,
            "live": live,
            "favorite": rng.random() < 0.1,
            "resources": resources,
        })

    ids = [a["id"] for a in assets]
    albums = [
        {"id": "Favorites", "name": "Favorites", "kind": "smart", "assets": [a["id"] for a in assets if a["favorite"]]},
        {"id": "Videos", "name": "Videos", "kind": "smart", "assets": [a["id"] for a in assets if a["kind"] == "video"]},
        {"id": "Live", "name": "Live", "kind": "smart", "assets": [a["id"] for a in assets if a["live"]]},
        {"id": "Screenshots", "name": "Screenshots", "kind": "smart", "assets": rng.sample(ids, 12)},
        {"id": "ALB-1", "name": "Iceland", "kind": "user", "assets": rng.sample(ids, 40)},
        {"id": "ALB-2", "name": "Trips/Banff", "kind": "user", "assets": rng.sample(ids, 25)},
        {"id": "ALB-3", "name": "Family", "kind": "user", "assets": rng.sample(ids, 60)},
    ]
    with open(os.path.join(out, "library.json"), "w", encoding="utf-8") as handle:
        json.dump({"sync_token": "fake-1", "assets": assets, "albums": albums}, handle)
    print(f"{len(assets)} assets, {len(albums)} albums in {out}")


if __name__ == "__main__":
    main()
