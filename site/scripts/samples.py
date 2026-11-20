"""Turns picked library frames into site samples: small WebP files in public/samples and their
recipes in src/samples.json, read from a running engine's /api/images (never typed by hand).

    python scripts/samples.py 28 50 51 26 33 20 16 49 59 61 63 65
"""
import io
import json
import sys
import urllib.request
from pathlib import Path

from PIL import Image

ENGINE = "http://127.0.0.1:8420"
SITE = Path(__file__).resolve().parent.parent
OUT = SITE / "public" / "samples"
MAX_BYTES = 200_000
LONG_SIDE = 1024


def webp(path: str) -> bytes:
    im = Image.open(path).convert("RGB")
    im.thumbnail((LONG_SIDE, LONG_SIDE), Image.LANCZOS)
    for q in (82, 76, 70, 64, 58, 52, 46):
        buf = io.BytesIO()
        im.save(buf, "WEBP", quality=q, method=6)
        if buf.tell() <= MAX_BYTES:
            return buf.getvalue()
    im.thumbnail((896, 896), Image.LANCZOS)
    buf = io.BytesIO()
    im.save(buf, "WEBP", quality=60, method=6)
    return buf.getvalue()


def main(frames: list[int]) -> None:
    with urllib.request.urlopen(f"{ENGINE}/api/images?limit=200") as r:
        items = {i["frame"]: i for i in json.load(r)["items"]}
    OUT.mkdir(parents=True, exist_ok=True)
    samples = []
    for f in frames:
        i = items[f]
        p = i["params"]
        # Samples belong to the model, not a size tag: "flux1-schnell:q4" → "flux1-schnell".
        model = i["model"].split(":")[0]
        data = webp(i["output_path"])
        name = f"{model}-{f:04d}.webp"
        (OUT / name).write_bytes(data)
        samples.append({
            "model": model, "file": f"/samples/{name}", "frame": f, "prompt": i["prompt"],
            "seed": p["seed"], "steps": p["steps"], "width": p["width"], "height": p["height"],
            "seconds": round((i["finished_ms"] - i["started_ms"]) / 1000, 1),
        })
        print(f"{name}: {len(data) // 1000} KB")
    (SITE / "src" / "samples.json").write_text(json.dumps(samples, indent=2, ensure_ascii=False) + "\n", encoding="utf-8")


if __name__ == "__main__":
    main([int(a) for a in sys.argv[1:]])
