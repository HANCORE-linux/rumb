"""Composes a screenshot from the files the motion_frames test writes.

usage: RUMB_VIDEO=out RUMB_ONLY=480 RUMB_THEME=shadesofjade RUMB_COLUMNS=190 RUMB_ROWS=42 \\
         cargo test --release motion_frames
       python3 docs/screenshot.py out/video-0480.json JetBrainsMono-Regular.ttf JetBrainsMono-Bold.ttf docs/screenshot.png
The pictures are laid over the text grid, the way a terminal draws them.
"""
import json
import os
import sys

from PIL import Image, ImageDraw, ImageFont

data_path, regular, bold, out = sys.argv[1:5]
data = json.load(open(data_path))
cw, ch = data["cell"]
canvas = Image.new("RGBA", (data["width"] * cw, data["height"] * ch), data["bg"])
draw = ImageDraw.Draw(canvas)
size = cw / 0.6
fonts = {False: ImageFont.truetype(regular, size), True: ImageFont.truetype(bold, size)}
ascent = fonts[False].getmetrics()[0]
top = (ch - sum(fonts[False].getmetrics())) // 2
for y, row in enumerate(data["rows"]):
    x = 0
    for text, fg, bg, is_bold in row:
        if bg:
            draw.rectangle([x * cw, y * ch, (x + len(text)) * cw - 1, (y + 1) * ch - 1], fill=bg)
        for k, symbol in enumerate(text):
            if symbol != " ":
                draw.text(((x + k) * cw, y * ch + top + ascent), symbol, font=fonts[bool(is_bold)], fill=fg or data["fg"], anchor="ls")
        x += len(text)
folder = os.path.dirname(data_path)
for picture in data["images"]:
    image = Image.open(os.path.join(folder, picture["png"])).convert("RGBA")
    canvas.alpha_composite(image, (picture["x"] * cw, picture["y"] * ch))
canvas.convert("RGB").save(out, optimize=True)
