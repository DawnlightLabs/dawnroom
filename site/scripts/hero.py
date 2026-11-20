"""Records one job's real step previews for the landing hero: runs a job on a running engine, keeps
every `preview` frame from its event stream, then the finished print and its recipe, all from that
same job (never typed by hand).

    python scripts/hero.py quality 3824736140 "A lighthouse at first light, long exposure, film grain"

Writes public/hero/<model>-step-NN.webp, public/hero/<model>.webp (+ -640.webp) and src/hero.json.
`--finish <job id> <preset>` redoes the print and hero.json from frames already saved.
"""
import base64
import io
import json
import sys
import urllib.request
from pathlib import Path

from PIL import Image

ENGINE = "http://127.0.0.1:8420"
SITE = Path(__file__).resolve().parent.parent
OUT = SITE / "public" / "hero"


def post(path: str, body: dict) -> dict:
    req = urllib.request.Request(f"{ENGINE}{path}", data=json.dumps(body).encode(), headers={"Content-Type": "application/json"})
    with urllib.request.urlopen(req) as r:
        return json.load(r)


def events(job_id: str):
    """Yields (event, data) from the job's SSE stream until it closes."""
    with urllib.request.urlopen(f"{ENGINE}/api/jobs/{job_id}/events", timeout=300) as r:
        name, data = None, []
        for raw in r:
            line = raw.decode("utf-8").rstrip("\r\n")
            if line.startswith("event:"):
                name = line[6:].strip()
            elif line.startswith("data:"):
                data.append(line[5:].lstrip())
            elif not line and data:
                yield name, json.loads("\n".join(data))
                name, data = None, []


def main(preset: str, seed: int, prompt: str) -> None:
    pick = {"preset": preset} if preset in ("fast", "quality") else {"model": preset}
    job = post("/api/jobs", {"prompt": prompt, "seed": seed, **pick})
    print("job", job["id"], job["model"])
    OUT.mkdir(parents=True, exist_ok=True)
    model, frames, final = job["model"], [], None
    for name, d in events(job["id"]):
        if name == "preview":
            f = OUT / f"{model}-step-{d['step']:02d}.webp"
            Image.open(io.BytesIO(base64.b64decode(d["b64_png"]))).convert("RGB").save(f, "WEBP", quality=70, method=6)
            frames.append({"step": d["step"], "total": d["total"], "file": f"/hero/{f.name}"})
            print("preview", d["step"], "/", d["total"])
        elif name == "progress":
            print("progress", d.get("stage"), d.get("step"), d.get("total"))
        elif name == "finished":
            final = d["job"]
    if not final or final.get("status") != "completed":
        sys.exit(f"job didn't complete: {final and final.get('status')} {final and final.get('error')}")
    finish(job["id"], preset, frames)


def finish(job_id: str, preset: str, frames: list) -> None:
    """The finished print and its recipe; the image id is the job id."""
    with urllib.request.urlopen(f"{ENGINE}/api/images?limit=20") as r:
        img = next(i for i in json.load(r)["items"] if i["id"] == job_id)
    model = img["model"]
    im = Image.open(img["output_path"]).convert("RGB")
    im.thumbnail((1024, 1024), Image.LANCZOS)
    buf = io.BytesIO()
    im.save(buf, "WEBP", quality=78, method=6)
    (OUT / f"{model}.webp").write_bytes(buf.getvalue())
    small = im.copy()
    small.thumbnail((640, 640), Image.LANCZOS)
    small.save(OUT / f"{model}-640.webp", "WEBP", quality=76, method=6)
    p = img["params"]
    hero = {
        "model": model, "file": f"/hero/{model}.webp", "frame": img["frame"], "prompt": img["prompt"],
        "seed": p["seed"], "steps": p["steps"], "width": p["width"], "height": p["height"],
        "seconds": round((img["finished_ms"] - img["started_ms"]) / 1000, 1),
        "preset": preset, "frames": frames,
    }
    (SITE / "src" / "hero.json").write_text(json.dumps(hero, indent=2, ensure_ascii=False) + "\n", encoding="utf-8")
    print(f"final {len(buf.getvalue()) // 1000} KB, {len(frames)} frames")


if __name__ == "__main__":
    if sys.argv[1] == "--finish":  # redo the last step for a job whose frames are already saved
        _, _, job_id, preset = sys.argv
        steps = sorted(OUT.glob("*-step-*.webp"))
        total = len(steps)
        main_frames = [{"step": i + 1, "total": total, "file": f"/hero/{f.name}"} for i, f in enumerate(steps)]
        finish(job_id, preset, main_frames)
    else:
        main(sys.argv[1], int(sys.argv[2]), sys.argv[3])
