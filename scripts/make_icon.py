"""Draws the app icon: an orange (Rust) gear with API braces on a navy tile.

python3 scripts/make_icon.py   (writes every size into crates/d365-openapi-tauri/icons)
"""
import math
from PIL import Image, ImageDraw, ImageFont

S = 2048  # draw large, downscale for antialiasing
ICONS = "crates/d365-openapi-tauri/icons"

img = Image.new("RGBA", (S, S), (0, 0, 0, 0))

# Navy tile with a vertical gradient and rounded corners.
grad = Image.new("RGBA", (S, S))
top, bottom = (36, 69, 110), (17, 32, 56)
gd = ImageDraw.Draw(grad)
for y in range(S):
    t = y / (S - 1)
    gd.line([(0, y), (S, y)], fill=tuple(round(a + (b - a) * t) for a, b in zip(top, bottom)) + (255,))
mask = Image.new("L", (S, S), 0)
m = 0.06 * S
ImageDraw.Draw(mask).rounded_rectangle([m, m, S - m, S - m], radius=0.2 * S, fill=255)
img.paste(grad, (0, 0), mask)

d = ImageDraw.Draw(img)
cx, cy = S / 2, S / 2
teeth = 10
r_out, r_in = 0.36 * S, 0.29 * S
pts = []
for i in range(teeth):
    a = 2 * math.pi * i / teeth
    step = 2 * math.pi / teeth
    # tooth: flat top spanning ~40% of the step, sloped flanks
    for frac, r in [(-0.30, r_in), (-0.18, r_out), (0.18, r_out), (0.30, r_in)]:
        ang = a + frac * step
        pts.append((cx + r * math.cos(ang), cy + r * math.sin(ang)))
orange = (247, 76, 0, 255)
d.polygon(pts, fill=orange)
d.ellipse([cx - r_in - 2, cy - r_in - 2, cx + r_in + 2, cy + r_in + 2], fill=orange)

# Hole in the gear, showing API braces.
r_hole = 0.19 * S
d.ellipse([cx - r_hole, cy - r_hole, cx + r_hole, cy + r_hole], fill=(20, 38, 64, 255))
font = ImageFont.truetype("/usr/share/fonts/truetype/dejavu/DejaVuSansMono-Bold.ttf", int(0.24 * S))
d.text((cx, cy - 0.012 * S), "{}", font=font, fill=(255, 255, 255, 255), anchor="mm")

big = img.resize((1024, 1024), Image.LANCZOS)
for size, name in [(32, "32x32.png"), (128, "128x128.png"), (512, "icon.png")]:
    big.resize((size, size), Image.LANCZOS).save(f"{ICONS}/{name}")
big.save(f"{ICONS}/icon.icns")
big.save(f"{ICONS}/icon.ico", sizes=[(s, s) for s in (16, 24, 32, 48, 64, 128, 256)])
