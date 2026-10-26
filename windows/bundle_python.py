"""Copy the helper's Python packages into a bundle: pyicloud and PyYAML and
everything they require (by their metadata), from the dev venv and MSYS2.

Usage: python bundle_python.py SITE_PACKAGES_OUT
Run with the dev venv's Python (windows/setup-dev.ps1).
"""
import importlib.metadata as md
import os
import shutil
import sys

from pip._vendor.packaging.requirements import Requirement

ROOTS = ["pyicloud", "PyYAML"]


def closure(names):
    seen, todo = {}, list(names)
    while todo:
        name = todo.pop()
        key = name.lower().replace("_", "-")
        if key in seen:
            continue
        dist = md.distribution(name)
        seen[key] = dist
        for spec in dist.requires or []:
            req = Requirement(spec)
            if req.marker is None or req.marker.evaluate({"extra": ""}):
                todo.append(req.name)
    return seen.values()


def main():
    out = sys.argv[1]
    os.makedirs(out, exist_ok=True)
    for dist in sorted(closure(ROOTS), key=lambda d: d.metadata["Name"].lower()):
        copied = 0
        for path in dist.files or []:
            source = dist.locate_file(path)
            if path.parts[0] == ".." or "__pycache__" in path.parts or not source.is_file():
                continue  # scripts in bin/, bytecode
            target = os.path.join(out, *path.parts)
            os.makedirs(os.path.dirname(target), exist_ok=True)
            shutil.copy2(source, target)
            copied += 1
        print(f"{dist.metadata['Name']} {dist.version}: {copied} files")


if __name__ == "__main__":
    main()
