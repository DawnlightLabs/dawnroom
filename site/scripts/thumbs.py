"""Small strip thumbnails for the contact sheet (the loupe loads the full print on demand).

    python scripts/thumbs.py
"""
from pathlib import Path

from PIL import Image

SITE = Path(__file__).resolve().parent.parent
SRC = SITE / "public" / "samples"
OUT = SRC / "thumb"
LONG_SIDE = 400

OUT.mkdir(exist_ok=True)
for f in sorted(SRC.glob("*.webp")):
    im = Image.open(f).convert("RGB")
    im.thumbnail((LONG_SIDE, LONG_SIDE), Image.LANCZOS)
    im.save(OUT / f.name, "WEBP", quality=72, method=6)
    print(f.name, (OUT / f.name).stat().st_size // 1000, "KB")
