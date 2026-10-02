#!/usr/bin/env python3
"""Packs an .iconset folder into .icns with the standard library only.

macOS's `iconutil -c icns` is the reference tool; this writer exists so the committed
icns can be regenerated (and checked) on Linux. Modern icns entries hold PNG data as-is.
Usage: make_icns.py ZJ.iconset out.icns   |   make_icns.py --check out.icns
"""
import struct
import sys
from pathlib import Path

# (file name in the iconset, icns type, expected pixel size)
ENTRIES = [
    ("icon_16x16.png", b"icp4", 16),
    ("icon_32x32.png", b"icp5", 32),
    ("icon_16x16@2x.png", b"ic11", 32),
    ("icon_128x128.png", b"ic07", 128),
    ("icon_32x32@2x.png", b"ic12", 64),
    ("icon_256x256.png", b"ic08", 256),
    ("icon_128x128@2x.png", b"ic13", 256),
    ("icon_512x512.png", b"ic09", 512),
    ("icon_256x256@2x.png", b"ic14", 512),
    ("icon_512x512@2x.png", b"ic10", 1024),
]
PNG = b"\x89PNG\r\n\x1a\n"


def png_size(data):
    if not data.startswith(PNG) or data[12:16] != b"IHDR":
        raise ValueError("not a PNG")
    return struct.unpack(">II", data[16:24])


def pack(iconset, output):
    body = b""
    for name, kind, pixels in ENTRIES:
        data = (iconset / name).read_bytes()
        if png_size(data) != (pixels, pixels):
            raise ValueError(f"{name}: expected {pixels}x{pixels}, got {png_size(data)}")
        body += kind + struct.pack(">I", len(data) + 8) + data
    output.write_bytes(b"icns" + struct.pack(">I", len(body) + 8) + body)


def check(path):
    data = path.read_bytes()
    if data[:4] != b"icns" or struct.unpack(">I", data[4:8])[0] != len(data):
        raise ValueError("bad icns header")
    found, offset = {}, 8
    while offset < len(data):
        kind, length = data[offset:offset + 4], struct.unpack(">I", data[offset + 4:offset + 8])[0]
        found[kind] = png_size(data[offset + 8:offset + length])
        offset += length
    for _, kind, pixels in ENTRIES:
        if found.get(kind) != (pixels, pixels):
            raise ValueError(f"{kind.decode()}: {found.get(kind)}")
    print(f"ok: {len(found)} entries, {len(data)} bytes")


if __name__ == "__main__":
    if sys.argv[1] == "--check":
        check(Path(sys.argv[2]))
    else:
        pack(Path(sys.argv[1]), Path(sys.argv[2]))
        check(Path(sys.argv[2]))
