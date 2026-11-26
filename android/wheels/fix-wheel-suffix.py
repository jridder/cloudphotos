"""Rename maturin's Android extension suffix (-<arch>-android-android.so) to
the one CPython on Android imports (-<arch>-linux-android.so), fixing RECORD."""
import base64, hashlib, re, sys, zipfile, os

def fix(path):
    src = zipfile.ZipFile(path)
    tmp = path + ".tmp"
    out = zipfile.ZipFile(tmp, "w", zipfile.ZIP_DEFLATED)
    record_name, rows = None, []
    for info in src.infolist():
        data = src.read(info)
        name = re.sub(r"-(aarch64|x86_64)-android-android\.so$", r"-\1-linux-android.so", info.filename)
        if name.endswith(".dist-info/RECORD"):
            record_name = name
            continue
        digest = base64.urlsafe_b64encode(hashlib.sha256(data).digest()).rstrip(b"=").decode()
        rows.append(f"{name},sha256={digest},{len(data)}")
        out.writestr(name, data)
    rows.append(f"{record_name},,")
    out.writestr(record_name, "\n".join(rows) + "\n")
    out.close()
    os.replace(tmp, path)

for p in sys.argv[1:]:
    fix(p)
    print(p, [n for n in zipfile.ZipFile(p).namelist() if n.endswith(".so")])
