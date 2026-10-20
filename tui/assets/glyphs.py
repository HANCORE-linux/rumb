import struct
import sys

from PIL import Image, ImageDraw, ImageFont

path, out = sys.argv[1], sys.argv[2]
chars = list(dict.fromkeys([chr(c) for c in range(32, 127)] + list("›▸·…→←↑↓▌✓≠")))
blocks = []
for size in range(7, 17):
    font = ImageFont.truetype(path, size)
    ascent, descent = font.getmetrics()
    advance = round(font.getlength("M"))
    height = ascent + descent
    data = bytearray()
    for ch in chars:
        image = Image.new("L", (advance, height), 0)
        ImageDraw.Draw(image).text((0, ascent), ch, font=font, fill=255, anchor="ls")
        data += image.tobytes()
    blocks.append(struct.pack("<BBBBH", size, advance, height, ascent, len(chars)) + b"".join(struct.pack("<I", ord(ch)) for ch in chars) + bytes(data))
    print(size, advance, height, ascent)
with open(out, "wb") as file:
    file.write(struct.pack("<B", len(blocks)))
    for block in blocks:
        file.write(block)
