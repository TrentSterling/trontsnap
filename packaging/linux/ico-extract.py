#!/usr/bin/env python3
"""Unpack the PNG frames of an .ico into OUTDIR/<size>.png.

assets/icon.ico already carries the rainbow icon at 16/24/32/48/64/128/256 as
PNG-in-ICO, so the packaging gets real hicolor sizes with no ImageMagick or
Pillow. Frames stored as BMP are skipped (this repo's icon has none).

usage: python3 -I ico-extract.py ICON.ico OUTDIR
"""
import os
import struct
import sys


def main(ico, outdir):
    data = open(ico, "rb").read()
    _reserved, kind, count = struct.unpack_from("<HHH", data, 0)
    if kind != 1:
        sys.exit("ico-extract: %s is not an .ico" % ico)
    os.makedirs(outdir, exist_ok=True)
    written = 0
    for i in range(count):
        w, h, _c, _r, _planes, _bpp, size, off = struct.unpack_from("<BBBBHHII", data, 6 + 16 * i)
        blob = data[off:off + size]
        # An ICO stores 256 as 0.
        w, h = w or 256, h or 256
        if w != h or not blob.startswith(b"\x89PNG\r\n\x1a\n"):
            continue
        with open(os.path.join(outdir, "%d.png" % w), "wb") as f:
            f.write(blob)
        written += 1
    if not written:
        sys.exit("ico-extract: no PNG frames in %s" % ico)


if len(sys.argv) != 3:
    sys.exit(__doc__)
main(sys.argv[1], sys.argv[2])
