"""Draws the launcher's icon (a pixel-art grass block in a gold frame) without any image library:
icon.rgba (64x64 raw RGBA, the window icon), icon.ico (16..256 px, the exe icon) and icon.png
(256 px, the Linux AppImage's icon).
    python assets/make_icon.py
"""
import os
import random
import struct
import zlib

HERE = os.path.dirname(os.path.abspath(__file__))
random.seed(7)

GOLD = (255, 205, 80, 255)
GOLD_DARK = (176, 128, 30, 255)
GRASS = [(106, 170, 64, 255), (92, 152, 54, 255), (122, 186, 76, 255)]
DIRT = [(134, 96, 67, 255), (116, 82, 56, 255), (150, 108, 76, 255), (96, 68, 46, 255)]


def art():
    """16x16: a one-pixel gold frame around a grass block face."""
    px = [[(0, 0, 0, 0)] * 16 for _ in range(16)]
    # Grass reaches down unevenly, as on a Minecraft block's side.
    drip = [5, 6, 5, 4, 6, 7, 5, 5, 6, 4, 5, 7, 6, 5, 4, 6]
    for y in range(16):
        for x in range(16):
            if x in (0, 15) or y in (0, 15):
                px[y][x] = GOLD if (x + y) % 5 else GOLD_DARK
            elif y < drip[x]:
                px[y][x] = random.choice(GRASS)
            else:
                px[y][x] = random.choice(DIRT)
    # Rounded corners.
    for x, y in ((0, 0), (15, 0), (0, 15), (15, 15)):
        px[y][x] = (0, 0, 0, 0)
    return px


def scaled(px, size):
    return [[px[y * 16 // size][x * 16 // size] for x in range(size)] for y in range(size)]


def bmp_entry(img):
    """An ICO's BMP image: BITMAPINFOHEADER, bottom-up BGRA rows, then the AND mask."""
    n = len(img)
    header = struct.pack("<IiiHHIIiiII", 40, n, n * 2, 1, 32, 0, n * n * 4, 0, 0, 0, 0)
    rows = b"".join(bytes([c[2], c[1], c[0], c[3]]) for row in reversed(img) for c in row)
    mask_row = ((n + 31) // 32) * 4
    return header + rows + b"\0" * (mask_row * n)


px = art()
with open(os.path.join(HERE, "icon.rgba"), "wb") as f:
    f.write(b"".join(bytes(c) for row in scaled(px, 64) for c in row))

sizes = [16, 32, 48, 64, 128, 256]
images = [bmp_entry(scaled(px, s)) for s in sizes]
offset = 6 + 16 * len(sizes)
out = struct.pack("<HHH", 0, 1, len(sizes))
for s, data in zip(sizes, images):
    out += struct.pack("<BBBBHHII", s % 256, s % 256, 0, 0, 1, 32, len(data), offset)
    offset += len(data)
with open(os.path.join(HERE, "icon.ico"), "wb") as f:
    f.write(out + b"".join(images))


def png(img):
    """A PNG: RGBA rows, each behind a 0 (no filter) byte, deflated."""
    n = len(img)
    raw = b"".join(b"\0" + b"".join(bytes(c) for c in row) for row in img)

    def chunk(kind, data):
        return struct.pack(">I", len(data)) + kind + data + struct.pack(">I", zlib.crc32(kind + data))

    return b"\x89PNG\r\n\x1a\n" + chunk(b"IHDR", struct.pack(">IIBBBBB", n, n, 8, 6, 0, 0, 0)) + chunk(b"IDAT", zlib.compress(raw, 9)) + chunk(b"IEND", b"")


with open(os.path.join(HERE, "icon.png"), "wb") as f:
    f.write(png(scaled(px, 256)))
print("icon.rgba, icon.ico, icon.png written")
