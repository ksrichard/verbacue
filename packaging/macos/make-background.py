#!/usr/bin/env python3
"""Draws the DMG window background: packaging/macos/dmg-background.png and @2x.

The colors and fonts are the app's (crates/verbacue-gui/src/theme.rs). The arrow is
a skewer like the one in the icon. Run it again after changing the layout; the
icon positions must match ICON_Y, APP_X and APPS_X in dmg-settings.py.

Finder draws the icon labels in black in light mode and in white in dark mode, so
each label sits on a plate that both colors can be read on (contrast 4.5:1 or more).

Needs Pillow: python3 -m pip install pillow
"""
import os
from PIL import Image, ImageDraw, ImageFilter, ImageFont

HERE = os.path.dirname(os.path.abspath(__file__))
ROOT = os.path.join(HERE, "..", "..")
FONTS = os.path.join(ROOT, "crates", "verbacue-gui", "assets", "fonts")

# Window size in points. Keep in sync with dmg-settings.py.
W, H = 660, 400
ICON_Y, APP_X, APPS_X = 205, 170, 490

BG = (0x12, 0x0E, 0x0D)
GLOW = (0x2B, 0x1A, 0x14)
TEXT = (0xF3, 0xE9, 0xDC)
MUTED = (0xA8, 0x9A, 0x8D)
ACCENT = (0xE8, 0x62, 0x2A)
STEEL = (0xC9, 0xC2, 0xB8)
FLAME_LOW = (0xFF, 0x7A, 0x1A)
FLAME_MID = (0xFF, 0xB6, 0x27)
PLATE = (0x80, 0x73, 0x67)  # readable under black and under white text


def luminance(c):
    def ch(v):
        v /= 255
        return v / 12.92 if v <= 0.04045 else ((v + 0.055) / 1.055) ** 2.4

    r, g, b = (ch(v) for v in c)
    return 0.2126 * r + 0.7152 * g + 0.0722 * b


def contrast(a, b):
    la, lb = sorted((luminance(a), luminance(b)), reverse=True)
    return (la + 0.05) / (lb + 0.05)


assert contrast(PLATE, (0, 0, 0)) >= 4.5 and contrast(PLATE, (255, 255, 255)) >= 4.5


def draw(scale):
    s = lambda v: round(v * scale)  # noqa: E731
    img = Image.new("RGB", (s(W), s(H)), BG)

    # A warm glow under the icons, like the embers of a grill.
    glow = Image.new("L", img.size, 0)
    ImageDraw.Draw(glow).ellipse(
        (s(W / 2 - 300), s(ICON_Y - 120), s(W / 2 + 300), s(ICON_Y + 260)), fill=255
    )
    glow = glow.filter(ImageFilter.GaussianBlur(s(70)))
    img = Image.composite(Image.new("RGB", img.size, GLOW), img, glow)
    d = ImageDraw.Draw(img)

    # Title and hint.
    title = ImageFont.truetype(os.path.join(FONTS, "SpaceGrotesk-Bold.ttf"), s(30))
    body = ImageFont.truetype(os.path.join(FONTS, "Inter-Medium.ttf"), s(14))
    d.text((s(W / 2), s(52)), "verbacue", font=title, fill=TEXT, anchor="mm")
    d.text(
        (s(W / 2), s(84)),
        "Drag verbacue to Applications to install it",
        font=body,
        fill=MUTED,
        anchor="mm",
    )

    # The skewer arrow between the two icons.
    x0, x1, y = APP_X + 92, APPS_X - 92, ICON_Y
    flames = Image.new("RGBA", img.size, (0, 0, 0, 0))
    fd = ImageDraw.Draw(flames)
    for i, cx in enumerate(range(x0 + 34, x1 - 20, 22)):
        h = 20 if i % 2 else 26
        fd.ellipse((s(cx - 7), s(y + 6), s(cx + 7), s(y + 6 + h)), fill=FLAME_LOW + (150,))
        fd.ellipse((s(cx - 4), s(y + 10), s(cx + 4), s(y + 4 + h)), fill=FLAME_MID + (170,))
    flames = flames.filter(ImageFilter.GaussianBlur(s(2.5)))
    img.paste(flames, (0, 0), flames)
    d = ImageDraw.Draw(img)

    d.ellipse((s(x0 - 9), s(y - 9), s(x0 + 9), s(y + 9)), outline=STEEL, width=s(3))
    d.rounded_rectangle((s(x0 + 9), s(y - 2.5), s(x1 - 14), s(y + 2.5)), radius=s(2.5), fill=STEEL)
    d.polygon(
        [(s(x1 - 22), s(y - 11)), (s(x1 + 4), s(y)), (s(x1 - 22), s(y + 11)), (s(x1 - 16), s(y))],
        fill=ACCENT,
    )

    # Plates under the labels Finder draws below each 128-point icon.
    for cx in (APP_X, APPS_X):
        d.rounded_rectangle(
            (s(cx - 62), s(ICON_Y + 70), s(cx + 62), s(ICON_Y + 94)), radius=s(12), fill=PLATE
        )
    return img


if __name__ == "__main__":
    draw(1).save(os.path.join(HERE, "dmg-background.png"), dpi=(72, 72), optimize=True)
    draw(2).save(os.path.join(HERE, "dmg-background@2x.png"), dpi=(144, 144), optimize=True)
    print("wrote dmg-background.png and dmg-background@2x.png")
