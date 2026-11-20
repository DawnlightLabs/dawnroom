"""Model-card material for the library: each maker's showcase images (credited, linked) and best uses, from
scripts/data/model-cards.source.json (researched from the official model cards; uses are paraphrased).

Downloads each image into a scratch folder, converts it with Pillow to a small WebP for the model page and a
96 px thumbnail for library rows, and writes src/modelCards.json. Skip an image by listing its URL in SKIP;
drop a model's images entirely by removing its "images" in the source file.

    python -I scripts/cards.py [scratch-dir]
"""
import io
import json
import sys
import tempfile
import time
import urllib.request
from pathlib import Path

from PIL import Image

SITE = Path(__file__).resolve().parent.parent
SRC = SITE / "scripts" / "data" / "model-cards.source.json"
OUT = SITE / "public" / "cards"
UA = "dawnroom-site-build"
MAX_BYTES = 160_000
# Banners, charts and diagrams: not example generations (checked on a contact sheet).
SKIP = {
    "https://d1uqrfwi55x6ib.cloudfront.net/fibo-2.0-dev/assets/edit/masked_suitcase.png",
    "https://huggingface.co/OpenMOSS-Team/MOSS-Image-SFT/resolve/main/assets/showcase/layout-bboxes.webp",
    "https://qianwen-res.oss-accelerate.aliyuncs.com/Qwen-Image/image2.1/images/example-01.png",

    "https://huggingface.co/lodestones/Chroma1-HD/resolve/main/images/FictionalChromaBanner_1.png",
}


def fetch(url: str, dest: Path) -> Path:
    req = urllib.request.Request(url, headers={"User-Agent": UA})
    with urllib.request.urlopen(req, timeout=60) as r:
        dest.write_bytes(r.read())
    return dest


def webp(path: Path, long_side: int, max_bytes: int) -> tuple[bytes, int, int]:
    im = Image.open(path)
    im.seek(0)
    im = im.convert("RGB")
    im.thumbnail((long_side, long_side), Image.LANCZOS)
    for q in (80, 72, 64, 56, 48):
        buf = io.BytesIO()
        im.save(buf, "WEBP", quality=q, method=6)
        if buf.tell() <= max_bytes:
            break
    return buf.getvalue(), im.width, im.height


def thumb(path: Path) -> bytes:
    im = Image.open(path).convert("RGB")
    s = min(im.size)
    im = im.crop(((im.width - s) // 2, (im.height - s) // 2, (im.width + s) // 2, (im.height + s) // 2)).resize((96, 96), Image.LANCZOS)
    buf = io.BytesIO()
    im.save(buf, "WEBP", quality=72, method=6)
    return buf.getvalue()


def main(scratch: Path) -> None:
    src = json.loads(SRC.read_text(encoding="utf-8"))
    (OUT / "thumb").mkdir(parents=True, exist_ok=True)
    out = {}
    for mid, info in src.items():
        images = []
        for n, img in enumerate([i for i in info.get("images", []) if i["url"] not in SKIP], 1):
            raw = scratch / f"{mid}-{n}"
            try:
                fetch(img["url"], raw)
                data, w, h = webp(raw, 1024, MAX_BYTES)
            except Exception as e:  # a moved or unreadable image is left out, never fatal
                print(f"skip {mid} {img['url']}: {e}")
                continue
            name = f"{mid}-{n}.webp"
            (OUT / name).write_bytes(data)
            if not images:
                (OUT / "thumb" / name).write_bytes(thumb(raw))
            images.append({"file": f"/cards/{name}", "width": w, "height": h, "caption": img.get("caption", ""), "source": img.get("source_page") or info.get("card")})
            print(f"{name}: {len(data) // 1000} KB")
            time.sleep(0.5)
        out[mid] = {
            "card": info.get("card"),
            "site": info.get("site"),
            "uses": info.get("uses", []),
            "avoid": info.get("avoid", []),
            "sources": info.get("sources", []),
            "images": images,
            "thumb": f"/cards/thumb/{mid}-1.webp" if images else None,
        }
    (SITE / "src" / "modelCards.json").write_text(json.dumps(out, indent=2, ensure_ascii=False) + "\n", encoding="utf-8")


if __name__ == "__main__":
    main(Path(sys.argv[1]) if len(sys.argv) > 1 else Path(tempfile.mkdtemp(prefix="dawnroom-cards-")))
