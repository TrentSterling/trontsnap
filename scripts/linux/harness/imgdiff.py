#!/usr/bin/env python3
"""imgdiff.py A.png B.png [--out diff.png] [--thresh 24]
Counts pixels that differ by more than --thresh in any channel and prints the
count + bounding box. Exit 0 if they differ, 1 if identical (so `imgdiff.py a b`
reads as "did anything change"). --out writes a red-on-dark change mask."""
import argparse, sys
from PIL import Image, ImageChops

ap = argparse.ArgumentParser()
ap.add_argument("a"); ap.add_argument("b")
ap.add_argument("--out"); ap.add_argument("--thresh", type=int, default=24)
a = ap.parse_args()
A, B = Image.open(a.a).convert("RGB"), Image.open(a.b).convert("RGB")
if A.size != B.size:
    sys.exit(f"size mismatch {A.size} vs {B.size}")
d = ImageChops.difference(A, B)
mask = d.convert("L").point(lambda v: 255 if v > a.thresh else 0)
n = sum(1 for v in mask.getdata() if v)
bbox = mask.getbbox()
print(f"changed_pixels={n} bbox={bbox}")
if a.out:
    base = B.point(lambda v: v // 3)
    red = Image.new("RGB", B.size, (255, 40, 40))
    Image.composite(red, base, mask).save(a.out)
    print(a.out)
sys.exit(0 if n else 1)
