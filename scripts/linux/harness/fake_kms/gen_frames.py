#!/usr/bin/env python3
"""Real-time raw yuv420p frame source for the fake kms helper. Each frame carries the wall
clock as a 40-block binary strip (epoch ms mod 2^40, block = 16 px, bit 39 first, white=1)
along the top-left, the same encoding rec_clock.py draws on the real screen, so a test can read
a delivered clip's first/last frame back as "which instant was this" (decode_wall below).

  gen_frames.py WxH FPS      # raw frames on stdout, paced by the wall clock
"""
import sys, time

NB, BLK = 40, 16


def decode_wall(png, ref_epoch_s):
    """Wall-clock epoch (s) encoded in a frame (PNG path), resolved against a nearby reference."""
    from PIL import Image
    im = Image.open(png).convert("L")
    v = 0
    for i in range(NB):
        v = (v << 1) | (1 if im.getpixel((i * BLK + BLK // 2, BLK // 2)) > 128 else 0)
    mod = 1 << NB
    ref_ms = int(ref_epoch_s * 1000)
    base = ref_ms - (ref_ms % mod)
    return min((base - mod + v, base + v, base + mod + v), key=lambda c: abs(c - ref_ms)) / 1000.0


def main():
    w, h = map(int, sys.argv[1].split("x"))
    fps = float(sys.argv[2])
    assert w >= NB * BLK, "strip needs 640 px"
    out = sys.stdout.buffer
    bg = bytes([100]) * (w * h)
    uv = bytes([128]) * (w * h // 2)
    t0 = time.time()
    n = 0
    while True:
        due = t0 + n / fps
        d = due - time.time()
        if d > 0:
            time.sleep(d)
        v = int(time.time() * 1000) % (1 << NB)
        row = bytearray(bytes([100]) * w)
        for i in range(NB):
            row[i * BLK:(i + 1) * BLK] = bytes([235 if (v >> (NB - 1 - i)) & 1 else 16]) * BLK
        y = bytearray(bg)
        for r in range(BLK):
            y[r * w:(r + 1) * w] = row
        try:
            out.write(y)
            out.write(uv)
            out.flush()
        except (BrokenPipeError, OSError):
            return
        n += 1


if __name__ == "__main__":
    main()
