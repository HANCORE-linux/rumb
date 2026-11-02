"""Appends glyphs to assets/glyphs.bin without touching the ones it has.

usage: python3 add_glyphs.py JetBrainsMono-Regular.ttf glyphs.bin "λ∘≡"
"""
import struct
import sys

from PIL import Image, ImageDraw, ImageFont

font_path, path, extra = sys.argv[1], sys.argv[2], sys.argv[3]
old = open(path, "rb").read()
out = bytearray([old[0]])
at = 1
for _ in range(old[0]):
    size, advance, height, ascent, count = struct.unpack_from("<BBBBH", old, at)
    codes = [struct.unpack_from("<I", old, at + 6 + 4 * k)[0] for k in range(count)]
    start = at + 6 + 4 * count
    end = start + advance * height * count
    font = ImageFont.truetype(font_path, size)
    add = [ch for ch in dict.fromkeys(extra) if ord(ch) not in codes]
    data = bytearray(old[start:end])
    for ch in add:
        image = Image.new("L", (advance, height), 0)
        ImageDraw.Draw(image).text((0, ascent), ch, font=font, fill=255, anchor="ls")
        data += image.tobytes()
        codes.append(ord(ch))
    out += struct.pack("<BBBBH", size, advance, height, ascent, len(codes))
    out += b"".join(struct.pack("<I", code) for code in codes) + data
    at = end
open(path, "wb").write(out)
print(len(old), "->", len(out))
