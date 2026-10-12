"""Generates the Wax icon (icons/icon.png + icons/icon.ico).

A vinyl record with grooves and highlights, a green label and a wave-shaped "W".
Usage:  python dev/make_icon.py
"""
import math
from pathlib import Path

from PIL import Image, ImageDraw, ImageFilter

S = 2048                     # working canvas (supersampling)
C = S / 2
OUT = Path(__file__).resolve().parent.parent / "icons"


def circle(draw, r, **kw):
    draw.ellipse((C - r, C - r, C + r, C + r), **kw)


def disc_mask(r):
    m = Image.new("L", (S, S), 0)
    circle(ImageDraw.Draw(m), r, fill=255)
    return m


def make(label=0.205, hole=True):
    R = S * 0.47             # record radius
    RL = S * label           # label radius (larger at small sizes)
    img = Image.new("RGBA", (S, S), (0, 0, 0, 0))

    # Soft shadow under the record
    shadow = Image.new("RGBA", (S, S), (0, 0, 0, 0))
    sd = ImageDraw.Draw(shadow)
    sd.ellipse((C - R, C - R + S * 0.012, C + R, C + R + S * 0.012), fill=(0, 0, 0, 150))
    img = Image.alpha_composite(img, shadow.filter(ImageFilter.GaussianBlur(S * 0.012)))

    # Record: dark radial gradient
    disc = Image.new("RGBA", (S, S), (0, 0, 0, 0))
    dd = ImageDraw.Draw(disc)
    steps = 120
    for i in range(steps):
        t = i / steps
        r = R * (1 - t)
        v = int(30 - 18 * t)               # edge #1e1e1e → center #0c0c0c
        circle(dd, r, fill=(v, v, v, 255))

    # Grooves: thin alternating rings
    g = RL * 1.12
    k = 0
    while g < R * 0.965:
        w = S * 0.0035
        shade = 44 if k % 3 else 54
        circle(dd, g, outline=(shade, shade, shade, 255), width=max(1, int(w)))
        g += S * (0.0105 + 0.004 * ((k * 7) % 3) / 2)
        k += 1
    # Gap between tracks (a slightly darker, wider ring)
    circle(dd, R * 0.70, outline=(10, 10, 10, 255), width=int(S * 0.012))
    # Record edge
    circle(dd, R - S * 0.004, outline=(58, 58, 58, 255), width=int(S * 0.007))
    img = Image.alpha_composite(img, disc)

    # Highlights: two opposite light wedges, blurred and clipped to the record
    sheen = Image.new("RGBA", (S, S), (0, 0, 0, 0))
    sh = ImageDraw.Draw(sheen)
    box = (C - R, C - R, C + R, C + R)
    sh.pieslice(box, 200, 245, fill=(255, 255, 255, 46))
    sh.pieslice(box, 20, 65, fill=(255, 255, 255, 30))
    sheen = sheen.filter(ImageFilter.GaussianBlur(S * 0.03))
    ring = disc_mask(R * 0.985)
    ImageDraw.Draw(ring).ellipse((C - RL * 1.05, C - RL * 1.05, C + RL * 1.05, C + RL * 1.05), fill=0)
    sheen.putalpha(Image.composite(sheen.getchannel("A"), Image.new("L", (S, S), 0), ring))
    img = Image.alpha_composite(img, sheen)

    # Green label with a diagonal gradient
    label = Image.new("RGBA", (S, S), (0, 0, 0, 0))
    lg = Image.linear_gradient("L").rotate(-35, resample=Image.BICUBIC).resize((S, S))
    top, bottom = (36, 222, 108), (22, 158, 74)        # #24DE6C → #169E4A
    grad = Image.merge("RGB", [
        lg.point(lambda v, a=a, b=b: int(a + (b - a) * v / 255)) for a, b in zip(top, bottom)
    ])
    label.paste(grad, (0, 0), disc_mask(RL))
    ld = ImageDraw.Draw(label)
    circle(ld, RL, outline=(16, 120, 56, 255), width=int(S * 0.006))
    img = Image.alpha_composite(img, label)

    # A "W" shaped like a sound wave, white stroke with round caps
    mark = Image.new("RGBA", (S, S), (0, 0, 0, 0))
    md = ImageDraw.Draw(mark)
    u = RL / 0.205 * 0.001                              # unit relative to the size
    pts = [(-108, -52), (-56, 62), (0, -20), (56, 62), (108, -52)]
    pts = [(C + x * u, C + y * u) for x, y in pts]
    w = int(46 * u)
    md.line(pts, fill=(255, 255, 255, 255), width=w, joint="curve")
    for x, y in (pts[0], pts[-1]):
        md.ellipse((x - w / 2, y - w / 2, x + w / 2, y + w / 2), fill=(255, 255, 255, 255))
    if hole:  # spindle hole, below the W
        hx, hy, hr = C, C + 112 * u, 15 * u
        md.ellipse((hx - hr, hy - hr, hx + hr, hy + hr), fill=(12, 12, 12, 255))
    img = Image.alpha_composite(img, mark)
    return img


def main():
    OUT.mkdir(exist_ok=True)
    big = make()
    small = make(label=0.30, hole=False)   # 16–48 px: a large label so the W stays readable
    big.resize((512, 512), Image.LANCZOS).save(OUT / "icon.png")
    sizes = [256, 128, 64, 48, 32, 24, 16]
    imgs = [(big if s > 48 else small).resize((s, s), Image.LANCZOS) for s in sizes]
    imgs[0].save(OUT / "icon.ico", sizes=[(s, s) for s in sizes], append_images=imgs[1:])
    # preview on light and dark backgrounds
    prev = Image.new("RGBA", (1100, 300), (40, 40, 40, 255))
    ImageDraw.Draw(prev).rectangle((550, 0, 1100, 300), fill=(235, 235, 235, 255))
    x = 20
    for s in (256, 64, 32, 16):
        ic = (big if s > 48 else small).resize((s, s), Image.LANCZOS)
        prev.alpha_composite(ic, (x, (300 - s) // 2))
        prev.alpha_composite(ic, (x + 550, (300 - s) // 2))
        x += s + 30
    prev.save(OUT.parent / "dev" / "icon_preview.png")
    print("ok:", OUT / "icon.png", OUT / "icon.ico")


if __name__ == "__main__":
    main()
