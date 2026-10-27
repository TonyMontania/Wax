"""Makes docs/social-preview.png (1280×640), the picture GitHub shows when the repo's link is shared.
Upload it in the repo's Settings → General → Social preview.  Run: python dev/make_social.py"""
from pathlib import Path

from PIL import Image, ImageDraw, ImageFilter, ImageFont

ROOT = Path(__file__).resolve().parent.parent
W, H = 1280, 640
BG, GREEN, TEXT, SUB = (18, 18, 18), (29, 185, 84), (255, 255, 255), (179, 179, 179)


def font(size, bold=False):
    for name in (["segoeuib.ttf", "arialbd.ttf", "DejaVuSans-Bold.ttf"] if bold else ["segoeui.ttf", "arial.ttf", "DejaVuSans.ttf"]):
        try:
            return ImageFont.truetype(name, size)
        except OSError:
            pass
    return ImageFont.load_default()


img = Image.new("RGB", (W, H), BG)
# A soft green glow behind the screenshot
glow = Image.new("RGB", (W, H), BG)
ImageDraw.Draw(glow).ellipse((560, 60, 1360, 700), fill=(20, 90, 45))
img = Image.blend(img, glow.filter(ImageFilter.GaussianBlur(140)), 0.9)

shot = Image.open(ROOT / "docs/screenshots/browse.webp").convert("RGB")
sw = 720
shot = shot.resize((sw, round(shot.height * sw / shot.width)), Image.LANCZOS)
x, y = W - sw - 40, (H - shot.height) // 2
shadow = Image.new("L", (W, H), 0)
ImageDraw.Draw(shadow).rounded_rectangle((x + 8, y + 16, x + sw + 8, y + shot.height + 16), 14, fill=170)
img.paste((0, 0, 0), (0, 0), shadow.filter(ImageFilter.GaussianBlur(18)))
mask = Image.new("L", shot.size, 0)
ImageDraw.Draw(mask).rounded_rectangle((0, 0, *shot.size), 12, fill=255)
img.paste(shot, (x, y), mask)

icon = Image.open(ROOT / "icons/icon.png").convert("RGBA").resize((112, 112), Image.LANCZOS)
img.paste(icon, (64, 150), icon)
d = ImageDraw.Draw(img)
d.text((64, 282), "Wax", font=font(96, True), fill=TEXT)
lines = ["The classic Spotify app,", "for the music on your computer."]
for i, line in enumerate(lines):
    d.text((66, 400 + i * 40), line, font=font(30), fill=SUB)
d.rounded_rectangle((64, 506, 64 + 330, 506 + 46), 23, fill=GREEN)
d.text((64 + 165, 529), "Windows · macOS · Linux", font=font(22, True), fill=TEXT, anchor="mm")

out = ROOT / "docs/social-preview.png"
img.save(out, optimize=True)
print(out, out.stat().st_size // 1024, "KB")
