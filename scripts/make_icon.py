#!/usr/bin/env python3
"""Zeichnet das Ampel-Icon und schreibt assets/icon.ico und assets/icon-64.rgba.
Ohne Zusatzpakete: Pixel per Hand, PNG per zlib, ICO mit eingebetteten PNGs."""
import math
import struct
import zlib
from pathlib import Path

ASSETS = Path(__file__).resolve().parent.parent / "assets"
HOUSING = (32, 32, 38)
LIGHTS = [(235, 45, 45), (245, 190, 20), (40, 200, 90)]
SUPERSAMPLE = 4


def rounded_rect_alpha(x, y, left, top, right, bottom, radius):
    cx = min(max(x, left + radius), right - radius)
    cy = min(max(y, top + radius), bottom - radius)
    return 1.0 if (x - cx) ** 2 + (y - cy) ** 2 <= radius ** 2 else 0.0


def render(size):
    pixels = bytearray(size * size * 4)
    n = SUPERSAMPLE
    for py in range(size):
        for px in range(size):
            r = g = b = a = 0.0
            for sy in range(n):
                for sx in range(n):
                    # Koordinaten im Bereich 0..1
                    x = (px + (sx + 0.5) / n) / size
                    y = (py + (sy + 0.5) / n) / size
                    if not rounded_rect_alpha(x, y, 0.24, 0.02, 0.76, 0.98, 0.14):
                        continue
                    color = HOUSING
                    for i, light in enumerate(LIGHTS):
                        if math.hypot(x - 0.5, y - (0.19 + i * 0.31)) <= 0.135:
                            color = light
                    r += color[0]
                    g += color[1]
                    b += color[2]
                    a += 1
            if a:
                i = (py * size + px) * 4
                pixels[i:i + 4] = bytes((round(r / a), round(g / a), round(b / a), round(255 * a / n / n)))
    return bytes(pixels)


def png(size, rgba):
    def chunk(kind, data):
        return struct.pack(">I", len(data)) + kind + data + struct.pack(">I", zlib.crc32(kind + data))

    rows = b"".join(b"\0" + rgba[y * size * 4:(y + 1) * size * 4] for y in range(size))
    header = struct.pack(">IIBBBBB", size, size, 8, 6, 0, 0, 0)
    return b"\x89PNG\r\n\x1a\n" + chunk(b"IHDR", header) + chunk(b"IDAT", zlib.compress(rows, 9)) + chunk(b"IEND", b"")


def main():
    ASSETS.mkdir(exist_ok=True)
    sizes = [16, 24, 32, 48, 64, 128, 256]
    images = [png(s, render(s)) for s in sizes]

    offset = 6 + 16 * len(sizes)
    header = struct.pack("<HHH", 0, 1, len(sizes))
    entries = b""
    for s, data in zip(sizes, images):
        dim = 0 if s == 256 else s
        entries += struct.pack("<BBBBHHII", dim, dim, 0, 0, 1, 32, len(data), offset)
        offset += len(data)
    (ASSETS / "icon.ico").write_bytes(header + entries + b"".join(images))
    (ASSETS / "icon-64.rgba").write_bytes(render(64))
    (ASSETS / "icon-256.png").write_bytes(images[-1])


if __name__ == "__main__":
    main()
