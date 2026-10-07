"""Regenerate the code-drawn Pip icon and documentation sample (Pillow).

These are vector-like brand assets, not screenshots of a user's desktop.
Usage: python scripts/generate-brand.py
"""
from pathlib import Path
from PIL import Image, ImageDraw, ImageFont

ROOT = Path(__file__).resolve().parents[1]
SCALE = 4
# Pip: same 128-unit geometry as crates/app/ui/icon.svg and components/pip.slint.
U = 8
icon = Image.new("RGBA", (128 * U, 128 * U))
d = ImageDraw.Draw(icon)
box = lambda x, y, w, h: (x * U, y * U, (x + w) * U, (y + h) * U)
d.rounded_rectangle(box(76, 6, 28, 20), radius=9 * U, fill="#6656e0")
d.rounded_rectangle(box(10, 16, 108, 104), radius=42 * U, fill="#8b7cf6")
sheen = Image.new("RGBA", icon.size)
ImageDraw.Draw(sheen).rounded_rectangle(box(26, 25, 22, 9), radius=4.5 * U, fill=(255, 255, 255, 71))
icon.alpha_composite(sheen)
d = ImageDraw.Draw(icon)
for x in (17, 93):
    d.rounded_rectangle(box(x, 82, 18, 11), radius=5.5 * U, fill="#ffb8c8")
d.ellipse(box(35, 33, 58, 58), fill="white")
d.ellipse(box(49, 47, 30, 30), fill="#ff6b7d")
d.ellipse(box(69, 50, 9, 9), fill="white")
smile = [(54 + 20 * t, 99 + 16 * t * (1 - t)) for t in (i / 24 for i in range(25))]
d.line([(x * U, y * U) for x, y in smile], fill="#2b2238", width=int(4.5 * U), joint="curve")
for x, y in (smile[0], smile[-1]):
    r = 2.25 * U
    d.ellipse((x * U - r, y * U - r, x * U + r, y * U + r), fill="#2b2238")
icon = icon.resize((256, 256), Image.Resampling.LANCZOS)
assets = ROOT / "crates/app/assets"
images = ROOT / "docs/images"
assets.mkdir(parents=True, exist_ok=True)
images.mkdir(parents=True, exist_ok=True)
icon.save(assets / "fastrecorder.ico", sizes=[(16,16),(20,20),(24,24),(32,32),(40,40),(48,48),(64,64),(128,128),(256,256)])
icon.save(images / "logo.png")

def font(size, bold=False):
    candidates = [Path("C:/Windows/Fonts") / ("segoeuib.ttf" if bold else "segoeui.ttf"),
                  Path("/usr/share/fonts/truetype/dejavu") / ("DejaVuSans-Bold.ttf" if bold else "DejaVuSans.ttf")]
    for path in candidates:
        if path.exists():
            return ImageFont.truetype(str(path), size)
    return ImageFont.load_default(size=size)

# A deterministic, privacy-safe preview for the actual Slint studio screenshot.
sample = Image.new("RGB", (1600, 900), "#dce5e7")
d = ImageDraw.Draw(sample)
d.rounded_rectangle((98, 64, 1502, 836), radius=28, fill="#fbfcfc")
d.rounded_rectangle((98, 64, 386, 836), radius=28, fill="#edf1f2")
d.rectangle((350, 64, 386, 836), fill="#edf1f2")
d.text((142, 109), "Notes", font=font(30, True), fill="#263b42")
d.rounded_rectangle((122, 180, 363, 239), radius=10, fill="#dce7e6")
d.text((146, 193), "Getting started", font=font(22), fill="#315b58")
for y, label in [(265,"Project ideas"),(326,"This week"),(387,"Reading list")]:
    d.text((146, y), label, font=font(22), fill="#7a8b90")
d.text((456, 140), "A little space to think.", font=font(48, True), fill="#263b42")
d.text((459, 218), "Keep your ideas clear, and make something useful.", font=font(26), fill="#73848a")
for y, title, subtitle in [(322,"Capture an idea", "Record a quick walkthrough for your team."),
                           (457,"Explain it simply", "One clear example can make all the difference."),
                           (592,"Share when you are ready", "Save your recording and keep moving.")]:
    d.rounded_rectangle((459, y+4, 489, y+34), radius=8, fill="#e4eceb", outline="#9eaaa9", width=2)
    d.text((515, y-3), title, font=font(28,True), fill="#354b52")
    d.text((515, y+45), subtitle, font=font(24), fill="#7a8b90")
sample.save(images / "sample-desktop.png")
