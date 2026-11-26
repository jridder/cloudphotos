"""Runs photos_helper (the desktop app's link to iCloud) inside the Android
app. Helper.kt calls start() once, then call() for each request, always on
one thread; streamed events go to the listener's onEvent(json)."""
import json
import os
import sys
import time

_methods = None


def _without_usb_security_keys():
    """fido2.hid picks its USB backend by sys.platform and refuses to import
    on "android", which stops pyicloud loading. Its Linux backend only needs
    the standard library, so load that; on a phone it never finds a key."""
    if sys.platform != "android" or "fido2.hid" in sys.modules:
        return
    sys.platform = "linux"
    try:
        import fido2.hid  # noqa: F401
    finally:
        sys.platform = "android"


def start(listener, home, timezone=None, fake_dir=None):
    global _methods
    os.environ["HOME"] = home
    if timezone:
        # tzlocal (via pyicloud) reads TZ; Android has no /etc/localtime.
        os.environ["TZ"] = timezone
        if hasattr(time, "tzset"):  # not on Android's Python
            time.tzset()
    _without_usb_security_keys()

    import photos_helper

    photos_helper.emit = lambda message: listener.onEvent(json.dumps(message))
    library = photos_helper.FakeLibrary(fake_dir) if fake_dir else photos_helper.ICloudLibrary()
    _methods = photos_helper.method_table(library)


def call(method, params_json):
    import photos_helper

    request = {"id": 0, "method": method, "params": json.loads(params_json or "{}")}
    return json.dumps(photos_helper.handle(_methods, request))


def selftest():
    """Loads everything a real sign-in needs, without signing in (debug
    builds: `adb shell am start -n com.jridder.cloudphotos/.MainActivity --ez selftest true`)."""
    _without_usb_security_keys()
    import importlib.metadata as metadata

    import cryptography.hazmat.primitives.asymmetric.ec  # noqa: F401
    import pydantic_core  # noqa: F401
    import pyicloud  # noqa: F401
    from pyicloud.services.photos_cloudkit import client  # noqa: F401
    import pyicloud.hsa2_bridge  # noqa: F401
    import requests

    requests.head("https://www.icloud.com", timeout=10)
    return ", ".join(f"{name} {metadata.version(name)}" for name in ("pyicloud", "pydantic-core", "cryptography", "requests"))
