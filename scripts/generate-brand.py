"""Regenerate the code-drawn project icon and documentation sample (Pillow).

These are vector-like brand assets, not screenshots of a user's desktop.
Usage: python scripts/generate-brand.py
"""
from pathlib import Path
from PIL import Image, ImageDraw, ImageFont

ROOT = Path(__file__).resolve().parents[1]
SCALE = 4
icon = Image.new("RGBA", (1024, 1024))
d = ImageDraw.Draw(icon)
d.rounded_rectangle((0, 0, 1023, 1023), radius=272, fill="#6852df")
d.rounded_rectangle((248, 248, 776, 776), radius=160, outline="white", width=56)
d.ellipse((408, 408, 616, 616), fill="white")
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
