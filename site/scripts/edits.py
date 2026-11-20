"""Turns a real edit from the library into the landing's Generate | Edit example: the original and the
edit as small WebP files in public/edits, and both recipes (prompt, model, seed, steps, time, Auto's
reason) in src/edits.json, read from a running engine's /api/images (never typed by hand).

    python scripts/edits.py 30 [--engine http://127.0.0.1:8424]

The frame given is the edit; its original is the Library image it was edited from (parent_id).
"""
import io
import json
import sys
import urllib.request
from pathlib import Path

from PIL import Image

SITE = Path(__file__).resolve().parent.parent
OUT = SITE / "public" / "edits"
MAX_BYTES = 200_000


def webp(path: str) -> bytes:
    im = Image.open(path).convert("RGB")
    im.thumbnail((1024, 1024), Image.LANCZOS)
    for q in (80, 72, 64, 56, 48):
        buf = io.BytesIO()
        im.save(buf, "WEBP", quality=q, method=6)
        if buf.tell() <= MAX_BYTES:
            return buf.getvalue()
    return buf.getvalue()


def recipe(item: dict, models: dict, name: str) -> dict:
    p = item["params"]
    secs = round((item["finished_ms"] - item["started_ms"]) / 1000, 1)
    (OUT / name).write_bytes(webp(item["output_path"]))
    return {
        "file": f"/edits/{name}",
        "frame": item["frame"],
        "prompt": item["prompt"],
        "model": item["model"],
        "modelName": models.get(item["model"], item["model"]),
        "seed": item["seed"],
        "steps": p["steps"],
        "width": p["width"],
        "height": p["height"],
        "seconds": secs,
        "task": p.get("task", "generate"),
        "auto": p.get("auto"),
    }


def main(frame: int, engine: str) -> None:
    with urllib.request.urlopen(f"{engine}/api/images?limit=200") as r:
        items = json.load(r)["items"]
    with urllib.request.urlopen(f"{engine}/api/models") as r:
        models = {m["id"]: m["name"] for m in json.load(r)["models"]}
    edit = next(i for i in items if i["frame"] == frame)
    original = next(i for i in items if i["id"] == edit["parent_id"])
    OUT.mkdir(parents=True, exist_ok=True)
    data = {
        "generate": recipe(original, models, f"original-{original['frame']:04d}.webp"),
        "edit": recipe(edit, models, f"edit-{edit['frame']:04d}.webp"),
        "gpu": "RTX 5060 Laptop",
    }
    (SITE / "src" / "edits.json").write_text(json.dumps(data, indent=2) + "\n", encoding="utf-8")
    print(json.dumps(data, indent=2))


if __name__ == "__main__":
    args = sys.argv[1:]
    eng = "http://127.0.0.1:8420"
    if "--engine" in args:
        i = args.index("--engine")
        eng = args[i + 1]
        del args[i : i + 2]
    main(int(args[0]), eng)
